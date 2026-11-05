//! SSH 相关端点：连通性测试、主机指纹、环境探测。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::AuditResult;
use crate::domain::AuthKind;
use crate::error::ApiError;
use crate::ssh::hostkey;
use crate::ssh::session::SshSession;
use crate::ssh::SshConnectParams;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

#[derive(Debug, Deserialize)]
pub struct TestConnectionRequest {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    pub auth_kind: String,
    /// 使用已存凭据
    #[serde(default)]
    pub credential_id: Option<String>,
    /// 或直接传（仅本次使用，不落库；用于向导里"先测一下"）
    #[serde(default)]
    pub secret_inline: Option<String>,
    #[serde(default)]
    pub passphrase_inline: Option<String>,
}

fn default_port() -> u16 {
    22
}

#[derive(Debug, Serialize)]
pub struct SshTestResult {
    pub stage: String,
    pub ok: bool,
    pub message: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_key: Option<HostKeyInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct HostKeyInfo {
    pub algorithm: String,
    pub fingerprint: String,
    pub trusted: bool,
}

pub async fn test_connection(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<TestConnectionRequest>,
) -> Result<Json<SshTestResult>, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    let started = std::time::Instant::now();

    let auth = AuthKind::parse(&req.auth_kind)
        .ok_or_else(|| ApiError::bad_request(format!("未知认证方式：{}", req.auth_kind)))?;

    // 凭据来源：已存凭据 或 本次临时传入
    let (secret, passphrase) = if let Some(cid) = req
        .credential_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let id = Uuid::parse_str(cid)
            .map_err(|_| ApiError::bad_request("凭据 ID 不合法"))?;
        let (_, s, p) = st.decrypt_credential_async(id).await?;
        (Some(s), p)
    } else {
        (req.secret_inline.clone(), req.passphrase_inline.clone())
    };

    let params = SshConnectParams {
        host: req.host.trim().to_string(),
        port: req.port,
        username: req.username.trim().to_string(),
        auth_kind: auth,
        secret,
        passphrase,
    };

    let result = match SshSession::connect(&params, &st.cfg).await {
        Ok(session) => {
            // 连上了：跑一条无害命令确认 exec 通道可用
            let exec_ok = session.exec("id -un").await.is_ok();
            let hk = session.host_key.clone().map(|k| HostKeyInfo {
                algorithm: k.algorithm.clone(),
                fingerprint: k.fingerprint.clone(),
                trusted: true,
            });
            let _ = session.disconnect().await;
            SshTestResult {
                stage: "done".into(),
                ok: exec_ok,
                message: if exec_ok {
                    "连接与认证成功".into()
                } else {
                    "认证成功，但命令通道不可用".to_string()
                },
                duration_ms: started.elapsed().as_millis() as u64,
                host_key: hk,
                remediation: if exec_ok {
                    None
                } else {
                    Some("该账号可能被限制了 shell 访问（如 ForceCommand）".into())
                },
            }
        }
        Err(e) => {
            // 主机密钥未信任：主动去拿指纹回填，供前端展示确认
            let msg = e.to_string();
            let mut host_key = None;
            if msg.contains("host_key") || msg.contains("指纹") {
                // 用一个"只握手不认证"的探测拿指纹：复用 trust-getter
                host_key = fetch_host_key(&params, &st).await;
            }
            let stage = stage_of(&msg);
            SshTestResult {
                stage: stage.into(),
                ok: false,
                message: msg,
                duration_ms: started.elapsed().as_millis() as u64,
                host_key,
                remediation: Some(match stage {
                    "connect" => "检查主机地址、端口与防火墙",
                    "host_key" => "在下方核对指纹并信任后再重试",
                    "auth" => "检查用户名与密码/私钥是否匹配",
                    _ => "查看面板日志获取详情",
                }
                .into()),
            }
        }
    };

    st.audit
        .record(
            &user.actor(ip),
            action::SSH_TEST,
            &format!("{}:{}", params.host, params.port),
            if result.ok {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            result.duration_ms as i64,
            serde_json::json!({
                "stage": result.stage,
                "user": params.username,
                // 不记录任何凭据内容
            }),
        )
        .await;

    Ok(Json(result))
}

fn stage_of(msg: &str) -> &'static str {
    if msg.contains("host_key") || msg.contains("指纹") {
        "host_key"
    } else if msg.contains("auth") || msg.contains("认证") {
        "auth"
    } else {
        "connect"
    }
}

