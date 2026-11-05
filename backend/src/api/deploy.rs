//! 部署端点：预检、执行、状态、回滚、SSE 流。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::AuditResult;
use crate::deploy::hub::{DeployState, DeployStepState};
use crate::deploy::preflight::{self, PreflightInput, PreflightReport};
use crate::deploy::runner::{self, DeployInput};
use crate::domain::{BinaryFlavor, InstanceRole, ServiceMode};
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("实例 ID 不合法"))
}

// ---------------------------------------------------------------------------
// 预检
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct PreflightRequest {
    /// 上传的二进制（base64），用于架构与哈希预检
    #[serde(default)]
    pub binary_b64: Option<String>,
    #[serde(default)]
    pub arch_expected: Option<String>,
    #[serde(default)]
    pub config: Option<String>,
}

pub async fn preflight_run(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<PreflightRequest>,
) -> Result<Json<PreflightReport>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let row = crate::db::repo::get_instance(&st.db, iid).await?;

    let driver = st.driver(iid).await?;
    let Some(ssh) = crate::driver::ssh::as_ssh_driver(&driver) else {
        return Err(ApiError::bad_request(
            "部署预检仅适用于 SSH 远程实例；本机实例请直接使用启停与保存配置",
        ));
    };
    let session = ssh.session().await?;

    let env = crate::ssh::probe::probe(
        &session,
        &crate::ssh::probe::ProbeOptions {
            config_dir: &dir_of(&row.paths.config, "/etc/nfrp"),
            candidate_ports: &[],
        },
    )
    .await?;

    let binary_bytes = match req.binary_b64.as_deref() {
        Some(b64) if !b64.is_empty() => Some(crate::crypto::base64_decode(b64).map_err(|_| {
            ApiError::bad_request("binary_b64 不是合法 base64")
        })?),
        _ => None,
    };
    let arch_expected = req
        .arch_expected
        .clone()
        .unwrap_or_else(|| crate::ssh::probe::normalize_arch(&env.arch));

    let report = preflight::run(
        &session,
        &env,
        &PreflightInput {
            flavor: row.flavor,
            arch_expected: &arch_expected,
            install_dir: &row.paths.remote_root(),
            config_dir: &dir_of(&row.paths.config, "/etc/nfrp"),
            log_dir: &dir_of(&row.paths.log, "/var/log/nfrp"),
            config_port: extract_port(&req.config.clone().unwrap_or_default()),
            admin_api_url: &row.admin_api_base_url,
            service_mode_systemd: row.service_mode == ServiceMode::Systemd,
            binary_bytes: binary_bytes.as_deref(),
        },
    )
    .await?;

    Ok(Json(report))
}

