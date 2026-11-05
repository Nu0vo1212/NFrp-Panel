//! 实例管理端点。

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::{AuditResult, InstanceRow};
use crate::db::repo::{self, InstanceUpdate};
use crate::domain::*;
use crate::error::ApiError;
use crate::state::AppState;
use crate::frpapi::FrpAdminClient;

use super::{peer_ip, AuthUser, PeerIp};

// ---------------------------------------------------------------------------
// DTO
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct InstanceSummary {
    pub id: String,
    pub name: String,
    pub role: String,
    pub driver: String,
    pub driver_label: String,
    pub flavor: String,
    pub flavor_label: String,
    pub connection_kind: String,
    pub host: String,
    pub remote_ident: String,
    pub binary_version: Option<String>,
    pub deployment_status: String,
    pub service_mode: String,
    pub enabled: bool,
    pub live: LiveStatus,
    pub capabilities: Capabilities,
    pub last_deployed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct LiveStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub healthy: bool,
    pub since: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InstanceDetail {
    #[serde(flatten)]
    pub summary: InstanceSummary,
    pub paths: InstancePaths,
    pub admin_api: AdminApiView,
    pub connection: ConnectionView,
    pub capabilities: Capabilities,
    pub deployment: DeploymentInfo,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AdminApiView {
    pub base_url: String,
    pub user: String,
    pub enabled: bool,
    pub password_set: bool,
    /// 永远只回显掩码，绝不回显明文
    pub password_masked: String,
    pub prometheus_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConnectionView {
    pub kind: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth_kind: Option<String>,
    pub credential_id: Option<String>,
    pub sudo: Option<String>,
    pub host_key_fingerprint: Option<String>,
    pub host_key_trusted: bool,
}

#[derive(Debug, Serialize)]
pub struct DeploymentInfo {
    pub status: String,
    pub binary_version: Option<String>,
    pub last_deployed_at: Option<String>,
    pub service_mode: String,
    pub service_unit: Option<String>,
    pub last_task: Option<serde_json::Value>,
}

/// 新建实例入参。
#[derive(Debug, Deserialize)]
pub struct InstanceCreateRequest {
    pub name: String,
    pub role: String,
    pub driver: String,
    pub flavor: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub connection: Option<ConnectionInput>,
    pub paths: PathsInput,
    #[serde(default)]
    pub service_mode: Option<String>,
    #[serde(default)]
    pub service_unit: Option<String>,
    #[serde(default)]
    pub admin_api: Option<AdminApiInput>,
    #[serde(default)]
    pub binary_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionInput {
    Local,
    Ssh {
        host: String,
        port: u16,
        username: String,
        auth_kind: String,
        #[serde(default)]
        credential_id: Option<String>,
        #[serde(default)]
        sudo: Option<String>,
        #[serde(default)]
        sudo_cmd: Option<String>,
        #[serde(default)]
        host_key_fingerprint: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsInput {
    pub binary: String,
    pub config: String,
    pub log: String,
    #[serde(default)]
    pub work_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminApiInput {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub user: String,
    /// 明文密码只在创建/更新时传入一次；之后只能读取掩码
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub prometheus_url: Option<String>,
}

/// 更新实例入参（全部可选）。
#[derive(Debug, Deserialize)]
pub struct InstanceUpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub flavor: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub connection: Option<ConnectionInput>,
    #[serde(default)]
    pub paths: Option<PathsInput>,
    #[serde(default)]
    pub service_mode: Option<String>,
    #[serde(default)]
    pub service_unit: Option<String>,
    #[serde(default)]
    pub admin_api: Option<AdminApiInput>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: Option<i64>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

// ---------------------------------------------------------------------------
// 组装视图
// ---------------------------------------------------------------------------

impl InstanceSummary {
    pub fn from_row(row: &InstanceRow, live: LiveStatus) -> Self {
        let caps = row
            .capabilities
            .clone()
            .unwrap_or_else(|| Capabilities::unknown(row.flavor));
        Self {
            id: row.id.to_string(),
            name: row.name.clone(),
            role: row.role.as_str().to_string(),
            driver: row.driver.as_str().to_string(),
            driver_label: row.driver.display_name().to_string(),
            flavor: row.flavor.as_str().to_string(),
            flavor_label: row.flavor.display_name().to_string(),
            connection_kind: if row.connection.is_remote() {
                "ssh".into()
            } else {
                "local".into()
            },
            host: row.connection.host_label(),
            remote_ident: match &row.connection {
                Connection::Local => row.paths.binary.clone(),
                Connection::Ssh { .. } => format!("{}:{}", row.connection.host_label(), row.paths.binary),
            },
            binary_version: row.binary_version.clone(),
            deployment_status: row.deployment_status.as_str().to_string(),
            service_mode: row.service_mode.as_str().to_string(),
            enabled: row.enabled,
            live,
            capabilities: caps,
            last_deployed_at: row.last_deployed_at.clone(),
            updated_at: row.updated_at.clone(),
        }
    }
}

fn offline_live(reason: Option<String>) -> LiveStatus {
    LiveStatus {
        state: "stopped".into(),
        pid: None,
        healthy: false,
        since: None,
        error: reason,
    }
}

async fn build_detail(st: &AppState, row: &InstanceRow) -> Result<InstanceDetail, ApiError> {
    // 状态查询失败不影响列表可用性：降级为 stopped + error 说明
    let live = match st.driver(row.id).await {
        Ok(d) => match d.status().await {
            Ok(s) => LiveStatus {
                state: s.state.as_str().to_string(),
                pid: s.pid,
                healthy: s.healthy,
                since: None,
                error: s.last_error,
            },
            Err(e) => offline_live(Some(e.to_string())),
        },
        Err(e) => offline_live(Some(e.to_string())),
    };

    let caps = row
        .capabilities
        .clone()
        .unwrap_or_else(|| Capabilities::unknown(row.flavor));

    let connection = match &row.connection {
        Connection::Local => ConnectionView {
            kind: "local".into(),
            host: None,
            port: None,
            username: None,
            auth_kind: None,
            credential_id: None,
            sudo: None,
            host_key_fingerprint: None,
            host_key_trusted: true,
        },
        Connection::Ssh {
            host,
            port,
            username,
            auth_kind,
            credential_id,
            sudo,
            host_key_fingerprint,
            ..
        } => {
            // 判断该主机是否有受信任指纹
            let trusted = repo::list_host_keys(&st.db)
                .await
                .map(|ks| {
                    ks.iter()
                        .any(|k| &k.host == host && k.port == *port)
                })
                .unwrap_or(false);
            ConnectionView {
                kind: "ssh".into(),
                host: Some(host.clone()),
                port: Some(*port),
                username: Some(username.clone()),
                auth_kind: Some(auth_kind.as_str().to_string()),
                credential_id: credential_id.map(|c| c.to_string()),
                sudo: Some(sudo.as_str().to_string()),
                host_key_fingerprint: host_key_fingerprint.clone(),
                host_key_trusted: trusted,
            }
        }
    };

    let last_task = repo::latest_deploy_task(&st.db, row.id)
        .await
        .ok()
        .flatten()
        .map(|t| {
            serde_json::json!({
                "id": t.id.to_string(),
                "kind": t.kind,
                "state": t.state,
                "started_at": t.started_at,
                "finished_at": t.finished_at,
                "error": t.error,
            })
        });

    let summary = InstanceSummary::from_row(row, live);

    Ok(InstanceDetail {
        deployment: DeploymentInfo {
            status: row.deployment_status.as_str().to_string(),
            binary_version: row.binary_version.clone(),
            last_deployed_at: row.last_deployed_at.clone(),
            service_mode: row.service_mode.as_str().to_string(),
            service_unit: row.service_unit.clone(),
            last_task,
        },
        paths: row.paths.clone(),
        admin_api: AdminApiView {
            base_url: row.admin_api_base_url.clone(),
            user: row.admin_api_user.clone(),
            enabled: row.admin_api_enabled,
            password_set: row.admin_api_password_enc.is_some(),
            password_masked: if row.admin_api_password_enc.is_some() {
                "••••••••".into()
            } else {
                String::new()
            },
            prometheus_url: row.admin_api_prometheus_url.clone(),
        },
        capabilities: caps,
        connection,
        tags: row.tags.clone(),
        summary,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn list(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<super::ListResponse<InstanceSummary>>, ApiError> {
    let rows = repo::list_instances(&st.db).await?;

    let needle = q.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty());
    let rows: Vec<InstanceRow> = rows
        .into_iter()
        .filter(|r| {
            if let Some(n) = &needle {
                if !r.name.to_lowercase().contains(n) {
                    return false;
                }
            }
            if let Some(d) = &q.driver {
                if !d.is_empty() && r.driver.as_str() != d {
                    return false;
                }
            }
            if let Some(role) = &q.role {
                if !role.is_empty() && r.role.as_str() != role {
                    return false;
                }
            }
            true
        })
        .collect();

    let total = rows.len() as i64;
    let (page, page_size) = super::PageQuery {
        page: q.page,
        page_size: q.page_size,
    }
    .resolve();
    let start = ((page - 1) * page_size).max(0) as usize;
    let page_rows: Vec<&InstanceRow> = rows
        .iter()
        .skip(start)
        .take(page_size as usize)
        .collect();

    // ★ 列表只读本地库，立即返回；实时状态由前端逐个查 /status 后回填。
    //   SSH 实例的单次状态查询可能要几秒，放在列表请求里会让整页一起等。
    let items: Vec<InstanceSummary> = page_rows
        .iter()
        .map(|row| {
            InstanceSummary::from_row(
                row,
                LiveStatus {
                    state: "unknown".into(),
                    pid: None,
                    healthy: false,
                    since: None,
                    error: None,
                },
            )
        })
        .collect();

    Ok(Json(super::ListResponse::new(items, page, page_size, total)))
}
pub async fn create(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<InstanceCreateRequest>,
) -> Result<(axum::http::StatusCode, Json<InstanceDetail>), ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let started = Instant::now();

    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("实例名称不能为空"));
    }
    let role = InstanceRole::parse(&req.role)
        .ok_or_else(|| ApiError::bad_request(format!("未知角色：{}", req.role)))?;
    let driver = DriverKind::parse(&req.driver)
        .ok_or_else(|| ApiError::bad_request(format!("未知驱动：{}", req.driver)))?;
    let flavor = BinaryFlavor::parse(&req.flavor)
        .ok_or_else(|| ApiError::bad_request(format!("未知二进制类型：{}", req.flavor)))?;

    let connection = build_connection(req.connection, &st, driver).await?;
    // 驱动与连接方式必须自洽
    match (driver, &connection) {
        (DriverKind::Binary, c) if c.is_remote() => {
            return Err(ApiError::bad_request(
                "本机二进制驱动不能配 SSH 连接方式，请改选「SSH 远程二进制」驱动",
            ))
        }
        (DriverKind::SshBinary, c) if !c.is_remote() => {
            return Err(ApiError::bad_request(
                "SSH 远程二进制驱动需要 SSH 连接信息",
            ))
        }
        _ => {}
    }

    let paths = InstancePaths {
        binary: req.paths.binary.trim().to_string(),
        config: req.paths.config.trim().to_string(),
        log: req.paths.log.trim().to_string(),
        work_dir: req.paths.work_dir.trim().to_string(),
    };
    // 路径校验按驱动区分：
    // * Binary / SshBinary 接管进程与配置文件 → 路径必填；
    // * AdminApi 不拥有这两者 → 允许为空（首次加载也不应因此拒绝）；
    // * Embedded 暂未实现 → 允许为空，运行时会明确报"未实现"。
    let needs_paths = matches!(driver, DriverKind::Binary | DriverKind::SshBinary);
    if needs_paths && (paths.binary.is_empty() || paths.config.is_empty()) {
        return Err(ApiError::bad_request(
            "该驱动需要二进制路径与配置路径（Admin API 驱动可留空）",
        ));
    }
    if driver == DriverKind::AdminApi && req.admin_api.as_ref().is_none() {
        return Err(ApiError::bad_request(
            "Admin API 驱动必须提供 admin_api 配置（地址与凭据）",
        ));
    }

    let service_mode = req
        .service_mode
        .as_deref()
        .and_then(ServiceMode::parse)
        .unwrap_or(if connection.is_remote() {
            ServiceMode::Systemd
        } else {
            ServiceMode::Process
        });

    let (admin_base, admin_user, prom_url) = validate_admin_api(req.admin_api.as_ref())?;
    let admin_pw_plain = req
        .admin_api
        .as_ref()
        .and_then(|a| a.password.clone())
        .filter(|p| !p.is_empty());

    let id = repo::insert_instance(
        &st.db,
        &name,
        role,
        driver,
        flavor,
        &req.tags,
        &connection,
        &paths,
        service_mode,
        req.service_unit.as_deref(),
        &admin_base,
        &admin_user,
        None,
        prom_url.as_deref(),
    )
    .await?;

    // ★ 密码必须用实例 ID 做 AAD 加密，所以只能在拿到 ID 之后回写。
    //   任何一步失败都把实例删掉，避免留下"半成品"。
    if let Some(pw) = admin_pw_plain {
        let enc = st
            .keys
            .encrypt(&pw, &format!("instance:{id}:admin_password"))
            .map_err(|e| {
                let _ = ();
                e
            })?;
        if let Err(e) = repo::update_instance(
            &st.db,
            id,
            InstanceUpdate {
                admin_api_password_enc: Some(Some(enc)),
                ..Default::default()
            },
        )
        .await
        {
            let _ = repo::delete_instance(&st.db, id).await;
            return Err(e);
        }
    }

    st.audit
        .record(
            &user.actor(ip),
            action::INSTANCE_CREATE,
            &name,
            AuditResult::Success,
            started.elapsed().as_millis() as i64,
            serde_json::json!({
                "instance_id": id.to_string(),
                "driver": driver.as_str(),
                "flavor": flavor.as_str(),
                "role": role.as_str(),
            }),
        )
        .await;

    let row = repo::get_instance(&st.db, id).await?;
    let detail = build_detail(&st, &row).await?;
    Ok((axum::http::StatusCode::CREATED, Json(detail)))
}

pub async fn get(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<InstanceDetail>, ApiError> {
    let id = parse_id(&id)?;
    let row = repo::get_instance(&st.db, id).await?;
    Ok(Json(build_detail(&st, &row).await?))
}

pub async fn update(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<InstanceUpdateRequest>,
) -> Result<Json<InstanceDetail>, ApiError> {
    user.require_admin()?;
    let id = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let started = Instant::now();
    let row = repo::get_instance(&st.db, id).await?;

    let mut up = InstanceUpdate::default();
    if let Some(n) = req.name {
        if n.trim().is_empty() {
            return Err(ApiError::bad_request("实例名称不能为空"));
        }
        up.name = Some(n.trim().to_string());
    }
    if let Some(r) = req.role {
        up.role = Some(
            InstanceRole::parse(&r)
                .ok_or_else(|| ApiError::bad_request(format!("未知角色：{r}")))?,
        );
    }
    if let Some(d) = req.driver {
        up.driver = Some(
            DriverKind::parse(&d)
                .ok_or_else(|| ApiError::bad_request(format!("未知驱动：{d}")))?,
        );
    }
    if let Some(f) = req.flavor {
        up.flavor = Some(
            BinaryFlavor::parse(&f)
                .ok_or_else(|| ApiError::bad_request(format!("未知二进制类型：{f}")))?,
        );
    }
    if let Some(e) = req.enabled {
        up.enabled = Some(e);
    }
    if let Some(t) = req.tags {
        up.tags = Some(t);
    }
    if let Some(c) = req.connection {
        up.connection = Some(build_connection(Some(c), &st, row.driver).await?);
    }
    if let Some(p) = req.paths {
        up.paths = Some(InstancePaths {
            binary: p.binary.trim().to_string(),
            config: p.config.trim().to_string(),
            log: p.log.trim().to_string(),
            work_dir: p.work_dir.trim().to_string(),
        });
    }
    if let Some(m) = req.service_mode {
        up.service_mode = Some(
            ServiceMode::parse(&m)
                .ok_or_else(|| ApiError::bad_request(format!("未知服务托管方式：{m}")))?,
        );
    }
    if let Some(u) = req.service_unit {
        up.service_unit = Some(if u.trim().is_empty() { None } else { Some(u) });
    }
    if let Some(api) = req.admin_api {
        if !api.base_url.trim().is_empty() {
            up.admin_api_base_url = Some(api.base_url.clone());
        }
        up.admin_api_user = Some(api.user.clone());
        if let Some(pw) = api.password {
            up.admin_api_password_enc = Some(if pw.is_empty() {
                None
            } else {
                Some(st.keys.encrypt(
                    &pw,
                    &format!("instance:{id}:admin_password"),
                )?)
            });
        }
        if let Some(en) = api.enabled {
            up.admin_api_enabled = Some(en);
        }
        if let Some(p) = api.prometheus_url {
            up.admin_api_prometheus_url = Some(if p.trim().is_empty() { None } else { Some(p) });
        }
    }

    repo::update_instance(&st.db, id, up).await?;
    // 配置变了必须重建驱动，否则旧驱动还拿着旧凭据/旧地址
    st.drivers.invalidate(id).await;

    st.audit
        .record(
            &user.actor(ip),
            action::INSTANCE_UPDATE,
            &row.name,
            AuditResult::Success,
            started.elapsed().as_millis() as i64,
            serde_json::json!({ "instance_id": id.to_string() }),
        )
        .await;

    let row = repo::get_instance(&st.db, id).await?;
    Ok(Json(build_detail(&st, &row).await?))
}

#[derive(Debug, Deserialize, Default)]
pub struct DeleteQuery {
    #[serde(default)]
    pub force: Option<bool>,
    /// 是否保留配置/日志文件（默认保留，避免误删用户数据）
    #[serde(default)]
    pub keep_files: Option<bool>,
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> Result<axum::http::StatusCode, ApiError> {
    user.require_admin()?;
    let id = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = repo::get_instance(&st.db, id).await?;

    // 运行中的实例默认不允许直接删除
    if !q.force.unwrap_or(false) {
        if let Ok(d) = st.driver(id).await {
            if let Ok(s) = d.status().await {
                if s.state == LiveState::Running && row.driver == DriverKind::Binary {
                    return Err(ApiError::conflict(
                        "实例正在运行：请先停止，或带 ?force=true 强制删除",
                    ));
                }
            }
        }
    }

    repo::delete_instance(&st.db, id).await?;
    st.drivers.invalidate(id).await;

    st.audit
        .success(
            &user.actor(ip),
            action::INSTANCE_DELETE,
            &row.name,
            serde_json::json!({
                "instance_id": id.to_string(),
                "keep_files": q.keep_files.unwrap_or(true),
            }),
        )
        .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// 生命周期
// ---------------------------------------------------------------------------

async fn lifecycle(
    st: &Arc<AppState>,
    user: &AuthUser,
    ip: String,
    id: Uuid,
    act: &str,
) -> Result<Json<InstanceStatus>, ApiError> {
    user.require_admin()?;
    let row = repo::get_instance(&st.db, id).await?;
    let driver = st.driver(id).await?;
    let started = Instant::now();

    let result = match act {
        "start" => driver.start().await,
        "stop" => driver.stop(crate::driver::StopOptions::default()).await,
        "restart" => driver.restart().await,
        other => return Err(ApiError::bad_request(format!("未知操作：{other}"))),
    };

    let action_name = match act {
        "start" => action::INSTANCE_START,
        "stop" => action::INSTANCE_STOP,
        _ => action::INSTANCE_RESTART,
    };

    match result {
        Ok(status) => {
            // ★ 启动成功后立刻探测一次并落库。
            //   否则列表页读不到版本与能力（记录里的 binary_version/capabilities
            //   只会在显式探测时才写入），前端会显示空版本与"未知"。
            //   停止后没必要探测：此时 Admin API 必然不可达，探测会串行等一串连接超时，
            //   让"停止"按钮卡住十几秒。
            if status.state == LiveState::Running {
                let _ = driver.probe().await;
            }

            repo::update_instance(
                &st.db,
                id,
                InstanceUpdate {
                    last_error: Some(None),
                    ..Default::default()
                },
            )
            .await?;
            st.audit
                .record(
                    &user.actor(ip),
                    action_name,
                    &row.name,
                    AuditResult::Success,
                    started.elapsed().as_millis() as i64,
                    serde_json::json!({ "state": status.state.as_str(), "pid": status.pid }),
                )
                .await;
            Ok(Json(status))
        }
        Err(e) => {
            st.audit
                .record(
                    &user.actor(ip),
                    action_name,
                    &row.name,
                    AuditResult::Failure,
                    started.elapsed().as_millis() as i64,
                    serde_json::json!({ "error": e.to_string() }),
                )
                .await;
            let _ = repo::update_instance(
                &st.db,
                id,
                InstanceUpdate {
                    last_error: Some(Some(e.to_string())),
                    ..Default::default()
                },
            )
            .await;
            Err(e)
        }
    }
}

pub async fn start(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<InstanceStatus>, ApiError> {
    let ip = peer_ip(&peer, &headers);
    lifecycle(&st, &user, ip, parse_id(&id)?, "start").await
}

pub async fn stop(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<InstanceStatus>, ApiError> {
    let ip = peer_ip(&peer, &headers);
    lifecycle(&st, &user, ip, parse_id(&id)?, "stop").await
}

pub async fn restart(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<InstanceStatus>, ApiError> {
    let ip = peer_ip(&peer, &headers);
    lifecycle(&st, &user, ip, parse_id(&id)?, "restart").await
}

pub async fn status(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<InstanceStatus>, ApiError> {
    let id = parse_id(&id)?;
    let driver = st.driver(id).await?;
    Ok(Json(driver.status().await?))
}

#[derive(Debug, Serialize)]
pub struct HealthView {
    pub healthy: bool,
    pub message: String,
    pub checks: Vec<CheckView>,
}

#[derive(Debug, Serialize)]
pub struct CheckView {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub async fn health_check(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<HealthView>, ApiError> {
    let id = parse_id(&id)?;
    let driver = st.driver(id).await?;
    let report = driver.health().await?;
    Ok(Json(HealthView {
        healthy: report.healthy,
        message: report.message,
        checks: report
            .checks
            .into_iter()
            .map(|(name, ok, detail)| CheckView { name, ok, detail })
            .collect(),
    }))
}

pub async fn capabilities(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Capabilities>, ApiError> {
    let id = parse_id(&id)?;
    let driver = st.driver(id).await?;
    Ok(Json(driver.probe().await?))
}

#[derive(Debug, Serialize)]
pub struct ReloadView {
    pub outcome: String,
    pub restarted: bool,
}

pub async fn reload(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ReloadView>, ApiError> {
    user.require_admin()?;
    let id = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = repo::get_instance(&st.db, id).await?;
    let driver = st.driver(id).await?;
    let started = Instant::now();
    let outcome = driver.reload().await?;
    st.audit
        .record(
            &user.actor(ip),
            action::INSTANCE_RELOAD,
            &row.name,
            AuditResult::Success,
            started.elapsed().as_millis() as i64,
            serde_json::json!({ "outcome": outcome.as_str() }),
        )
        .await;
    Ok(Json(ReloadView {
        outcome: outcome.as_str().to_string(),
        restarted: outcome == ReloadOutcome::Restarted,
    }))
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("实例 ID 不合法"))
}

async fn build_connection(
    input: Option<ConnectionInput>,
    _st: &AppState,
    driver: DriverKind,
) -> Result<Connection, ApiError> {
    match input {
        None => {
            if driver == DriverKind::SshBinary {
                Err(ApiError::bad_request("SSH 驱动必须提供 connection 信息"))
            } else {
                Ok(Connection::Local)
            }
        }
        Some(ConnectionInput::Local) => Ok(Connection::Local),
        Some(ConnectionInput::Ssh {
            host,
            port,
            username,
            auth_kind,
            credential_id,
            sudo,
            sudo_cmd,
            host_key_fingerprint,
        }) => {
            if host.trim().is_empty() {
                return Err(ApiError::bad_request("SSH 主机不能为空"));
            }
            if port == 0 {
                return Err(ApiError::bad_request("SSH 端口必须大于 0"));
            }
            if username.trim().is_empty() {
                return Err(ApiError::bad_request("SSH 用户名不能为空"));
            }
            let auth = AuthKind::parse(&auth_kind)
                .ok_or_else(|| ApiError::bad_request(format!("未知认证方式：{auth_kind}")))?;
            let cred = match credential_id {
                Some(c) if !c.is_empty() => Some(
                    Uuid::parse_str(&c)
                        .map_err(|_| ApiError::bad_request("凭据 ID 不合法"))?,
                ),
                _ => None,
            };
            if auth != AuthKind::Agent && cred.is_none() {
                return Err(ApiError::bad_request(
                    "密码/私钥认证必须绑定一个凭据（credential_id）",
                ));
            }
            Ok(Connection::Ssh {
                host: host.trim().to_string(),
                port,
                username: username.trim().to_string(),
                auth_kind: auth,
                credential_id: cred,
                sudo: sudo.as_deref().and_then(SudoMode::parse).unwrap_or(SudoMode::None),
                sudo_cmd,
                host_key_fingerprint,
            })
        }
    }
}

fn validate_admin_api(
    input: Option<&AdminApiInput>,
) -> Result<(String, String, Option<String>), ApiError> {
    let Some(api) = input else {
        return Ok((String::new(), String::new(), None));
    };
    let base = api.base_url.trim();
    if !base.is_empty() {
        crate::frpapi::normalize_base(base)?;
    }
    Ok((
        base.to_string(),
        api.user.clone(),
        api.prometheus_url.clone().filter(|p| !p.is_empty()),
    ))
}

/// 诊断端点：列出能力探测看到的每个分片结果（排查"误判实现"用）。
pub async fn diagnose_capabilities(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let iid = parse_id(&id)?;
    let row = repo::get_instance(&st.db, iid).await?;
    let pw = st.decrypt_admin_password(&row).unwrap_or_default();
    let Some(client) = FrpAdminClient::new(
        &row.admin_api_base_url,
        crate::frpapi::AdminAuth {
            user: row.admin_api_user.clone(),
            password: pw.clone(),
        },
        row.role,
        std::time::Duration::from_millis(st.cfg.raw.frp.admin_timeout_ms),
        None,
    )
    .ok() else {
        return Ok(Json(serde_json::json!({
            "error": "Admin API 未配置或地址不合法",
            "base_url": row.admin_api_base_url,
        })));
    };
    let endpoints = crate::frpapi::probe::diagnose_endpoints(&client)
        .await
        .into_iter()
        .map(|(p, s, b)| serde_json::json!({ "path": p, "status": s, "body": b }))
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({
        "base_url": row.admin_api_base_url,
        "user": row.admin_api_user,
        "password_len": pw.len(),
        "password_set": !pw.is_empty(),
        "role": row.role.as_str(),
        "endpoints": endpoints,
    })))
}