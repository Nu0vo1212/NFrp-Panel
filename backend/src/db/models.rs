//! 数据库模型（与表结构一一对应）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{
    AuthKind, BinaryFlavor, Capabilities, Connection, DeploymentStatus, DriverKind,
    InstancePaths, InstanceRole, ServiceMode, SudoMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "viewer" {
            Role::Viewer
        } else {
            Role::Admin
        }
    }
    pub fn can_write(&self) -> bool {
        matches!(self, Role::Admin)
    }
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    SshPassword,
    SshPrivateKey,
    AdminPassword,
    Token,
}

impl CredentialKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SshPassword => "ssh_password",
            Self::SshPrivateKey => "ssh_private_key",
            Self::AdminPassword => "admin_password",
            Self::Token => "token",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ssh_password" => Self::SshPassword,
            "ssh_private_key" => Self::SshPrivateKey,
            "admin_password" => Self::AdminPassword,
            "token" => Self::Token,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CredentialRow {
    pub id: Uuid,
    pub name: String,
    pub kind: CredentialKind,
    pub username: Option<String>,
    pub secret_enc: String,
    pub passphrase_enc: Option<String>,
    pub algorithm_hint: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstanceRow {
    pub id: Uuid,
    pub name: String,
    pub role: InstanceRole,
    pub driver: DriverKind,
    pub flavor: BinaryFlavor,
    pub enabled: bool,
    pub tags: Vec<String>,
    pub connection: Connection,
    pub paths: InstancePaths,
    pub service_mode: ServiceMode,
    pub service_unit: Option<String>,
    pub admin_api_base_url: String,
    pub admin_api_user: String,
    pub admin_api_password_enc: Option<String>,
    pub admin_api_prometheus_url: Option<String>,
    pub admin_api_enabled: bool,
    pub binary_version: Option<String>,
    pub deployment_status: DeploymentStatus,
    pub capabilities: Option<Capabilities>,
    pub last_deployed_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct HostKeyRow {
    pub id: Uuid,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub public_key: String,
    pub trusted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Success,
    Failure,
    Denied,
}

impl AuditResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "denied" => Self::Denied,
            _ => Self::Failure,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: Uuid,
    pub ts: String,
    pub actor: String,
    pub actor_ip: String,
    pub action: String,
    pub target: String,
    pub result: AuditResult,
    pub duration_ms: i64,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub ts: i64,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub conns: i64,
    pub clients: i64,
    pub proxies: i64,
}

#[derive(Debug, Clone)]
pub struct ConfigBackupRow {
    pub id: Uuid,
    pub instance_id: Uuid,
    pub path: String,
    pub content_sha: String,
    pub size: i64,
    pub created_at: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct DeployTaskRow {
    pub id: Uuid,
    pub instance_id: Uuid,
    pub kind: String,
    pub state: String,
    pub steps: serde_json::Value,
    pub logs: serde_json::Value,
    pub error: Option<String>,
    pub rollback_of: Option<Uuid>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// SSH 认证方式的展示名（用于审计与 UI）。
pub fn auth_kind_label(k: AuthKind) -> &'static str {
    match k {
        AuthKind::Password => "密码",
        AuthKind::PrivateKey => "私钥",
        AuthKind::PrivateKeyPassphrase => "私钥+口令",
        AuthKind::Agent => "ssh-agent",
    }
}

pub fn sudo_label(s: SudoMode) -> &'static str {
    match s {
        SudoMode::None => "无",
        SudoMode::Sudo => "sudo",
        SudoMode::SudoN => "sudo -n",
        SudoMode::Custom => "自定义",
    }
}
