//! 代理 / Visitor 管理端点。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::AuditResult;
use crate::domain::{ProxyInfo, ProxyOpResult, ProxySpec};
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("实例 ID 不合法"))
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ProxyInfo>>, ApiError> {
    let driver = st.driver(parse_id(&id)?).await?;
    Ok(Json(driver.list_proxies().await?))
}

pub async fn create(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(spec): Json<ProxySpec>,
) -> Result<Json<ProxyOpResult>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;
    let driver = st.driver(iid).await?;
    let started = std::time::Instant::now();
    let result = driver.create_proxy(spec.clone()).await;
    st.audit
        .record(
            &user.actor(ip),
            action::PROXY_CREATE,
            &row.name,
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            started.elapsed().as_millis() as i64,
            serde_json::json!({
                "instance_id": iid.to_string(),
                "proxy": spec.name,
                "type": spec.proxy_type,
                "via": result.as_ref().map(|r| r.applied_via.clone()).ok(),
                "error": result.as_ref().err().map(|e| e.to_string()),
            }),
        )
        .await;
    Ok(Json(result?))
}

pub async fn update(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
    Json(spec): Json<ProxySpec>,
) -> Result<Json<ProxyOpResult>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;
    let driver = st.driver(iid).await?;
    let started = std::time::Instant::now();
    let result = driver.update_proxy(&name, spec).await;
    st.audit
        .record(
            &user.actor(ip),
            action::PROXY_UPDATE,
            &row.name,
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            started.elapsed().as_millis() as i64,
            serde_json::json!({
                "instance_id": iid.to_string(),
                "proxy": name,
                "via": result.as_ref().map(|r| r.applied_via.clone()).ok(),
            }),
        )
        .await;
    Ok(Json(result?))
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
) -> Result<Json<ProxyOpResult>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;
    let driver = st.driver(iid).await?;
    let started = std::time::Instant::now();
    let result = driver.delete_proxy(&name).await;
    st.audit
        .record(
            &user.actor(ip),
            action::PROXY_DELETE,
            &row.name,
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            started.elapsed().as_millis() as i64,
            serde_json::json!({
                "instance_id": iid.to_string(),
                "proxy": name,
                "via": result.as_ref().map(|r| r.applied_via.clone()).ok(),
            }),
        )
        .await;
    Ok(Json(result?))
}

/// Visitor 列表：从配置中解析（NFrp 的 visitor 定义在 `[[visitors]]`）。
pub async fn list_visitors(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ProxyInfo>>, ApiError> {
    let iid = parse_id(&id)?;
    let driver = st.driver(iid).await?;
    let doc = driver.read_config().await?;
    let visitors = doc
        .parsed
        .visitors
        .iter()
        .map(|v| {
            let mut info = ProxyInfo::minimal(&v.name, &v.proxy_type);
            info.local_addr = v.effective_local_addr();
            info.status = "config".into();
            info.source = "config".into();
            info.managed_by = "config_file".into();
            info
        })
        .collect::<Vec<_>>();
    Ok(Json(visitors))
}

#[derive(Debug, Deserialize)]
pub struct PreflightProxyRequest {
    pub spec: ProxySpec,
}

#[derive(Debug, serde::Serialize)]
pub struct PreflightProxyResult {
    pub ok: bool,
    pub items: Vec<PreflightItem>,
}

#[derive(Debug, serde::Serialize)]
pub struct PreflightItem {
    pub key: String,
    pub label: String,
    pub status: String,
    pub message: String,
}

/// 代理创建前的预检：字段合法性 + 远端端口是否已被占用（实测服务端会因此硬失败）。
pub async fn preflight(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<PreflightProxyRequest>,
) -> Result<Json<PreflightProxyResult>, ApiError> {
    let iid = parse_id(&id)?;
    let driver = st.driver(iid).await?;
    let mut items = Vec::new();

    // 1) 字段校验
    match req.spec.validate() {
        Ok(_) => items.push(PreflightItem {
            key: "fields".into(),
            label: "字段校验".into(),
            status: "pass".into(),
            message: "必填字段完整".into(),
        }),
        Err(e) => items.push(PreflightItem {
            key: "fields".into(),
            label: "字段校验".into(),
            status: "fail".into(),
            message: e,
        }),
    }

    // 2) 重名
    let existing = driver.list_proxies().await.unwrap_or_default();
    if existing.iter().any(|p| p.name == req.spec.name) {
        items.push(PreflightItem {
            key: "name".into(),
            label: "代理名".into(),
            status: "fail".into(),
            message: format!("代理 [{}] 已存在", req.spec.name),
        });
    } else {
        items.push(PreflightItem {
            key: "name".into(),
            label: "代理名".into(),
            status: "pass".into(),
            message: "名称可用".into(),
        });
    }

    // 3) 远端端口占用（服务端角色才有意义）
    if req.spec.proxy_type == "tcp" || req.spec.proxy_type == "udp" {
        if let Some(port) = req.spec.remote_port {
            let caps = driver.capabilities();
            let occupied = if let Some(c) = admin_client_of(&st, iid).await {
                crate::frpapi::probe::occupied_ports(&c, caps.api_kind).await
            } else {
                vec![]
            };
            if occupied.contains(&port) {
                items.push(PreflightItem {
                    key: "port".into(),
                    label: "远端端口".into(),
                    status: "fail".into(),
                    message: format!("端口 {port} 已被占用（服务端会拒绝该代理）"),
                });
            } else {
                items.push(PreflightItem {
                    key: "port".into(),
                    label: "远端端口".into(),
                    status: "pass".into(),
                    message: format!("端口 {port} 可用"),
                });
            }
        }
    }

    let ok = items.iter().all(|i| i.status != "fail");
    Ok(Json(PreflightProxyResult { ok, items }))
}

/// 从实例记录构造 Admin API 客户端（仅用于预检的只读查询）。
async fn admin_client_of(
    st: &AppState,
    id: Uuid,
) -> Option<crate::frpapi::FrpAdminClient> {
    let row = crate::db::repo::get_instance(&st.db, id).await.ok()?;
    if !row.admin_api_enabled || row.admin_api_base_url.is_empty() {
        return None;
    }
    let pw = st.decrypt_admin_password(&row).unwrap_or_default();
    crate::frpapi::FrpAdminClient::new(
        &row.admin_api_base_url,
        crate::frpapi::AdminAuth {
            user: row.admin_api_user.clone(),
            password: pw,
        },
        row.role,
        std::time::Duration::from_millis(st.cfg.raw.frp.admin_timeout_ms),
        row.capabilities.as_ref().map(|c| c.api_kind),
    )
    .ok()
}
