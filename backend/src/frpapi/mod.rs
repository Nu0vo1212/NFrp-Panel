//! frp Admin API 客户端与适配层。
//!
//! 这一层是**兼容性的核心**：NFrp 与官方 frp 的面板 API 在端点、响应结构、
//! 鉴权要求上都不同（见 `docs/04-verified-behavior.md`），此处统一成领域模型。
//!
//! 约束：
//! - 凭据只存在于服务端内存，`Debug` 实现永不打印；
//! - 所有请求带超时；超时/连接失败明确区分「不可达」与「上游报错」；
//! - 不臆造端点：某个实现没有的能力，返回 `None` / `Unsupported`，由上层降级。

pub mod nfrp;
pub mod official;
pub mod probe;

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde_json::Value;
use url::Url;

use crate::domain::{ApiKind, InstanceRole};
use crate::error::ApiError;

/// Admin API 凭据（Basic Auth）。手动实现 Debug 以免泄露。
#[derive(Clone, Default)]
pub struct AdminAuth {
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for AdminAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminAuth")
            .field("user", &self.user)
            .field("password", &if self.password.is_empty() { "" } else { "‹已设置›" })
            .finish()
    }
}

impl AdminAuth {
    pub fn is_empty(&self) -> bool {
        self.user.is_empty() && self.password.is_empty()
    }
}

/// NFrp 客户端写操作要求的防 CSRF 头（实测缺失会 403）。
pub const NFRP_CLIENT_HEADER: &str = "X-Nfrp-Client";

#[derive(Clone)]
pub struct FrpAdminClient {
    base: Url,
    auth: AdminAuth,
    http: Client,
    /// 目标是客户端时，写操作要带 `X-Nfrp-Client: 1`。
    is_client: bool,
    /// 若已知实现类型，则跳过部分试探。
    hint: Option<ApiKind>,
}

impl std::fmt::Debug for FrpAdminClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrpAdminClient")
            .field("base", &self.base.as_str())
            .field("auth", &self.auth)
            .field("is_client", &self.is_client)
            .finish()
    }
}

/// 一次原始 HTTP 调用的结果，供适配层与探测共同使用。
#[derive(Debug)]
pub struct RawResponse {
    pub status: StatusCode,
    pub body: String,
}

impl RawResponse {
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }
    pub fn json(&self) -> Result<Value, ApiError> {
        serde_json::from_str(&self.body).map_err(|_| {
            ApiError::upstream(format!(
                "Admin API 返回的不是 JSON（HTTP {}）：{}",
                self.status.as_u16(),
                truncate(&self.body, 200)
            ))
        })
    }
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}

