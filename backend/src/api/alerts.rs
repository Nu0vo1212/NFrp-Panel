//! 告警通知端点：webhook 配置 + 指标阈值规则（仅 admin 写）。
//!
//! 主动通知由 `crate::alerts` 模块在实例状态变化 / 部署失败 / 指标越界时触发。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::{AlertRuleRow, AlertWebhookRow};
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

#[derive(Debug, Serialize)]
pub struct WebhookView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub events: Vec<String>,
    pub created_at: String,
}

impl From<&AlertWebhookRow> for WebhookView {
    fn from(w: &AlertWebhookRow) -> Self {
        Self {
            id: w.id.to_string(),
            name: w.name.clone(),
            url: w.url.clone(),
            enabled: w.enabled,
            events: w.events.clone(),
            created_at: w.created_at.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RuleView {
    pub id: String,
    pub instance_id: String,
    pub metric: String,
    pub op: String,
    pub threshold: f64,
    pub window_min: i64,
    pub enabled: bool,
    pub created_at: String,
}

impl From<&AlertRuleRow> for RuleView {
    fn from(r: &AlertRuleRow) -> Self {
        Self {
            id: r.id.to_string(),
            instance_id: r.instance_id.to_string(),
            metric: r.metric.clone(),
            op: r.op.clone(),
            threshold: r.threshold,
            window_min: r.window_min,
            enabled: r.enabled,
            created_at: r.created_at.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateRuleRequest {
    pub instance_id: String,
    pub metric: String,
    pub op: String,
    pub threshold: f64,
    #[serde(default = "default_window")]
    pub window_min: i64,
}

fn default_window() -> i64 {
    5
}

// ---- webhooks ----

pub async fn list_webhooks(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Json<Vec<WebhookView>>, ApiError> {
    let rows = crate::db::repo::list_webhooks(&st.db).await?;
    Ok(Json(rows.iter().map(WebhookView::from).collect()))
}

pub async fn create_webhook(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<(StatusCode, Json<WebhookView>), ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let name = req.name.trim().to_string();
    let url = req.url.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("名称不能为空"));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ApiError::bad_request("Webhook 地址必须是 http:// 或 https:// 开头"));
    }
    if req.events.is_empty() {
        return Err(ApiError::bad_request("至少选择一个事件"));
    }
    let row = crate::db::repo::insert_webhook(&st.db, &name, &url, &req.events).await?;
    st.audit.success(&user.actor(ip), action::ALERT_WEBHOOK_CREATE, &name, serde_json::json!({})).await;
    Ok((StatusCode::CREATED, Json(WebhookView::from(&row))))
}

pub async fn delete_webhook(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let wid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("Webhook ID 不合法"))?;
    crate::db::repo::delete_webhook(&st.db, wid).await?;
    st.audit.success(&user.actor(ip), action::ALERT_WEBHOOK_DELETE, &id, serde_json::json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test_webhook(
    State(_st): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    user.require_admin()?;
    let ok = crate::alerts::send_test(&req.url.trim()).await;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

// ---- metric alert rules ----

pub async fn list_rules(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Json<Vec<RuleView>>, ApiError> {
    let rows = crate::db::repo::list_alert_rules(&st.db, None).await?;
    Ok(Json(rows.iter().map(RuleView::from).collect()))
}

pub async fn create_rule(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<CreateRuleRequest>,
) -> Result<(StatusCode, Json<RuleView>), ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let iid = Uuid::parse_str(&req.instance_id).map_err(|_| ApiError::bad_request("实例 ID 不合法"))?;
    if crate::db::repo::get_instance(&st.db, iid).await.is_err() {
        return Err(ApiError::not_found("实例不存在"));
    }
    if !matches!(req.metric.as_str(), "bytes_in" | "bytes_out" | "conns" | "clients" | "proxies") {
        return Err(ApiError::bad_request("不支持的指标"));
    }
    if !matches!(req.op.as_str(), "gt" | "gte" | "lt" | "lte") {
        return Err(ApiError::bad_request("不支持的比较符"));
    }
    let row = crate::db::repo::insert_alert_rule(
        &st.db,
        iid,
        &req.metric,
        &req.op,
        req.threshold,
        req.window_min.max(1),
    )
    .await?;
    st.audit.success(&user.actor(ip), action::ALERT_RULE_CREATE, &req.instance_id, serde_json::json!({})).await;
    Ok((StatusCode::CREATED, Json(RuleView::from(&row))))
}

pub async fn delete_rule(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let rid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("规则 ID 不合法"))?;
    crate::db::repo::delete_alert_rule(&st.db, rid).await?;
    st.audit.success(&user.actor(ip), action::ALERT_RULE_DELETE, &id, serde_json::json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}
