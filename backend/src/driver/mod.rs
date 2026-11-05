//! Driver trait：所有驱动对外暴露的**唯一**能力面。
//!
//! 设计约束：
//! - 调用方（API 层）不需要 `match driver_kind()`；
//! - 驱动不拥有的能力返回 `ApiError::DriverUnsupported`，绝不返回假数据；
//! - 所有方法都取 `&self`，驱动器实例存于 `DriverRegistry`，可并发调用。

pub mod adminapi;
pub mod binary;
pub mod embedded;
pub mod ssh;

use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::{
    BackupInfo, Capabilities, ConfigDoc, ConfigWriteResult, InstanceStatus, LogPage, LogQuery,
    MetricsSnapshot, ProxyInfo, ProxyOpResult, ProxySpec, ReloadOutcome, ValidationReport,
    DriverKind,
};
use crate::error::ApiError;

/// 停止进程的选项。
#[derive(Debug, Clone, Copy)]
pub struct StopOptions {
    /// 立即强制杀死，不做优雅终止。
    pub force: bool,
    /// 优雅终止的等待上限（毫秒）。
    pub grace_ms: u64,
}

impl Default for StopOptions {
    fn default() -> Self {
        Self {
            force: false,
            grace_ms: 5_000,
        }
    }
}

/// 写配置的选项。
#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    /// 写入后是否应用（reload / 重启）。
    pub apply: bool,
    /// 无法热重载时是否自动重启。
    pub auto_restart: bool,
    /// 是否先做备份（默认 true）。
    pub backup: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            apply: true,
            auto_restart: false,
            backup: true,
        }
    }
}

/// 驱动健康报告。
#[derive(Debug, Clone)]
pub struct HealthReport {
    pub healthy: bool,
    pub message: String,
    pub checks: Vec<(String, bool, String)>,
}

#[async_trait]
pub trait Driver: Send + Sync {
    fn kind(&self) -> DriverKind;

    /// 实例 ID（日志与审计用）。
    fn instance_id(&self) -> uuid::Uuid;

    /// 向下转换成具体驱动类型。
    ///
    /// 仅用于需要**驱动专有能力**的编排（例如 SSH 部署需要会话与凭据参数）；
    /// 常规能力一律走 trait 方法，不允许在业务代码里 `match` 驱动类型。
    fn as_any(&self) -> &dyn std::any::Any;

    // ---- 生命周期 ----
    async fn start(&self) -> Result<InstanceStatus, ApiError>;
    async fn stop(&self, opts: StopOptions) -> Result<InstanceStatus, ApiError>;
    async fn restart(&self) -> Result<InstanceStatus, ApiError>;
    async fn health(&self) -> Result<HealthReport, ApiError>;

    // ---- 状态 ----
    async fn status(&self) -> Result<InstanceStatus, ApiError>;
    async fn reload(&self) -> Result<ReloadOutcome, ApiError>;

    // ---- 配置 ----
    async fn read_config(&self) -> Result<ConfigDoc, ApiError>;
    async fn validate_config(&self, content: &str) -> Result<ValidationReport, ApiError>;
    async fn write_config(
        &self,
        content: &str,
        opts: WriteOptions,
    ) -> Result<ConfigWriteResult, ApiError>;
    async fn list_backups(&self) -> Result<Vec<BackupInfo>, ApiError>;
    async fn restore_backup(&self, backup_id: &str) -> Result<ConfigWriteResult, ApiError>;

    // ---- 日志 ----
    async fn logs(&self, query: LogQuery) -> Result<LogPage, ApiError>;

    // ---- 代理 ----
    async fn list_proxies(&self) -> Result<Vec<ProxyInfo>, ApiError>;
    async fn create_proxy(&self, spec: ProxySpec) -> Result<ProxyOpResult, ApiError>;
    async fn update_proxy(&self, name: &str, spec: ProxySpec) -> Result<ProxyOpResult, ApiError>;
    async fn delete_proxy(&self, name: &str) -> Result<ProxyOpResult, ApiError>;

    // ---- 指标 ----
    async fn metrics(&self) -> Result<MetricsSnapshot, ApiError>;

    // ---- 能力 ----
    fn capabilities(&self) -> Arc<Capabilities>;
    async fn probe(&self) -> Result<Capabilities, ApiError>;
}

/// 便捷：把「能力不支持」转成统一错误。
pub fn unsupported(driver: DriverKind, what: &str, reason: &str) -> ApiError {
    ApiError::DriverUnsupported(format!(
        "驱动 [{}] 不支持{what}：{reason}",
        driver.display_name()
    ))
}
