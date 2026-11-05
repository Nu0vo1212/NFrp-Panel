//! 认证相关端点。

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::audit::{action, Actor};
use crate::auth::{TokenPair, UserInfo};
use crate::db::models::AuditResult;
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

pub async fn login(
    State(st): State<Arc<AppState>>,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    let ip = peer_ip(&peer, &headers);
    let actor = Actor::anonymous(ip.clone());
    let started = std::time::Instant::now();

    if req.username.trim().is_empty() || req.password.is_empty() {
        return Err(ApiError::bad_request("用户名和密码不能为空"));
    }

    match st.auth.login(req.username.trim(), &req.password).await {
        Ok(pair) => {
            st.audit
                .record(
                    &actor,
                    action::AUTH_LOGIN,
                    &req.username,
                    AuditResult::Success,
                    started.elapsed().as_millis() as i64,
                    serde_json::json!({ "role": pair.user.role }),
                )
                .await;
            Ok(Json(pair))
        }
        Err(e) => {
            st.audit
                .record(
                    &actor,
                    action::AUTH_LOGIN_FAILED,
                    &req.username,
                    AuditResult::Failure,
                    started.elapsed().as_millis() as i64,
                    serde_json::json!({ "error": e.to_string() }),
                )
                .await;
            Err(e)
        }
    }
}

pub async fn refresh(
    State(st): State<Arc<AppState>>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<TokenPair>, ApiError> {
    let pair = st.auth.refresh(&req.refresh_token).await?;
    Ok(Json(pair))
}

pub async fn logout(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
) -> Result<(), ApiError> {
    let ip = peer_ip(&peer, &headers);
    st.audit
        .success(&user.actor(ip), action::AUTH_LOGOUT, &user.username, serde_json::json!({}))
        .await;
    // JWT 无状态：前端丢弃 token 即可；服务端只记审计
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    #[serde(flatten)]
    pub user: UserInfo,
    pub permissions: Permissions,
}

#[derive(Debug, Serialize)]
pub struct Permissions {
    pub can_write: bool,
    pub can_manage_ssh: bool,
    pub can_deploy: bool,
    pub can_manage_credentials: bool,
    pub can_read_audit: bool,
}

pub async fn me(State(st): State<Arc<AppState>>, user: AuthUser) -> Result<Json<MeResponse>, ApiError> {
    let claims = crate::auth::Claims {
        sub: user.id.to_string(),
        username: user.username.clone(),
        role: user.role.as_str().to_string(),
        typ: "access".into(),
        exp: 0,
        iat: 0,
    };
    let info = st.auth.me(&claims).await?;
    let admin = user.role.can_write();
    Ok(Json(MeResponse {
        user: info,
        permissions: Permissions {
            can_write: admin,
            can_manage_ssh: admin,
            can_deploy: admin,
            can_manage_credentials: admin,
            can_read_audit: true,
        },
    }))
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: crate::VERSION,
    })
}

/// 修改自己的密码。
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<(), ApiError> {
    if req.new_password.len() < 8 {
        return Err(ApiError::bad_request("新密码至少 8 位"));
    }
    let ip = peer_ip(&peer, &headers);
    // 校验旧密码
    let pair = st
        .auth
        .login(&user.username, &req.current_password)
        .await
        .map_err(|_| ApiError::Unauthorized("当前密码不正确".into()))?;
    let _ = pair;
    let hash = crate::crypto::hash_password(&req.new_password)
        .map_err(ApiError::Internal)?;
    crate::db::repo::set_password(&st.db, user.id, &hash).await?;
    st.audit
        .success(
            &user.actor(ip),
            "auth.change_password",
            &user.username,
            serde_json::json!({}),
        )
        .await;
    Ok(())
}
