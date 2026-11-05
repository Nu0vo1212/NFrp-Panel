//! 面板设置端点：默认值、安全项、SSH 默认值、版本清单。

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::audit::action;
use crate::error::ApiError;
use crate::state::AppState;

pub use crate::deploy::catalog::CatalogEntry;

use super::{peer_ip, AuthUser, PeerIp};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsView {
    pub driver: String,
    pub flavor: String,
    pub binary_dir: String,
    pub server_binary: String,
    pub client_binary: String,
    pub config_dir: String,
    pub log_dir: String,
    pub work_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrpView {
    pub admin_timeout_ms: u64,
    pub status_poll_ms: u64,
    pub metrics_history_days: u32,
    pub auto_restart_on_save: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshView {
    pub connect_timeout_ms: u64,
    pub command_timeout_ms: u64,
    pub retry: u32,
    pub default_port: u16,
    pub default_user: String,
    pub default_install_dir: String,
    pub default_config_dir: String,
    pub default_log_dir: String,
    pub default_service: String,
    pub default_sudo: String,
    pub allow_custom_command: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityView {
    pub access_token_ttl_minutes: i64,
    pub audit_retention_days: u32,
    /// 主密钥指纹（用于确认密钥是否一致，**不泄露密钥**）
    pub master_key_fingerprint: String,
    pub audit_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsView {
    pub defaults: DefaultsView,
    pub frp: FrpView,
    pub ssh: SshView,
    pub security: SecurityView,
    pub theme: String,
}

fn view_of(st: &AppState) -> SettingsView {
    let c = &st.cfg.raw;
    SettingsView {
        defaults: DefaultsView {
            driver: c.defaults.driver.clone(),
            flavor: c.defaults.flavor.clone(),
            binary_dir: c.defaults.binary_dir.clone(),
            server_binary: c.defaults.server_binary.clone(),
            client_binary: c.defaults.client_binary.clone(),
            config_dir: c.defaults.config_dir.clone(),
            log_dir: c.defaults.log_dir.clone(),
            work_dir: c.defaults.work_dir.clone(),
        },
        frp: FrpView {
            admin_timeout_ms: c.frp.admin_timeout_ms,
            status_poll_ms: c.frp.status_poll_ms,
            metrics_history_days: c.frp.metrics_history_days,
            auto_restart_on_save: c.frp.auto_restart_on_save,
        },
        ssh: SshView {
            connect_timeout_ms: c.ssh.connect_timeout_ms,
            command_timeout_ms: c.ssh.command_timeout_ms,
            retry: c.ssh.retry,
            default_port: c.ssh.default_port,
            default_user: c.ssh.default_user.clone(),
            default_install_dir: c.ssh.default_install_dir.clone(),
            default_config_dir: c.ssh.default_config_dir.clone(),
            default_log_dir: c.ssh.default_log_dir.clone(),
            default_service: c.ssh.default_service.clone(),
            default_sudo: c.ssh.default_sudo.clone(),
            allow_custom_command: c.ssh.allow_custom_command,
        },
        security: SecurityView {
            access_token_ttl_minutes: c.security.access_token_ttl_minutes,
            audit_retention_days: c.audit.retention_days,
            master_key_fingerprint: st.keys.master_fingerprint(),
            audit_enabled: c.audit.enabled,
        },
        theme: c.theme.default.clone(),
    }
}

pub async fn get_settings(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Json<SettingsView>, ApiError> {
    Ok(Json(view_of(&st)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    #[serde(default)]
    pub defaults: Option<DefaultsView>,
    #[serde(default)]
    pub frp: Option<FrpView>,
    #[serde(default)]
    pub ssh: Option<SshView>,
    #[serde(default)]
    pub theme: Option<String>,
}

pub async fn update_settings(
    State(st): State<Arc<AppState>>,
    user: AuthUser,
    peer: PeerIp,
    headers: HeaderMap,
    Json(req): Json<UpdateSettingsRequest>,
) -> Result<Json<SettingsView>, ApiError> {
    user.require_admin()?;
    let ip = peer_ip(&peer, &headers);

    // ★ 设置分两类：
    //   1) 只能改配置文件（`panel.toml`）的项 —— 这里不落库，只返回"需重启"提示；
    //   2) 可热更新的项 —— 存到 settings 表。
    //   为保证"所见即所存"，可热更新的项在启动时优先读 DB。
    if let Some(theme) = &req.theme {
        if theme == "light" || theme == "dark" {
            crate::db::repo::put_setting(&st.db, "theme.default", theme).await?;
        } else {
            return Err(ApiError::bad_request("theme 只能是 light 或 dark"));
        }
    }
    if let Some(frp) = &req.frp {
        crate::db::repo::put_setting(
            &st.db,
            "frp.auto_restart_on_save",
            if frp.auto_restart_on_save { "1" } else { "0" },
        )
        .await?;
        crate::db::repo::put_setting(
            &st.db,
            "frp.status_poll_ms",
            &frp.status_poll_ms.to_string(),
        )
        .await?;
    }
    if let Some(ssh) = &req.ssh {
        crate::db::repo::put_setting(
            &st.db,
            "ssh.allow_custom_command",
            if ssh.allow_custom_command { "1" } else { "0" },
        )
        .await?;
    }
    if let Some(d) = &req.defaults {
        crate::db::repo::put_setting(&st.db, "defaults.driver", &d.driver).await?;
        crate::db::repo::put_setting(&st.db, "defaults.flavor", &d.flavor).await?;
    }

    st.audit
        .record(
            &user.actor(ip),
            action::SETTINGS_UPDATE,
            "panel",
            crate::db::models::AuditResult::Success,
            0,
            serde_json::json!({
                "changed": {
                    "defaults": req.defaults.is_some(),
                    "frp": req.frp.is_some(),
                    "ssh": req.ssh.is_some(),
                    "theme": req.theme.is_some(),
                },
                "note": "部分设置需重启面板生效",
            }),
        )
        .await;

    Ok(Json(view_of(&st)))
}

/// 内置版本清单（离线可用；可按需替换为在线清单）。
#[derive(Debug, Serialize)]
pub struct CatalogView {
    pub entries: Vec<crate::deploy::catalog::CatalogEntry>,
    pub note: String,
}

pub async fn catalog(_user: AuthUser) -> Result<Json<CatalogView>, ApiError> {
    Ok(Json(CatalogView {
        entries: crate::deploy::catalog::builtin_entries(),
        note: "清单为离线内置示例：正式部署请填入你自己的镜像地址与 SHA256 校验值".into(),
    }))
}