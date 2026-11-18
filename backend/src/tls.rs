//! 统一的出站 HTTPS 客户端构建。
//!
//! 安全基线：**始终校验证书**，不再使用 `danger_accept_invalid_certs(true)`。
//! 访问用户自签证书的场景，通过环境变量 `FRP_PANEL_CA_FILE` 指定额外的
//! CA 证书（PEM），该证书会被加入信任集，系统根证书同时保留。
//!
//! 这样既消除了中间人风险，又保留了对自签 frp Admin API / Prometheus 的支持。

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::ClientBuilder;

pub const CA_FILE_ENV: &str = "FRP_PANEL_CA_FILE";

static EXTRA_CA: OnceLock<Option<reqwest::Certificate>> = OnceLock::new();

/// 载入额外 CA（进程内只做一次）。返回 None 表示未配置或载入失败（失败会打日志）。
fn extra_ca() -> Option<&'static reqwest::Certificate> {
    EXTRA_CA
        .get_or_init(|| {
            let path = std::env::var(CA_FILE_ENV).ok()?;
            let path = path.trim();
            if path.is_empty() {
                return None;
            }
            let p = Path::new(path);
            let pem = std::fs::read(p).ok()?;
            match reqwest::Certificate::from_pem(&pem) {
                Ok(cert) => {
                    tracing::info!(path = %p.display(), "已载入额外信任 CA 证书");
                    Some(cert)
                }
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "额外 CA 证书解析失败，将只信任系统根证书");
                    None
                }
            }
        })
        .as_ref()
}

/// 构建一个安全的管理端点 HTTP 客户端：校验系统 CA，可选追加自定义 CA，**不**关闭校验。
pub fn admin_client_builder(timeout: Duration) -> ClientBuilder {
    let mut b = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_millis(2_000))
        .no_proxy();
    if let Some(cert) = extra_ca() {
        b = b.add_root_certificate(cert.clone());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_builds_without_panic() {
        let _ = admin_client_builder(Duration::from_secs(3)).build().unwrap();
    }
}
