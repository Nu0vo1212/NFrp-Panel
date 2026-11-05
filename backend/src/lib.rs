//! FrpPanel 后端库。
//!
//! 分层：`api`（HTTP 薄层） → `driver`（驱动能力） → `frpapi`/`configfile`/`deploy`（服务）
//! → `db`（存储）。`api` 层不允许直接发 frp 请求，必须经 `frpapi`。

pub mod api;
pub mod audit;
pub mod auth;
pub mod config;
pub mod configfile;
pub mod crypto;
pub mod db;
pub mod deploy;
pub mod domain;
pub mod driver;
pub mod error;
pub mod frpapi;
pub mod metrics;
pub mod secret;
pub mod ssh;
pub mod state;
pub mod systemd;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
