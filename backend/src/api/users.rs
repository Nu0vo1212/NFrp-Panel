//! 用户管理端点（仅 admin）。
//!
//! 补全 RBAC：viewer 角色从此可以被真正创建和管理。
//! 所有写操作都会递增目标用户 token_version，使其旧令牌立即失效。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::crypto;
use crate::db::models::{Role, UserRow};
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

impl From<&UserRow> for UserSummary {
    fn from(u: &UserRow) -> Self {
        Self {
            id: u.id.to_string(),
            username: u.username.clone(),
            role: u.role.as_str().to_string(),
            created_at: u.created_at.clone(),
            last_login_at: u.last_login_at.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

fn parse_role(s: &str) -> Result<Role, ApiError> {
    match s {
        "admin" => Ok(Role::Admin),
        "viewer" => Ok(Role::Viewer),
        _ => Err(ApiError::bad_request(format!("未知角色：{s}（可选 admin / viewer）"))),
    }
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Json<Vec<UserSummary>>, ApiError> {
    let rows = crate::db::repo::list_users(&st.db).await?;
    Ok(Json(rows.iter().map(UserSummary::from).collect()))
}

pub async fn create(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserSummary>), ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let username = req.username.trim().to_string();
    if username.is_empty() {
        return Err(ApiError::bad_request("用户名不能为空"));
    }
    if req.password.len() < 8 {
        return Err(ApiError::bad_request("密码至少 8 位"));
    }
    let role = parse_role(&req.role)?;
    if crate::db::repo::find_user(&st.db, &username).await?.is_some() {
        return Err(ApiError::conflict("用户名已存在"));
    }
    let hash = crypto::hash_password(&req.password).map_err(ApiError::Internal)?;
    let row = crate::db::repo::insert_user(&st.db, &username, &hash, role).await?;
    st.audit
        .success(
            &user.actor(ip),
            action::USER_CREATE,
            &username,
            serde_json::json!({ "role": role.as_str(), "user_id": row.id.to_string() }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(UserSummary::from(&row))))
}

pub async fn update(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<UserSummary>, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let uid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("用户 ID 不合法"))?;
    let target = crate::db::repo::find_user_by_id(&st.db, uid)
        .await?
        .ok_or_else(|| ApiError::not_found("用户不存在"))?;

    if let Some(role_s) = &req.role {
        let new_role = parse_role(role_s)?;
        // 不允许把最后一个 admin 降权
        if target.role.can_write() && !new_role.can_write() {
            let admins = crate::db::repo::list_users(&st.db)
                .await?
                .iter()
                .filter(|u| u.role.can_write())
                .count();
            if admins <= 1 {
                return Err(ApiError::bad_request("不能降级最后一个管理员"));
            }
        }
        crate::db::repo::update_user_role(&st.db, uid, new_role).await?;
        st.audit
            .success(
                &user.actor(ip.clone()),
                action::USER_ROLE_CHANGE,
                &target.username,
                serde_json::json!({ "from": target.role.as_str(), "to": new_role.as_str() }),
            )
            .await;
    }

    if let Some(pw) = &req.password {
        if pw.len() < 8 {
            return Err(ApiError::bad_request("新密码至少 8 位"));
        }
        let hash = crypto::hash_password(pw).map_err(ApiError::Internal)?;
        crate::db::repo::set_password(&st.db, uid, &hash).await?;
        st.audit
            .success(
                &user.actor(ip.clone()),
                action::USER_PASSWORD_RESET,
                &target.username,
                serde_json::json!({}),
            )
            .await;
    }

    let updated = crate::db::repo::find_user_by_id(&st.db, uid)
        .await?
        .ok_or_else(|| ApiError::not_found("用户不存在"))?;
    Ok(Json(UserSummary::from(&updated)))
}

pub async fn remove(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let uid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("用户 ID 不合法"))?;
    let target = crate::db::repo::find_user_by_id(&st.db, uid)
        .await?
        .ok_or_else(|| ApiError::not_found("用户不存在"))?;

    // 不能删除自己
    if uid == user.id {
        return Err(ApiError::bad_request("不能删除当前登录的账号"));
    }
    // 不能删除最后一个管理员
    if target.role.can_write() {
        let admins = crate::db::repo::list_users(&st.db)
            .await?
            .iter()
            .filter(|u| u.role.can_write())
            .count();
        if admins <= 1 {
            return Err(ApiError::bad_request("不能删除最后一个管理员"));
        }
    }

    crate::db::repo::delete_user(&st.db, uid).await?;
    st.audit
        .success(
            &user.actor(ip),
            action::USER_DELETE,
            &target.username,
            serde_json::json!({ "user_id": uid.to_string() }),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}
