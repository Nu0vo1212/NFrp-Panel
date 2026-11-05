//! 配置管理端点。

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::AuditResult;
use crate::domain::*;
use crate::driver::WriteOptions;
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("实例 ID 不合法"))
}

pub async fn get_config(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ConfigDoc>, ApiError> {
    let driver = st.driver(parse_id(&id)?).await?;
    Ok(Json(driver.read_config().await?))
}

#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    pub content: String,
}

pub async fn validate(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidationReport>, ApiError> {
    let driver = st.driver(parse_id(&id)?).await?;
    Ok(Json(driver.validate_config(&req.content).await?))
}

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub content: String,
    #[serde(default)]
    pub apply: Option<bool>,
    #[serde(default)]
    pub auto_restart: Option<bool>,
}

pub async fn put_config(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<WriteRequest>,
) -> Result<Json<ConfigWriteResult>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;
    let driver = st.driver(iid).await?;
    let started = Instant::now();

    // 记录改动规模（脱敏后的行数差），便于审计回溯
    let old = driver.read_config().await.map(|d| d.content).unwrap_or_default();
    let diff = crate::secret::diff_stat(&old, &req.content);

    let opts = WriteOptions {
        apply: req.apply.unwrap_or(true),
        auto_restart: req
            .auto_restart
            .unwrap_or(st.cfg.raw.frp.auto_restart_on_save),
        backup: true,
    };

    match driver.write_config(&req.content, opts).await {
        Ok(result) => {
            st.audit
                .record(
                    &user.actor(ip),
                    action::CONFIG_WRITE,
                    &row.name,
                    AuditResult::Success,
                    started.elapsed().as_millis() as i64,
                    serde_json::json!({
                        "instance_id": iid.to_string(),
                        "applied": result.applied.as_str(),
                        "backup": result.backup.as_ref().map(|b| b.path.clone()),
                        "diff": { "added": diff.added, "removed": diff.removed },
                    }),
                )
                .await;
            Ok(Json(result))
        }
        Err(e) => {
            st.audit
                .record(
                    &user.actor(ip),
                    action::CONFIG_WRITE,
                    &row.name,
                    AuditResult::Failure,
                    started.elapsed().as_millis() as i64,
                    serde_json::json!({ "error": e.to_string() }),
                )
                .await;
            Err(e)
        }
    }
}

pub async fn list_backups(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<BackupInfo>>, ApiError> {
    let driver = st.driver(parse_id(&id)?).await?;
    Ok(Json(driver.list_backups().await?))
}

#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
    pub backup_id: String,
}

pub async fn restore(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<ConfigWriteResult>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;
    let driver = st.driver(iid).await?;
    let started = Instant::now();
    let result = driver.restore_backup(&req.backup_id).await;
    st.audit
        .record(
            &user.actor(ip),
            action::CONFIG_RESTORE,
            &row.name,
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            started.elapsed().as_millis() as i64,
            serde_json::json!({
                "instance_id": iid.to_string(),
                "backup_id": req.backup_id,
            }),
        )
        .await;
    Ok(Json(result?))
}

/// 生成一份"最小可用"的初始配置（供向导与空配置场景使用）。
#[derive(Debug, Deserialize)]
pub struct TemplateQuery {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub flavor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub content: String,
    pub notes: Vec<String>,
}

pub async fn template(Json(q): Json<TemplateQuery>) -> Json<TemplateResponse> {
    let role = q.role.as_deref().unwrap_or("server");
    let flavor = q.flavor.as_deref().unwrap_or("nfrp");
    let (content, notes) = crate::configfile::template_for(role, flavor);
    Json(TemplateResponse { content, notes })
}
