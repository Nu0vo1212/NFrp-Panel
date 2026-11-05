//! EmbeddedDriver（预留）：后续直接链接 `nfrp-common` 在面板进程内起 frps/frpc。
//!
//! 第一版**不实现**，但把接口占位并给出明确、可解释的错误，
//! 避免前端出现"点了没反应"的空洞。
//!
//! 实现路径（计划）：
//! 1. 以 `nfrp-server` / `nfrp-client` 的库化 API（`Registry::new` + `serve::run`）在
//!    独立 tokio 任务里启动；
//! 2. 配置使用 `nfrp_common::config::ServerConfig`，由面板内存持有，不落盘；
//! 3. Admin API 调用改为**进程内直连**（省掉 HTTP 往返），但保持同一套能力语义；
//! 4. 代理动态管理直接走 `registry`，无需 Admin API。

use std::sync::Arc;

use async_trait::async_trait;

use crate::db::models::InstanceRow;
use crate::domain::*;
use crate::error::ApiError;

use super::{unsupported, Driver, HealthReport, StopOptions, WriteOptions};

pub struct EmbeddedDriver {
    row: InstanceRow,
}

impl EmbeddedDriver {
    pub fn new(row: InstanceRow) -> Self {
        Self { row }
    }

    fn not_ready(&self, what: &str) -> ApiError {
        unsupported(
            DriverKind::Embedded,
            what,
            "内嵌驱动为预留接口，第一版未实现（可改用二进制 / SSH 驱动）",
        )
    }
}

#[async_trait]
impl Driver for EmbeddedDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Embedded
    }

    fn instance_id(&self) -> uuid::Uuid {
        self.row.id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn start(&self) -> Result<InstanceStatus, ApiError> {
        Err(self.not_ready("启动"))
    }
    async fn stop(&self, _o: StopOptions) -> Result<InstanceStatus, ApiError> {
        Err(self.not_ready("停止"))
    }
    async fn restart(&self) -> Result<InstanceStatus, ApiError> {
        Err(self.not_ready("重启"))
    }
    async fn health(&self) -> Result<HealthReport, ApiError> {
        Ok(HealthReport {
            healthy: false,
            message: "内嵌驱动未实现".into(),
            checks: vec![("驱动可用性".into(), false, "预留接口".into())],
        })
    }
    async fn status(&self) -> Result<InstanceStatus, ApiError> {
        Ok(InstanceStatus::unknown(
            "内嵌驱动为预留接口，第一版未实现",
        ))
    }
    async fn reload(&self) -> Result<ReloadOutcome, ApiError> {
        Err(self.not_ready("热重载"))
    }
    async fn read_config(&self) -> Result<ConfigDoc, ApiError> {
        Err(self.not_ready("读取配置"))
    }
    async fn validate_config(&self, _c: &str) -> Result<ValidationReport, ApiError> {
        Err(self.not_ready("校验配置"))
    }
    async fn write_config(
        &self,
        _c: &str,
        _o: WriteOptions,
    ) -> Result<ConfigWriteResult, ApiError> {
        Err(self.not_ready("写配置"))
    }
    async fn list_backups(&self) -> Result<Vec<BackupInfo>, ApiError> {
        Ok(vec![])
    }
    async fn restore_backup(&self, _id: &str) -> Result<ConfigWriteResult, ApiError> {
        Err(self.not_ready("回滚"))
    }
    async fn logs(&self, _q: LogQuery) -> Result<LogPage, ApiError> {
        Err(self.not_ready("读取日志"))
    }
    async fn list_proxies(&self) -> Result<Vec<ProxyInfo>, ApiError> {
        Ok(vec![])
    }
    async fn create_proxy(&self, _s: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        Err(self.not_ready("新增代理"))
    }
    async fn update_proxy(&self, _n: &str, _s: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        Err(self.not_ready("更新代理"))
    }
    async fn delete_proxy(&self, _n: &str) -> Result<ProxyOpResult, ApiError> {
        Err(self.not_ready("删除代理"))
    }
    async fn metrics(&self) -> Result<MetricsSnapshot, ApiError> {
        Err(self.not_ready("采集指标"))
    }
    fn capabilities(&self) -> Arc<Capabilities> {
        let mut c = Capabilities::unknown(self.row.flavor);
        c.notes.push(CapabilityNote {
            level: "info".into(),
            text: "内嵌驱动为预留接口，第一版未实现".into(),
        });
        Arc::new(c)
    }
    async fn probe(&self) -> Result<Capabilities, ApiError> {
        Ok(self.capabilities().as_ref().clone())
    }
}