fn dir_of(path: &str, fallback: &str) -> String {
    path.rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// 从配置文本里粗提控制端口（用于端口占用预检）。
fn extract_port(config: &str) -> Option<u16> {
    for key in ["bind_port", "bindPort"] {
        for line in config.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(key) {
                let rest = rest.trim_start().trim_start_matches('=').trim();
                if let Ok(p) = rest.parse::<u16>() {
                    return Some(p);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 执行
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeployRequest {
    /// 二进制内容（base64）。为 None 时只下发配置
    #[serde(default)]
    pub binary_b64: Option<String>,
    /// 二进制文件名（默认按角色取 frps/frpc）
    #[serde(default)]
    pub binary_name: Option<String>,
    /// 配置内容
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub unit_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskCreated {
    pub task_id: String,
    pub stream: String,
}

pub async fn deploy(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<DeployRequest>,
) -> Result<(axum::http::StatusCode, Json<TaskCreated>), ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;

    if row.driver != crate::domain::DriverKind::SshBinary {
        return Err(ApiError::bad_request(
            "部署接口仅适用于 SSH 远程实例；本机实例请直接管理进程与配置",
        ));
    }

    // 配置：优先用请求里的，否则读当前远程配置
    let driver = st.driver(iid).await?;
    let config = match req.config.clone().filter(|c| !c.trim().is_empty()) {
        Some(c) => c,
        None => driver.read_config().await?.content,
    };
    let report = driver.validate_config(&config).await?;
    if !report.ok {
        return Err(ApiError::Validation(format!(
            "配置校验未通过：{}",
            report
                .errors
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("；")
        )));
    }

    let binary = match req.binary_b64.as_deref().filter(|b| !b.is_empty()) {
        Some(b64) => Some(
            crate::crypto::base64_decode(b64)
                .map_err(|_| ApiError::bad_request("binary_b64 不是合法 base64"))?,
        ),
        None => None,
    };

    let binary_name = req
        .binary_name
        .clone()
        .unwrap_or_else(|| runner::binary_name_for(row.role).to_string());
    let unit_name = req
        .unit_name
        .clone()
        .or_else(|| row.service_unit.clone())
        .unwrap_or_else(|| runner::default_unit_name(row.role, row.flavor));

    let transfer = runner::transfer_context_for(&st, &row).await?;
    let admin_password = st.decrypt_admin_password(&row).unwrap_or_default();

    // 建任务
    let plan = crate::deploy::plan::build_plan(
        row.role,
        row.service_mode,
        row.flavor,
        binary.is_some(),
    );
    let task_id = Uuid::new_v4().to_string();
    let (_tx, _rx) = st
        .deploy
        .create(&task_id, iid, "deploy", crate::deploy::plan::to_steps(&plan), None)
        .await;

    // 持久化任务
    let _ = crate::db::repo::insert_deploy_task(&st.db, iid, "deploy", None).await;

    st.audit
        .record(
            &user.actor(ip),
            action::DEPLOY_RUN,
            &row.name,
            AuditResult::Success,
            0,
            serde_json::json!({
                "instance_id": iid.to_string(),
                "task_id": task_id,
                "with_binary": binary.is_some(),
                "service": row.service_mode.as_str(),
            }),
        )
        .await;

    // 后台执行
    let st2 = st.clone();
    let task_id_for_run = task_id.clone();
    let input = DeployInput {
        instance: row,
        binary,
        binary_name,
        config,
        unit_name,
        admin_url: row_admin_url(&st2, iid).await,
        admin_user: String::new(),
        admin_password,
        transfer,
    };
    tokio::spawn(async move {
        let _ = runner::run(st2, task_id_for_run, input).await;
    });

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(TaskCreated {
            stream: format!("/api/deploy/tasks/{task_id}/stream"),
            task_id,
        }),
    ))
}

async fn row_admin_url(st: &Arc<AppState>, id: Uuid) -> String {
    match crate::db::repo::get_instance(&st.db, id).await {
        Ok(r) => {
            let user = r.admin_api_user.clone();
            let _ = user;
            r.admin_api_base_url
        }
        Err(_) => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 状态 / 流
// ---------------------------------------------------------------------------

pub async fn deploy_status(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Option<crate::deploy::DeployTask>>, ApiError> {
    let iid = parse_id(&id)?;
    Ok(Json(st.deploy.latest_for(iid).await))
}

pub async fn task_stream(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(task_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let rx = st
        .deploy
        .subscribe(&task_id)
        .await
        .ok_or_else(|| ApiError::not_found("部署任务不存在或已结束"))?;

    // 先把已有步骤状态发一遍（刷新页面也能看到完整进度）
    let snapshot = st.deploy.get(&task_id).await;
    let initial: Vec<Result<Event, Infallible>> = snapshot
        .map(|t| {
            t.steps
                .iter()
                .map(|s| {
                    Ok(Event::default().event("step").data(
                        serde_json::json!({
                            "key": s.key,
                            "state": s.state,
                            "message": s.message,
                            "remediation": s.remediation,
                        })
                        .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let tail = futures::stream::unfold(
        rx,
        |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let name = ev.event_name().to_string();
                        let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                        return Some((
                            Ok::<_, Infallible>(Event::default().event(name).data(data)),
                            rx,
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        },
    );

    let stream = futures::stream::iter(initial).chain(tail);
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

// ---------------------------------------------------------------------------
// 回滚
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct RollbackRequest {
    /// 指定要回滚到的配置备份 ID（可选；默认用最近一次）
    #[serde(default)]
    pub backup_id: Option<String>,
}

pub async fn rollback(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    user.require_admin()?;
    let iid = parse_id(&id)?;
    let ip = peer_ip(&peer, &headers);
    let row = crate::db::repo::get_instance(&st.db, iid).await?;
    let driver = st.driver(iid).await?;

    let backups = driver.list_backups().await?;
    let target = match req.backup_id.as_deref().filter(|s| !s.is_empty()) {
        Some(b) => b.to_string(),
        None => backups
            .first()
            .map(|b| b.id.clone())
            .ok_or_else(|| ApiError::not_found("没有可用的配置备份"))?,
    };

    let started = std::time::Instant::now();
    let result = driver.restore_backup(&target).await;
    st.audit
        .record(
            &user.actor(ip),
            action::DEPLOY_ROLLBACK,
            &row.name,
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            started.elapsed().as_millis() as i64,
            serde_json::json!({
                "instance_id": iid.to_string(),
                "backup_id": target,
            }),
        )
        .await;

    let r = result?;
    Ok(Json(serde_json::json!({
        "rolled_back": true,
        "backup_id": target,
        "applied": r.applied,
    })))
}

#[derive(Debug, Deserialize, Default)]
pub struct HistoryQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn deploy_history(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let iid = parse_id(&id)?;
    let rows = crate::db::repo::list_deploy_tasks(&st.db, iid, q.limit.unwrap_or(20)).await?;
    Ok(Json(
        rows.into_iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id.to_string(),
                    "kind": t.kind,
                    "state": t.state,
                    "started_at": t.started_at,
                    "finished_at": t.finished_at,
                    "error": t.error,
                    "rollback_of": t.rollback_of.map(|u| u.to_string()),
                })
            })
            .collect(),
    ))
}

/// 二进制下载（从 URL）——供向导"从 URL 下载"使用。
#[derive(Debug, Deserialize)]
pub struct FetchUrlRequest {
    pub url: String,
    /// 期望的 SHA256（可选但**强烈建议**）
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FetchUrlResponse {
    pub binary_b64: String,
    pub size: usize,
    pub sha256: String,
    pub arch: Option<String>,
    pub notes: Vec<String>,
}

pub async fn fetch_binary(
    State(_st): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<FetchUrlRequest>,
) -> Result<Json<FetchUrlResponse>, ApiError> {
    user.require_admin()?;
    let url = req.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError::bad_request("只支持 http(s) 链接"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("创建 HTTP 客户端失败：{e}")))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::upstream(format!("下载失败：{e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::upstream(format!(
            "下载返回 HTTP {}",
            resp.status().as_u16()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::upstream(format!("读取下载内容失败：{e}")))?;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err(ApiError::bad_request("文件超过 128 MB，拒绝处理"));
    }

    let sha = crate::crypto::sha256_hex(&bytes);
    let mut notes = Vec::new();
    if let Some(expect) = req.sha256.as_deref().filter(|s| !s.is_empty()) {
        if !expect.eq_ignore_ascii_case(&sha) {
            return Err(ApiError::Validation(format!(
                "SHA256 校验失败：期望 {expect}，实际 {sha}"
            )));
        }
        notes.push("SHA256 校验通过".to_string());
    } else {
        notes.push("未提供 SHA256：建议提供以便校验完整性".to_string());
    }
    let arch = crate::deploy::catalog::elf_machine(&bytes).map(str::to_string);
    if arch.is_none() && crate::deploy::catalog::is_pe(&bytes) {
        notes.push("这是一个 Windows 可执行文件，不能部署到 Linux".into());
    }

    use base64::Engine;
    Ok(Json(FetchUrlResponse {
        binary_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        size: bytes.len(),
        sha256: sha,
        arch,
        notes,
    }))
}

/// 位掩码：让未使用的导入不报警（`DeployState`/`DeployStepState` 在别处用到）。
#[allow(dead_code)]
fn _keep() {
    let _ = DeployState::Pending;
    let _ = DeployStepState::Pending;
    let _ = InstanceRole::Server;
    let _ = BinaryFlavor::Nfrp;
}
