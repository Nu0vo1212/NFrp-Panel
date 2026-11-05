//! 凭据管理端点。
//!
//! 铁律：**响应里永不出现明文**。列表只给 `has_secret` 与 `secret_preview`（固定掩码）。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::action;
use crate::db::models::{AuditResult, CredentialKind};
use crate::error::ApiError;
use crate::state::AppState;

use super::{peer_ip, AuthUser, PeerIp};

#[derive(Debug, Serialize)]
pub struct CredentialSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub username: Option<String>,
    pub has_secret: bool,
    pub secret_preview: String,
    pub algorithm_hint: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub used_by: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCredentialRequest {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub username: Option<String>,
    /// 密码或私钥内容（明文，仅此一次）
    pub secret: String,
    /// 私钥口令（可选）
    #[serde(default)]
    pub passphrase: Option<String>,
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Json<Vec<CredentialSummary>>, ApiError> {
    let rows = crate::db::repo::list_credentials(&st.db).await?;
    let instances = crate::db::repo::list_instances(&st.db).await.unwrap_or_default();

    let items = rows
        .into_iter()
        .map(|c| {
            let used_by = instances
                .iter()
                .filter(|i| {
                    matches!(&i.connection, crate::domain::Connection::Ssh { credential_id, .. } if *credential_id == Some(c.id))
                })
                .map(|i| i.name.clone())
                .collect::<Vec<_>>();
            CredentialSummary {
                id: c.id.to_string(),
                name: c.name,
                kind: c.kind.as_str().to_string(),
                username: c.username,
                has_secret: true,
                secret_preview: "••••••••".into(),
                algorithm_hint: c.algorithm_hint,
                created_at: c.created_at,
                last_used_at: c.last_used_at,
                used_by,
            }
        })
        .collect();

    Ok(Json(items))
}

pub async fn create(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<CreateCredentialRequest>,
) -> Result<(axum::http::StatusCode, Json<CredentialSummary>), ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);

    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("凭据名称不能为空"));
    }
    let kind = CredentialKind::parse(&req.kind)
        .ok_or_else(|| ApiError::bad_request(format!("未知凭据类型：{}", req.kind)))?;
    if req.secret.is_empty() {
        return Err(ApiError::bad_request("凭据内容不能为空"));
    }

    // 私钥格式预检：能在本地解析就说明可用，避免部署到一半才失败
    let algorithm_hint = if kind == CredentialKind::SshPrivateKey {
        Some(
            detect_key_algorithm(&req.secret)
                .unwrap_or_else(|| "未知算法".into()),
        )
    } else {
        None
    };

    // ★ AAD 绑定记录 ID，必须先有 ID 再加密：这里先生成 ID
    let id = Uuid::new_v4();
    let secret_enc = st
        .keys
        .encrypt(&req.secret, &format!("credential:{id}:secret"))?;
    let passphrase_enc = match req.passphrase.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => Some(
            st.keys
                .encrypt(p, &format!("credential:{id}:passphrase"))?,
        ),
        None => None,
    };

    let row = crate::db::repo::insert_credential_with_id(
        &st.db,
        id,
        &name,
        kind,
        req.username.as_deref(),
        &secret_enc,
        passphrase_enc.as_deref(),
        algorithm_hint.as_deref(),
    )
    .await?;

    st.audit
        .record(
            &user.actor(ip),
            action::CREDENTIAL_CREATE,
            &name,
            AuditResult::Success,
            0,
            serde_json::json!({
                "credential_id": row.id.to_string(),
                "kind": kind.as_str(),
                // 绝不记录凭据内容与口令
            }),
        )
        .await;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CredentialSummary {
            id: row.id.to_string(),
            name: row.name,
            kind: row.kind.as_str().to_string(),
            username: row.username,
            has_secret: true,
            secret_preview: "••••••••".into(),
            algorithm_hint: row.algorithm_hint,
            created_at: row.created_at,
            last_used_at: None,
            used_by: vec![],
        }),
    ))
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    user.require_admin()?;
    let cid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("凭据 ID 不合法"))?;
    let ip = peer_ip(&peer, &headers);

    // 被实例引用的凭据不能直接删，否则实例会静默失效
    let instances = crate::db::repo::list_instances(&st.db).await?;
    let used: Vec<String> = instances
        .iter()
        .filter(|i| {
            matches!(&i.connection, crate::domain::Connection::Ssh { credential_id, .. } if *credential_id == Some(cid))
        })
        .map(|i| i.name.clone())
        .collect();
    if !used.is_empty() {
        return Err(ApiError::conflict(format!(
            "该凭据正被实例使用：{}。请先解除绑定再删除",
            used.join("、")
        )));
    }

    let cred = crate::db::repo::get_credential(&st.db, cid).await?;
    crate::db::repo::delete_credential(&st.db, cid).await?;
    st.audit
        .success(
            &user.actor(ip),
            action::CREDENTIAL_DELETE,
            &cred.name,
            serde_json::json!({ "credential_id": cid.to_string() }),
        )
        .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// 尝试从私钥文本里识别算法（不解析内容，只看头尾标记）。
pub fn detect_key_algorithm(key: &str) -> Option<String> {
    let head = key.trim_start();
    if head.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
        // 新版 openssh 格式：内部包含算法名，但需解码才能确定；给出通用说明
        return Some("OpenSSH 私钥（ed25519/ecdsa/rsa 之一）".into());
    }
    for (marker, name) in [
        ("BEGIN RSA PRIVATE KEY", "RSA"),
        ("BEGIN EC PRIVATE KEY", "ECDSA"),
        ("BEGIN DSA PRIVATE KEY", "DSA"),
        ("BEGIN PRIVATE KEY", "PKCS#8（RSA/ECDSA/Ed25519）"),
        ("BEGIN ENCRYPTED PRIVATE KEY", "PKCS#8 加密（需口令）"),
    ] {
        if head.contains(marker) {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_key_types() {
        assert_eq!(
            detect_key_algorithm("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
            Some("OpenSSH 私钥（ed25519/ecdsa/rsa 之一）".into())
        );
        assert_eq!(
            detect_key_algorithm("-----BEGIN RSA PRIVATE KEY-----"),
            Some("RSA".into())
        );
        assert_eq!(detect_key_algorithm("not a key"), None);
    }
}
