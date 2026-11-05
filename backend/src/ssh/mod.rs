//! SSH 层：会话管理、主机密钥校验、白名单命令执行、远程环境探测。
//!
//! ## 依赖与权限要求
//! - 使用 `russh`（纯 Rust，ring 后端）：**不需要**系统安装 `ssh`/`scp`；
//! - 需要能连到目标主机 22（或自定义端口）；
//! - 需要 `sudo` 权限才能写 `/usr/local/bin`、`/etc/nfrp`、`systemctl`。
//!
//! ## 安全约束
//! 1. **主机密钥必须校验**：首次连接返回指纹要求人工确认，没有"跳过校验"开关；
//! 2. **命令白名单**：只允许模板化命令（见 [`exec::AllowedCommand`]），参数严格转义；
//! 3. **不在日志/审计里出现密码或私钥**：调用方传 `Secret` 包装类型。

pub mod exec;
pub mod hostkey;
pub mod probe;
pub mod session;
pub mod sftp;

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::RuntimeConfig;
use crate::db::models::InstanceRow;
use crate::domain::AuthKind;
use crate::error::ApiError;

pub use session::{ExecOutput, Phase, SshError, SshSession};

/// 一次 SSH 连接的配置（含解密后的凭据）。
#[derive(Clone)]
pub struct SshConnectParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_kind: AuthKind,
    /// 密码或私钥内容（已解密，仅内存）
    pub secret: Option<String>,
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for SshConnectParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnectParams")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_kind", &self.auth_kind)
            .field("secret", &self.secret.as_ref().map(|_| "‹已设置›"))
            .finish()
    }
}

/// 连接池：按实例缓存会话，减少握手开销。
pub struct SshPool {
    sessions: Mutex<std::collections::HashMap<uuid::Uuid, Arc<SshSession>>>,
}

impl SshPool {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub async fn invalidate(&self, id: uuid::Uuid) {
        self.sessions.lock().await.remove(&id);
    }

    /// 取得（必要时新建）会话；失活连接会被重建。
    pub async fn session(
        &self,
        id: uuid::Uuid,
        params: &SshConnectParams,
        cfg: &RuntimeConfig,
    ) -> Result<Arc<SshSession>, ApiError> {
        {
            let g = self.sessions.lock().await;
            if let Some(s) = g.get(&id) {
                if s.is_alive() {
                    return Ok(s.clone());
                }
            }
        }
        let s = Arc::new(SshSession::connect(params, cfg).await?);
        self.sessions.lock().await.insert(id, s.clone());
        Ok(s)
    }
}

impl Default for SshPool {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局连接池（进程内共享）。
pub fn global_pool() -> &'static SshPool {
    use std::sync::OnceLock;
    static POOL: OnceLock<SshPool> = OnceLock::new();
    POOL.get_or_init(SshPool::new)
}

/// 从实例记录构造 SSH 连接参数（凭据由调用方解密后放入）。
pub fn params_from_row(row: &InstanceRow) -> Result<(&str, u16, &str, AuthKind, Option<uuid::Uuid>), ApiError> {
    match &row.connection {
        crate::domain::Connection::Ssh {
            host,
            port,
            username,
            auth_kind,
            credential_id,
            ..
        } => {
            if host.trim().is_empty() {
                return Err(ApiError::bad_request("SSH 主机不能为空"));
            }
            Ok((host.as_str(), *port, username.as_str(), *auth_kind, *credential_id))
        }
        crate::domain::Connection::Local => Err(ApiError::bad_request(
            "该实例不是 SSH 连接（当前为本机模式）",
        )),
    }
}
