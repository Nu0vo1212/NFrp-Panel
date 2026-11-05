//! 应用状态：数据库、密钥、认证、审计，以及「实例 → 驱动」的注册表。
//!
//! 注册表是驱动生命周期的唯一入口：创建实例时按 `driver` 构造对应驱动，
//! 并把解密后的凭据注入到驱动上下文（明文只存在于内存）。

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::audit::AuditWriter;
use crate::auth::AuthService;
use crate::config::RuntimeConfig;
use crate::crypto::KeyRing;
use crate::db::models::{CredentialKind, InstanceRow};
use crate::db::repo;
use crate::domain::DriverKind;
use crate::driver::Driver;
use crate::error::ApiError;

pub struct AppState {
    pub cfg: Arc<RuntimeConfig>,
    pub db: SqlitePool,
    pub keys: Arc<KeyRing>,
    pub auth: Arc<AuthService>,
    pub audit: AuditWriter,
    /// 驱动注册表：实例 ID → 驱动
    pub drivers: Arc<DriverRegistry>,
    /// 部署任务运行态（SSE 订阅者）
    pub deploy: Arc<crate::deploy::DeployHub>,
    /// 后台采样任务句柄
    pub poller: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AppState {
    /// 解密实例的 Admin API 密码（AAD 绑定实例 ID，防密文搬移）。
    pub fn decrypt_admin_password(&self, row: &InstanceRow) -> Option<String> {
        let enc = row.admin_api_password_enc.as_ref()?;
        self.keys
            .decrypt(enc, &format!("instance:{}:admin_password", row.id))
            .ok()
    }

    /// 解密 SSH 凭据（密码 / 私钥 / passphrase）。
    pub async fn decrypt_credential_async(
        &self,
        credential_id: Uuid,
    ) -> Result<(CredentialKind, String, Option<String>), ApiError> {
        let cred = repo::get_credential(&self.db, credential_id).await?;
        let _ = repo::touch_credential(&self.db, credential_id).await;
        let secret = self
            .keys
            .decrypt(&cred.secret_enc, &format!("credential:{}:secret", cred.id))?;
        let passphrase = match &cred.passphrase_enc {
            Some(p) => Some(
                self.keys
                    .decrypt(p, &format!("credential:{}:passphrase", cred.id))?,
            ),
            None => None,
        };
        Ok((cred.kind, secret, passphrase))
    }

    /// 构造（或取得）某实例的驱动。
    pub async fn driver(&self, id: Uuid) -> Result<Arc<dyn Driver>, ApiError> {
        self.drivers.get_or_build(id, self).await
    }
}

/// 驱动注册表。
pub struct DriverRegistry {
    map: Mutex<HashMap<Uuid, Arc<dyn Driver>>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    /// 实例变更后丢弃缓存（下次请求重建）。
    pub async fn invalidate(&self, id: Uuid) {
        self.map.lock().await.remove(&id);
    }

    pub async fn get_or_build(
        &self,
        id: Uuid,
        st: &AppState,
    ) -> Result<Arc<dyn Driver>, ApiError> {
        {
            let g = self.map.lock().await;
            if let Some(d) = g.get(&id) {
                return Ok(d.clone());
            }
        }
        let row = repo::get_instance(&st.db, id).await?;
        let driver = self.build(&row, st)?;
        self.map.lock().await.insert(id, driver.clone());
        Ok(driver)
    }

    fn build(&self, row: &InstanceRow, st: &AppState) -> Result<Arc<dyn Driver>, ApiError> {
        match row.driver {
            DriverKind::Binary => {
                if row.connection.is_remote() {
                    // SSH 连接却选了本机驱动：明确报错而不是静默降级
                    return Err(ApiError::bad_request(
                        "该实例配置为 SSH 远程连接，但驱动是本机二进制；请把驱动改为 SSH 二进制",
                    ));
                }
                let ctx = Arc::new(crate::driver::binary::BinaryContext {
                    row: row.clone(),
                    admin_password: st.decrypt_admin_password(row),
                    child: Mutex::new(None),
                    capabilities: Mutex::new(Arc::new(
                        row.capabilities
                            .clone()
                            .unwrap_or_else(|| crate::domain::Capabilities::unknown(row.flavor)),
                    )),
                    admin_down_until: Mutex::new(None),
                    cfg: (*st.cfg).clone(),
                    keys: st.keys.clone(),
                    db: st.db.clone(),
                });
                Ok(Arc::new(crate::driver::binary::BinaryDriver::new(ctx)))
            }
            DriverKind::AdminApi => Ok(Arc::new(crate::driver::adminapi::AdminApiDriver::new(
                row.clone(),
                st.decrypt_admin_password(row),
                st.cfg.raw.frp.admin_timeout_ms,
            )?)),
            DriverKind::SshBinary => Ok(Arc::new(crate::driver::ssh::SshBinaryDriver::new(
                row.clone(),
                st.keys.clone(),
                st.db.clone(),
                (*st.cfg).clone(),
            ))),
            DriverKind::Embedded => Ok(Arc::new(
                crate::driver::embedded::EmbeddedDriver::new(row.clone()),
            )),
        }
    }
}

impl Default for DriverRegistry {
    fn default() -> Self {
        Self::new()
    }
}
