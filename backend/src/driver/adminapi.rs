//! AdminApiDriver：纳管**已在运行**的 frps/frpc 实例，只经 Admin API 管理。
//!
//! 诚实的边界（`docs/01-architecture.md` §5`）：
//! - 不接管进程：`start/stop/restart` 返回 `driver_unsupported`；
//! - 不拥有配置文件：配置读写、备份、回滚一律不支持（前端据此灰化入口）；
//! - 代理：只有 NFrp 支持动态增删；官方实现走不了动态，明确报错并给替代方案。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::db::models::InstanceRow;
use crate::domain::*;
use crate::error::ApiError;
use crate::frpapi::probe::{self, ProbeInput};
use crate::frpapi::{AdminAuth, FrpAdminClient};

use super::{unsupported, Driver, HealthReport, StopOptions, WriteOptions};

pub struct AdminApiDriver {
    row: InstanceRow,
    client: FrpAdminClient,
    capabilities: Arc<parking_lot_stub::Mutex<Capabilities>>,
}

/// 极简互斥（避免为单个字段引依赖）：用 std 的 Mutex 包装。
mod parking_lot_stub {
    use std::sync::Mutex as StdMutex;
    pub struct Mutex<T>(StdMutex<T>);
    impl<T> Mutex<T> {
        pub fn new(v: T) -> Self {
            Self(StdMutex::new(v))
        }
        pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut g)
        }
    }
}

impl AdminApiDriver {
    pub fn new(
        row: InstanceRow,
        admin_password: Option<String>,
        timeout_ms: u64,
    ) -> Result<Self, ApiError> {
        let client = FrpAdminClient::new(
            &row.admin_api_base_url,
            AdminAuth {
                user: row.admin_api_user.clone(),
                password: admin_password.unwrap_or_default(),
            },
            row.role,
            Duration::from_millis(timeout_ms),
            row.capabilities.as_ref().map(|c| c.api_kind),
        )?;
        let caps = row
            .capabilities
            .clone()
            .unwrap_or_else(|| Capabilities::unknown(row.flavor));
        Ok(Self {
            row,
            client,
            capabilities: Arc::new(parking_lot_stub::Mutex::new(caps)),
        })
    }

    /// 实时实现识别（缓存可能来自创建时的探测，实例可能已变化）。
    async fn live_api_kind(&self) -> ApiKind {
        if let Ok(k) = self.client.detect_api_kind().await {
            if k != ApiKind::None && k != ApiKind::Unknown {
                self.capabilities.with(|c| c.api_kind = k);
                return k;
            }
        }
        self.api_kind()
    }

    fn api_kind(&self) -> ApiKind {
        self.capabilities.with(|c| c.api_kind)
    }

    fn no_process(&self, what: &str) -> ApiError {
        unsupported(
            DriverKind::AdminApi,
            what,
            "面板不接管该实例的进程（它由外部启动与守护）",
        )
    }
}