impl FrpAdminClient {
    pub fn new(
        base_url: &str,
        auth: AdminAuth,
        role: InstanceRole,
        timeout: Duration,
        hint: Option<ApiKind>,
    ) -> Result<Self, ApiError> {
        let base = normalize_base(base_url)?;
        let http = Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_millis(2_000))
            // 面板元数据查询不应该走系统代理
            .no_proxy()
            .danger_accept_invalid_certs(true) // frp 面板常用自签名证书
            .build()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("创建 HTTP 客户端失败：{e}")))?;
        Ok(Self {
            base,
            auth,
            http,
            is_client: matches!(role, InstanceRole::Client),
            hint,
        })
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str()
    }

    pub fn hint(&self) -> Option<ApiKind> {
        self.hint
    }

    fn url(&self, path: &str) -> Result<Url, ApiError> {
        self.base.join(path.trim_start_matches('/')).map_err(|e| {
            ApiError::bad_request(format!("Admin API 地址不合法：{e}"))
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder, ApiError> {
        let url = self.url(path)?;
        let mut rb = self.http.request(method, url);
        if !self.auth.is_empty() {
            rb = rb.basic_auth(&self.auth.user, Some(&self.auth.password));
        }
        Ok(rb)
    }

    /// GET（读操作）。连接失败 → `upstream`，且消息里点明"不可达"。
    pub async fn get(&self, path: &str) -> Result<RawResponse, ApiError> {
        let rb = self.request(reqwest::Method::GET, path)?;
        self.send(rb).await
    }

    /// POST JSON（写操作，NFrp 客户端自动附加防护头）。
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<RawResponse, ApiError> {
        let mut rb = self.request(reqwest::Method::POST, path)?.json(body);
        if self.is_client {
            rb = rb.header(NFRP_CLIENT_HEADER, "1");
        }
        self.send(rb).await
    }

    /// PUT / DELETE（通用）。
    pub async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<RawResponse, ApiError> {
        let mut rb = self.request(method, path)?;
        if self.is_client {
            rb = rb.header(NFRP_CLIENT_HEADER, "1");
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        self.send(rb).await
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<RawResponse, ApiError> {
        match rb.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Ok(RawResponse { status, body })
            }
            Err(e) => {
                let msg = if e.is_timeout() {
                    format!(
                        "连接 frp Admin API 超时（{}）：请确认实例已启动且面板端口可达",
                        self.base
                    )
                } else if e.is_connect() {
                    format!("无法连接 frp Admin API（{}）：连接被拒绝或端口未监听", self.base)
                } else {
                    format!("请求 frp Admin API 失败（{}）：{e}", self.base)
                };
                Err(ApiError::upstream(msg))
            }
        }
    }

    // -----------------------------------------------------------------------
    // 高层封装：按实现分派
    // -----------------------------------------------------------------------

    /// 拉取统一状态。`api_kind` 决定走哪套解析。
    pub async fn status(
        &self,
        api_kind: ApiKind,
        role: InstanceRole,
    ) -> Result<crate::domain::InstanceStatus, ApiError> {
        match (api_kind, role) {
            (ApiKind::Nfrp, InstanceRole::Server) => nfrp::server_status(self).await,
            (ApiKind::Nfrp, InstanceRole::Client) => nfrp::client_status(self).await,
            (ApiKind::Official, InstanceRole::Server) => official::server_status(self).await,
            (ApiKind::Official, InstanceRole::Client) => official::client_status(self).await,
            _ => {
                // 未知实现：先试 NFrp，再试官方
                match nfrp::server_status(self).await {
                    Ok(s) if s.api_reachable => Ok(s),
                    _ => match role {
                        InstanceRole::Server => official::server_status(self).await,
                        InstanceRole::Client => official::client_status(self).await,
                    },
                }
            }
        }
    }

    /// 拉取代理列表。
    pub async fn list_proxies(
        &self,
        api_kind: ApiKind,
        role: InstanceRole,
    ) -> Result<Vec<crate::domain::ProxyInfo>, ApiError> {
        match (api_kind, role) {
            (ApiKind::Nfrp, InstanceRole::Server) => nfrp::server_proxies(self).await,
            (ApiKind::Nfrp, InstanceRole::Client) => nfrp::client_proxies(self).await,
            (ApiKind::Official, InstanceRole::Server) => official::server_proxies(self).await,
            (ApiKind::Official, InstanceRole::Client) => official::client_proxies(self).await,
            _ => match role {
                InstanceRole::Server => match nfrp::server_proxies(self).await {
                    Ok(v) => Ok(v),
                    Err(_) => official::server_proxies(self).await,
                },
                InstanceRole::Client => match nfrp::client_proxies(self).await {
                    Ok(v) => Ok(v),
                    Err(_) => official::client_proxies(self).await,
                },
            },
        }
    }

    /// 健康检查：只要能连上任意一个已知端点即视为可达。
    pub async fn health(&self, api_kind: ApiKind) -> Result<bool, ApiError> {
        let paths: &[&str] = match api_kind {
            ApiKind::Nfrp => &["/api/healthz", "/api/v2/status", "/api/status"],
            ApiKind::Official => &["/api/v2/clients?all=1", "/api/status"],
            _ => &["/api/healthz", "/api/status", "/api/v2/status"],
        };
        for p in paths {
            if let Ok(r) = self.get(p).await {
                if r.is_success() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// 轻量实现识别（最多 2~4 个端点），用于"每次判断该走哪条通道"的场景。
    ///
    /// 与 [`probe::probe`] 的区别：这里不做能力导出，只回答"这是谁"。
    pub async fn detect_api_kind(&self) -> Result<ApiKind, ApiError> {
        // NFrp 服务端有 /api/v2/version
        // ★ 连接失败（端口没监听）直接返回 Unknown：后面几个端点必然也连不上，
        //   顺序试探只会把列表/详情多拖几个连接超时。
        match self.get("/api/v2/version").await {
            Ok(r) if r.is_success() => return Ok(ApiKind::Nfrp),
            Err(_) => return Ok(ApiKind::Unknown),
            _ => {}
        }
        // 再看 /api/status 的结构（NFrp 客户端有 run_id/wire_protocol；NFrp 服务端有 clients[]）
        match self.get("/api/status").await {
            Ok(r) if r.is_success() => {
                let b = &r.body;
                if b.contains("\"run_id\"")
                    || b.contains("\"wire_protocol\"")
                    || b.contains("\"clients\"")
                {
                    return Ok(ApiKind::Nfrp);
                }
                return Ok(ApiKind::Official);
            }
            Err(_) => return Ok(ApiKind::Unknown),
            _ => {}
        }
        // 退到官方 v2 信封
        if let Ok(r) = self.get("/api/v2/clients?all=1").await {
            if r.is_success() {
                return Ok(ApiKind::Official);
            }
        }
        if let Ok(r) = self.get("/api/healthz").await {
            if r.is_success() {
                return Ok(ApiKind::Nfrp);
            }
        }
        Ok(ApiKind::Unknown)
    }
}

/// 规范化：允许用户填 `host:7500`、`http://host:7500`、带或不带尾部斜杠。
pub fn normalize_base(input: &str) -> Result<Url, ApiError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("Admin API 地址不能为空"));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let mut url = Url::parse(&with_scheme)
        .map_err(|e| ApiError::bad_request(format!("Admin API 地址不合法：{e}")))?;
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path().trim_end_matches('/'));
        url.set_path(&p);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_scheme_and_slash() {
        assert_eq!(
            normalize_base("127.0.0.1:7500").unwrap().as_str(),
            "http://127.0.0.1:7500/"
        );
        assert_eq!(
            normalize_base("http://h:1/api").unwrap().as_str(),
            "http://h:1/api/"
        );
        assert!(normalize_base("   ").is_err());
    }

    #[test]
    fn auth_debug_never_leaks_password() {
        let a = AdminAuth {
            user: "admin".into(),
            password: "hunter2".into(),
        };
        let s = format!("{a:?}");
        assert!(!s.contains("hunter2"));
        assert!(s.contains("admin"));
    }

    #[test]
    fn client_debug_never_leaks_password() {
        let c = FrpAdminClient::new(
            "http://127.0.0.1:7500",
            AdminAuth {
                user: "u".into(),
                password: "p4ss".into(),
            },
            InstanceRole::Server,
            Duration::from_secs(1),
            None,
        )
        .unwrap();
        let s = format!("{c:?}");
        assert!(!s.contains("p4ss"));
    }
}
