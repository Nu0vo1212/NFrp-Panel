//! HTTP API 层：只做「参数 → 领域调用 → DTO」。
//!
//! 约束（`docs/01-architecture.md` §2）：
//! - 不直接发 frp 请求，一切经 `state.driver(id)`；
//! - 写操作统一在这里做 RBAC 校验（`admin`），读操作允许 `admin`/`viewer`；
//! - 审计在一次请求的边界处记录（成功/失败/拒绝三种结果）。

pub mod audit;
pub mod config;
pub mod credentials;
pub mod logs;
pub mod metrics;
pub mod routes;
pub mod auth;
pub mod deploy;
pub mod instances;
pub mod proxies;
pub mod settings;
pub mod ssh;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

use crate::audit::Actor;
use crate::db::models::Role;
use crate::error::ApiError;
use crate::state::AppState;

/// 从 `Authorization: Bearer <token>` 解析出的调用者。
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: uuid::Uuid,
    pub username: String,
    pub role: Role,
}

impl AuthUser {
    pub fn actor(&self, ip: impl Into<String>) -> Actor {
        Actor {
            username: self.username.clone(),
            ip: ip.into(),
        }
    }

    /// 写操作要求 admin。
    pub fn require_admin(&self) -> Result<(), ApiError> {
        crate::auth::require_admin(self.role)
    }
}

/// Axum 提取器：校验 JWT 并把用户信息放进 handler。
impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("缺少 Authorization 头".into()))?;

        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or_else(|| ApiError::Unauthorized("Authorization 必须是 Bearer <token>".into()))?
            .trim();

        let claims = state.auth.verify(token, "access")?;
        let id = uuid::Uuid::parse_str(&claims.sub)
            .map_err(|_| ApiError::Unauthorized("token 主体非法".into()))?;
        Ok(AuthUser {
            id,
            username: claims.username,
            role: Role::parse(&claims.role),
        })
    }
}

/// 客户端 IP（用于审计）。
///
/// 用 [`PeerIp`] 提取器拿连接信息：`ConnectInfo` 缺失时会让请求直接失败，
/// 而"拿不到 IP"不该阻断业务（也不该让集成测试无法运行）。
pub fn peer_ip(peer: &PeerIp, headers: &axum::http::HeaderMap) -> String {
    // 优先 X-Forwarded-For 的第一个地址（面板常放在反代后面）
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let t = first.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        return real.trim().to_string();
    }
    peer.0
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// 容忍缺失的对端地址提取器。
#[derive(Debug, Clone, Copy)]
pub struct PeerIp(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(PeerIp(parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0)))
    }
}

/// 通用分页参数。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: Option<i64>,
}

impl PageQuery {
    pub fn resolve(&self) -> (i64, i64) {
        let page = self.page.unwrap_or(1).max(1);
        let size = self.page_size.unwrap_or(20).clamp(1, 200);
        (page, size)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Pagination {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub total_pages: i64,
}

impl Pagination {
    pub fn new(page: i64, page_size: i64, total: i64) -> Self {
        Self {
            page,
            page_size,
            total,
            total_pages: if page_size <= 0 {
                1
            } else {
                ((total as f64) / (page_size as f64)).ceil() as i64
            }
            .max(1),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    pub pagination: Pagination,
}

impl<T> ListResponse<T> {
    pub fn new(items: Vec<T>, page: i64, page_size: i64, total: i64) -> Self {
        Self {
            items,
            pagination: Pagination::new(page, page_size, total),
        }
    }
}