#[async_trait]
impl Driver for AdminApiDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::AdminApi
    }

    fn instance_id(&self) -> uuid::Uuid {
        self.row.id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn start(&self) -> Result<InstanceStatus, ApiError> {
        Err(self.no_process("启动"))
    }

    async fn stop(&self, _opts: StopOptions) -> Result<InstanceStatus, ApiError> {
        Err(self.no_process("停止"))
    }

    async fn restart(&self) -> Result<InstanceStatus, ApiError> {
        Err(self.no_process("重启"))
    }

    async fn health(&self) -> Result<HealthReport, ApiError> {
        let ok = self.client.health(self.live_api_kind().await).await.unwrap_or(false);
        Ok(HealthReport {
            healthy: ok,
            message: if ok {
                "Admin API 可达".into()
            } else {
                "Admin API 不可达".into()
            },
            checks: vec![(
                "Admin API".into(),
                ok,
                self.client.base_url().to_string(),
            )],
        })
    }

    async fn status(&self) -> Result<InstanceStatus, ApiError> {
        let mut s = self.client.status(self.live_api_kind().await, self.row.role).await?;
        s.detail
            .notes
            .push("部署信息来自 Admin API（面板不接管进程）".into());
        Ok(s)
    }

    async fn reload(&self) -> Result<ReloadOutcome, ApiError> {
        // 不拥有配置文件：谈不上"让新配置生效"
        Ok(ReloadOutcome::Unsupported)
    }

    async fn read_config(&self) -> Result<ConfigDoc, ApiError> {
        Err(unsupported(
            DriverKind::AdminApi,
            "读取配置文件",
            "面板不拥有该实例的配置文件，只能通过 frp 自己的面板查看",
        ))
    }

    async fn validate_config(&self, _content: &str) -> Result<ValidationReport, ApiError> {
        Err(unsupported(
            DriverKind::AdminApi,
            "校验配置文件",
            "面板不拥有该实例的配置文件",
        ))
    }

    async fn write_config(
        &self,
        _content: &str,
        _opts: WriteOptions,
    ) -> Result<ConfigWriteResult, ApiError> {
        Err(unsupported(
            DriverKind::AdminApi,
            "写配置文件",
            "面板不拥有该实例的配置文件；如需托管请改用二进制模式或 SSH 模式",
        ))
    }

    async fn list_backups(&self) -> Result<Vec<BackupInfo>, ApiError> {
        Ok(vec![])
    }

    async fn restore_backup(&self, _id: &str) -> Result<ConfigWriteResult, ApiError> {
        Err(unsupported(DriverKind::AdminApi, "回滚配置", "没有备份"))
    }

    async fn logs(&self, _query: LogQuery) -> Result<LogPage, ApiError> {
        Err(unsupported(
            DriverKind::AdminApi,
            "读取日志",
            "面板无法访问该实例所在主机的日志文件；请改用 SSH 模式或查看 frp 自身面板",
        ))
    }

    async fn list_proxies(&self) -> Result<Vec<ProxyInfo>, ApiError> {
        self.client
            .list_proxies(self.live_api_kind().await, self.row.role)
            .await
    }

    async fn create_proxy(&self, spec: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        spec.validate().map_err(ApiError::bad_request)?;
        if self.live_api_kind().await != ApiKind::Nfrp {
            return Err(unsupported(
                DriverKind::AdminApi,
                "动态新增代理",
                "官方 frp 的面板 API 不提供代理增删（0.71.x 实测 PUT /api/config 返回 200 但不生效）",
            ));
        }
        match self.row.role {
            InstanceRole::Client => {
                crate::frpapi::nfrp::add_proxy_via_client(&self.client, &spec).await?;
            }
            InstanceRole::Server => {
                let run_id = self
                    .client
                    .get("/api/v2/clients?all=1")
                    .await
                    .ok()
                    .and_then(|r| r.json().ok())
                    .and_then(|v| {
                        v.get("data")?
                            .as_array()?
                            .first()?
                            .get("run_id")?
                            .as_str()
                            .map(str::to_string)
                    })
                    .ok_or_else(|| ApiError::upstream("服务端当前没有在线客户端"))?;
                crate::frpapi::nfrp::add_proxy_via_server(&self.client, &run_id, &spec).await?;
            }
        }
        let mut info = ProxyInfo::minimal(&spec.name, &spec.proxy_type);
        info.local_addr = spec.effective_local_addr();
        info.remote_port = spec.remote_port;
        info.status = "running".into();
        info.source = "dynamic".into();
        info.managed_by = "panel".into();
        Ok(ProxyOpResult {
            applied_via: "admin_api".into(),
            reload: ReloadOutcome::Hot,
            proxy: info,
            warnings: vec![],
        })
    }

    async fn update_proxy(&self, name: &str, spec: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        // 动态代理：先删后加
        self.delete_proxy(name).await?;
        self.create_proxy(spec).await
    }

    async fn delete_proxy(&self, name: &str) -> Result<ProxyOpResult, ApiError> {
        if self.live_api_kind().await != ApiKind::Nfrp {
            return Err(unsupported(
                DriverKind::AdminApi,
                "动态删除代理",
                "官方 frp 的面板 API 不提供代理删除",
            ));
        }
        let proxies = self
            .client
            .list_proxies(self.live_api_kind().await, self.row.role)
            .await
            .unwrap_or_default();
        let managed_by = proxies
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.managed_by.clone())
            .unwrap_or_else(|| "panel".into());

        match (self.row.role, managed_by.as_str()) {
            (InstanceRole::Client, _) => {
                crate::frpapi::nfrp::remove_proxy_via_client(&self.client, name).await?;
            }
            (InstanceRole::Server, "server_push") => {
                let run_id = self
                    .client
                    .get("/api/v2/clients?all=1")
                    .await
                    .ok()
                    .and_then(|r| r.json().ok())
                    .and_then(|v| {
                        v.get("data")?
                            .as_array()?
                            .first()?
                            .get("run_id")?
                            .as_str()
                            .map(str::to_string)
                    })
                    .ok_or_else(|| ApiError::upstream("无法确定服务端 run_id"))?;
                crate::frpapi::nfrp::remove_proxy_via_server(&self.client, &run_id, name).await?;
            }
            (InstanceRole::Server, _) => {
                return Err(ApiError::conflict(
                    "该代理由客户端配置文件定义，无法从服务端面板动态删除",
                ));
            }
        }
        Ok(ProxyOpResult {
            applied_via: "admin_api".into(),
            reload: ReloadOutcome::Hot,
            proxy: ProxyInfo::minimal(name, "unknown"),
            warnings: vec![],
        })
    }

    async fn metrics(&self) -> Result<MetricsSnapshot, ApiError> {
        let ts = chrono::Utc::now();
        let st = self.client.status(self.live_api_kind().await, self.row.role).await?;
        let counters = st.counters.unwrap_or(InstanceCounters {
            bytes_in: 0,
            bytes_out: 0,
            conns: 0,
            clients: 0,
            proxies: 0,
            requests: None,
        });
        let availability = if counters.bytes_in + counters.bytes_out > 0 {
            "full"
        } else if counters.conns + counters.clients + counters.proxies > 0 {
            "aggregated"
        } else {
            "none"
        };
        Ok(MetricsSnapshot {
            ts,
            api_reachable: st.api_reachable,
            counters,
            availability: availability.into(),
            source: "admin_api".into(),
        })
    }

    fn capabilities(&self) -> Arc<Capabilities> {
        Arc::new(self.capabilities.with(|c| c.clone()))
    }

    async fn probe(&self) -> Result<Capabilities, ApiError> {
        let caps = probe::probe(ProbeInput {
            client: Some(&self.client),
            role: self.row.role,
            flavor: self.row.flavor,
            managed_locally: false,
            remote: false,
            binary_version: self.row.binary_version.clone(),
            owns_config: false,
        })
        .await;
        self.capabilities.with(|c| *c = caps.clone());
        Ok(caps)
    }
}