/// 只做握手拿主机密钥指纹（不认证）。
async fn fetch_host_key(params: &SshConnectParams, st: &AppState) -> Option<HostKeyInfo> {
    // 临时"信任一切"的探测连接：仅用于展示指纹，不用于任何实际操作
    let probe = SshConnectParams {
        secret: None,
        passphrase: None,
        ..params.clone()
    };
    let fp = crate::ssh::hostkey::probe_fingerprint(&probe, &st.cfg).await;
    fp.map(|(algorithm, fingerprint)| HostKeyInfo {
        algorithm,
        fingerprint,
        trusted: false,
    })
}

// ---------------------------------------------------------------------------
// 主机指纹
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct HostKeyView {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub trusted_at: String,
}

pub async fn list_host_keys(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Json<Vec<HostKeyView>>, ApiError> {
    let rows = crate::db::repo::list_host_keys(&st.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| HostKeyView {
                id: r.id.to_string(),
                host: r.host,
                port: r.port,
                algorithm: r.algorithm,
                fingerprint: r.fingerprint,
                trusted_at: r.trusted_at,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct TrustRequest {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    /// OpenSSH 单行公钥（`ssh-ed25519 AAAA...`）
    pub public_key: String,
}

pub async fn trust_host_key(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<TrustRequest>,
) -> Result<Json<HostKeyView>, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);
    if req.fingerprint.trim().is_empty() || req.public_key.trim().is_empty() {
        return Err(ApiError::bad_request("指纹与公钥不能为空"));
    }
    hostkey::trust(
        &st.db,
        &req.host,
        req.port,
        &req.algorithm,
        req.fingerprint.trim(),
        req.public_key.trim(),
    )
    .await?;
    st.audit
        .success(
            &user.actor(ip),
            action::HOSTKEY_TRUST,
            &format!("{}:{}", req.host, req.port),
            serde_json::json!({
                "fingerprint": req.fingerprint,
                "algorithm": req.algorithm,
            }),
        )
        .await;

    Ok(Json(HostKeyView {
        id: String::new(),
        host: req.host,
        port: req.port,
        algorithm: req.algorithm,
        fingerprint: req.fingerprint,
        trusted_at: crate::db::now_str(),
    }))
}

pub async fn delete_host_key(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    user.require_admin()?;
    let hid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("主机密钥 ID 不合法"))?;
    let ip = peer_ip(&peer, &headers);
    crate::db::repo::delete_host_key(&st.db, hid).await?;
    st.audit
        .success(
            &user.actor(ip),
            action::HOSTKEY_DELETE,
            &id,
            serde_json::json!({}),
        )
        .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    #[serde(flatten)]
    pub conn: TestConnectionRequest,
    #[serde(default)]
    pub config_dir: Option<String>,
    #[serde(default)]
    pub candidate_ports: Option<Vec<u16>>,
}

pub async fn probe_environment(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<ProbeRequest>,
) -> Result<Json<crate::ssh::probe::RemoteEnvironment>, ApiError> {
    user.require_admin()?;
    let auth = AuthKind::parse(&req.conn.auth_kind)
        .ok_or_else(|| ApiError::bad_request(format!("未知认证方式：{}", req.conn.auth_kind)))?;
    let (secret, passphrase) = if let Some(cid) =
        req.conn.credential_id.as_deref().filter(|s| !s.is_empty())
    {
        let id = Uuid::parse_str(cid).map_err(|_| ApiError::bad_request("凭据 ID 不合法"))?;
        let (_, s, p) = st.decrypt_credential_async(id).await?;
        (Some(s), p)
    } else {
        (req.conn.secret_inline.clone(), req.conn.passphrase_inline.clone())
    };

    let params = SshConnectParams {
        host: req.conn.host.trim().to_string(),
        port: req.conn.port,
        username: req.conn.username.trim().to_string(),
        auth_kind: auth,
        secret,
        passphrase,
    };
    let session = SshSession::connect(&params, &st.cfg).await?;
    let config_dir = req
        .config_dir
        .clone()
        .unwrap_or_else(|| st.cfg.raw.ssh.default_config_dir.clone());
    let ports = req.candidate_ports.unwrap_or_default();
    let env = crate::ssh::probe::probe(
        &session,
        &crate::ssh::probe::ProbeOptions {
            config_dir: &config_dir,
            candidate_ports: &ports,
        },
    )
    .await?;
    let _ = session.disconnect().await;
    Ok(Json(env))
}
