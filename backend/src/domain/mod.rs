//! 与驱动无关的领域模型。前端 TS 类型与此一一对应（见 `docs/02-api.md`）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    Binary,
    AdminApi,
    SshBinary,
    Embedded,
}

impl DriverKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::AdminApi => "admin_api",
            Self::SshBinary => "ssh_binary",
            Self::Embedded => "embedded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "binary" => Self::Binary,
            "admin_api" => Self::AdminApi,
            "ssh_binary" => Self::SshBinary,
            "embedded" => Self::Embedded,
            _ => return None,
        })
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Binary => "本机二进制",
            Self::AdminApi => "Admin API",
            Self::SshBinary => "SSH 远程二进制",
            Self::Embedded => "内嵌库（预留）",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BinaryFlavor {
    Nfrp,
    OfficialFrp,
}

impl BinaryFlavor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nfrp => "nfrp",
            Self::OfficialFrp => "official_frp",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "nfrp" => Self::Nfrp,
            "official_frp" | "official" | "frp" => Self::OfficialFrp,
            _ => return None,
        })
    }
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Nfrp => "NFrp (Rust-Frp)",
            Self::OfficialFrp => "官方 frp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceRole {
    Server,
    Client,
}

impl InstanceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "server" => Self::Server,
            "client" => Self::Client,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceMode {
    Process,
    Systemd,
    Nohup,
}

impl ServiceMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Systemd => "systemd",
            Self::Nohup => "nohup",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "process" => Self::Process,
            "systemd" => Self::Systemd,
            "nohup" => Self::Nohup,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    NotDeployed,
    Deployed,
    Failed,
    RolledBack,
}

impl DeploymentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotDeployed => "not_deployed",
            Self::Deployed => "deployed",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "not_deployed" => Self::NotDeployed,
            "deployed" => Self::Deployed,
            "failed" => Self::Failed,
            "rolled_back" => Self::RolledBack,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveState {
    Running,
    Stopped,
    Unknown,
    Error,
}

impl LiveState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Unknown => "unknown",
            Self::Error => "error",
        }
    }
}

// ---------------------------------------------------------------------------
// 连接方式
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Password,
    PrivateKey,
    PrivateKeyPassphrase,
    Agent,
}

impl AuthKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PrivateKey => "private_key",
            Self::PrivateKeyPassphrase => "private_key_passphrase",
            Self::Agent => "agent",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "password" => Self::Password,
            "private_key" => Self::PrivateKey,
            "private_key_passphrase" => Self::PrivateKeyPassphrase,
            "agent" => Self::Agent,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SudoMode {
    None,
    Sudo,
    SudoN,
    Custom,
}

impl SudoMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Sudo => "sudo",
            Self::SudoN => "sudo_n",
            Self::Custom => "custom",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Self::None,
            "sudo" => Self::Sudo,
            "sudo_n" => Self::SudoN,
            "custom" => Self::Custom,
            _ => return None,
        })
    }
}

/// 实例的连接方式：本机 或 SSH 远程。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Connection {
    Local,
    Ssh {
        host: String,
        port: u16,
        username: String,
        auth_kind: AuthKind,
        /// 凭据 ID（密码/私钥存于加密凭据表）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential_id: Option<Uuid>,
        sudo: SudoMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sudo_cmd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_key_fingerprint: Option<String>,
    },
}

impl Connection {
    pub fn is_remote(&self) -> bool {
        matches!(self, Connection::Ssh { .. })
    }
    pub fn host_label(&self) -> String {
        match self {
            Connection::Local => local_hostname(),
            Connection::Ssh { host, port, .. } => format!("{host}:{port}"),
        }
    }
}

pub fn local_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "localhost".into())
}

/// 本机/远程的路径约定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancePaths {
    pub binary: String,
    pub config: String,
    pub log: String,
    pub work_dir: String,
}

impl InstancePaths {
    pub fn remote_root(&self) -> String {
        let p = std::path::Path::new(&self.binary);
        p.parent()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "/usr/local/bin".into())
    }
}

/// Admin API 配置。密码**只存引用**，明文在加密凭据表里。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdminApiConfig {
    pub base_url: String,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<Uuid>,
    /// 加密后的明文（由 crypto 层处理，序列化时永不输出）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_password: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prometheus_url: Option<String>,
}

// ---------------------------------------------------------------------------
// 能力
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKind {
    /// NFrp 0.7.x 面板 API（/api/status + /api/v2/*）
    Nfrp,
    /// 官方 frp 0.5x+ 服务端 / 0.6x+ 客户端 API
    Official,
    Unknown,
    None,
}

impl ApiKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nfrp => "nfrp",
            Self::Official => "official",
            Self::Unknown => "unknown",
            Self::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "nfrp" => Self::Nfrp,
            "official" => Self::Official,
            "unknown" => Self::Unknown,
            "none" => Self::None,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrometheusSupport {
    Off,
    Enabled,
    Unknown,
}

impl PrometheusSupport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Enabled => "enabled",
            Self::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "enabled" => Self::Enabled,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDetail {
    /// 有流量字节、连接数等完整指标（NFrp）
    Full,
    /// 只有连接数/在线态（官方 frps）
    Aggregated,
    None,
}

impl MetricDetail {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Aggregated => "aggregated",
            Self::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "full" => Self::Full,
            "aggregated" => Self::Aggregated,
            "none" => Self::None,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityNote {
    pub level: String,
    pub text: String,
}

/// 能力位：前端唯一的驱动差异来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub flavor: BinaryFlavor,
    pub flavor_version: Option<String>,
    pub api_kind: ApiKind,
    pub managed_locally: bool,
    pub remote: bool,
    pub can_start_stop: bool,
    pub can_reload: bool,
    pub can_edit_config: bool,
    pub can_dynamic_proxy: bool,
    pub can_manage_visitors: bool,
    pub has_v2_api: bool,
    pub has_healthz: bool,
    pub prometheus: PrometheusSupport,
    pub metrics_detail: MetricDetail,
    pub auth_guard_header: bool,
    pub notes: Vec<CapabilityNote>,
    pub probed_at: Option<DateTime<Utc>>,
}

impl Capabilities {
    /// 未探测时的保守默认值：什么都不声称支持。
    pub fn unknown(flavor: BinaryFlavor) -> Self {
        Self {
            flavor,
            flavor_version: None,
            api_kind: ApiKind::Unknown,
            managed_locally: false,
            remote: false,
            can_start_stop: false,
            can_reload: false,
            can_edit_config: false,
            can_dynamic_proxy: false,
            can_manage_visitors: false,
            has_v2_api: false,
            has_healthz: false,
            prometheus: PrometheusSupport::Unknown,
            metrics_detail: MetricDetail::None,
            auth_guard_header: false,
            notes: Vec::new(),
            probed_at: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 状态 / 指标 / 代理
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceCounters {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub conns: u64,
    pub clients: u64,
    pub proxies: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub state: LiveState,
    pub healthy: bool,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub binary_version: Option<String>,
    pub api_reachable: bool,
    pub last_error: Option<String>,
    pub detail: StatusDetail,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters: Option<InstanceCounters>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusDetail {
    pub source: String,
    pub notes: Vec<String>,
}

impl InstanceStatus {
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            state: LiveState::Unknown,
            healthy: false,
            pid: None,
            uptime_secs: None,
            binary_version: None,
            api_reachable: false,
            last_error: Some(reason.into()),
            detail: StatusDetail {
                source: "unknown".into(),
                notes: Vec::new(),
            },
            counters: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub ts: DateTime<Utc>,
    pub api_reachable: bool,
    pub counters: InstanceCounters,
    pub availability: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub proxy_type: String,
    pub local_addr: String,
    pub remote_port: Option<u16>,
    pub custom_domains: Vec<String>,
    pub subdomain: String,
    pub status: String,
    pub error: Option<String>,
    pub cur_conns: u64,
    pub traffic_in: Option<u64>,
    pub traffic_out: Option<u64>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub last_start_at: Option<DateTime<Utc>>,
    pub source: String,
    pub managed_by: String,
    pub client_id: Option<String>,
    pub run_id: Option<String>,
}

impl ProxyInfo {
    pub fn minimal(name: &str, proxy_type: &str) -> Self {
        Self {
            name: name.into(),
            proxy_type: proxy_type.into(),
            local_addr: String::new(),
            remote_port: None,
            custom_domains: Vec::new(),
            subdomain: String::new(),
            status: "unknown".into(),
            error: None,
            cur_conns: 0,
            traffic_in: None,
            traffic_out: None,
            last_active_at: None,
            last_start_at: None,
            source: "config".into(),
            managed_by: "config_file".into(),
            client_id: None,
            run_id: None,
        }
    }
}

/// 创建/更新代理的入参。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxySpec {
    pub name: String,
    #[serde(rename = "type", default = "default_proxy_type")]
    pub proxy_type: String,
    #[serde(default)]
    pub local_addr: String,
    #[serde(default)]
    pub local_ip: Option<String>,
    #[serde(default)]
    pub local_port: Option<u16>,
    #[serde(default)]
    pub remote_port: Option<u16>,
    #[serde(default)]
    pub custom_domains: Vec<String>,
    #[serde(default)]
    pub subdomain: String,
    #[serde(default)]
    pub secret_key: Option<String>,
    #[serde(default)]
    pub allow_users: Vec<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub server_user: Option<String>,
    #[serde(default)]
    pub bind_addr: Option<String>,
    #[serde(default)]
    pub bind_port: Option<u16>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub use_encryption: bool,
    #[serde(default)]
    pub use_compression: bool,
    /// 原始 TOML 片段（高级用户直接传）
    #[serde(default)]
    pub raw_toml: Option<String>,
}

fn default_proxy_type() -> String {
    "tcp".into()
}

impl ProxySpec {
    /// 归一化本地地址：`local_addr` 优先，否则由 ip+port 拼。
    pub fn effective_local_addr(&self) -> String {
        if !self.local_addr.trim().is_empty() {
            return self.local_addr.trim().to_string();
        }
        match (&self.local_ip, self.local_port) {
            (Some(ip), Some(port)) => format!("{ip}:{port}"),
            (None, Some(port)) => format!("127.0.0.1:{port}"),
            _ => String::new(),
        }
    }

    pub fn split_local_addr(&self) -> (String, u16) {
        let a = self.effective_local_addr();
        if let Some((ip, port)) = a.rsplit_once(':') {
            (ip.to_string(), port.parse().unwrap_or(0))
        } else {
            (a, 0)
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("代理名不能为空".into());
        }
        if self.name.len() > 128 {
            return Err("代理名过长（≤128）".into());
        }
        if !matches!(
            self.proxy_type.as_str(),
            "tcp" | "udp" | "http" | "https" | "tcpmux" | "stcp" | "xtcp" | "sudp"
        ) {
            return Err(format!("不支持的代理类型：{}", self.proxy_type));
        }
        match self.proxy_type.as_str() {
            "tcp" | "udp" => {
                if self.effective_local_addr().is_empty() {
                    return Err("tcp/udp 代理必须指定 local_addr（或 localIP+localPort）".into());
                }
                if self.remote_port.unwrap_or(0) == 0 {
                    return Err("tcp/udp 代理必须指定 remote_port".into());
                }
            }
            "http" | "https" | "tcpmux" => {
                if self.effective_local_addr().is_empty() {
                    return Err("http/https 代理必须指定 local_addr".into());
                }
                if self.custom_domains.is_empty() && self.subdomain.is_empty() {
                    return Err("http/https 代理必须指定 custom_domains 或 subdomain".into());
                }
            }
            "stcp" | "xtcp" | "sudp" => {
                if self.effective_local_addr().is_empty() {
                    return Err("stcp/xtcp/sudp 代理必须指定 local_addr".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 配置文档
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl Diagnostic {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            level: "error".into(),
            code: code.into(),
            message: message.into(),
            hint: None,
            line: None,
        }
    }
    pub fn warn(code: &str, message: impl Into<String>) -> Self {
        Self {
            level: "warning".into(),
            code: code.into(),
            message: message.into(),
            hint: None,
            line: None,
        }
    }
    pub fn info(code: &str, message: impl Into<String>) -> Self {
        Self {
            level: "info".into(),
            code: code.into(),
            message: message.into(),
            hint: None,
            line: None,
        }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedConfig {
    pub ok: bool,
    pub role: InstanceRole,
    pub proxies: Vec<ProxySpec>,
    pub visitors: Vec<ProxySpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDoc {
    pub content: String,
    pub path: String,
    pub sha256: String,
    pub parsed: ParsedConfig,
    pub diagnostics: Vec<Diagnostic>,
    pub editable: bool,
    pub editable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub ok: bool,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    pub hints: Vec<Diagnostic>,
    pub checked_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: String,
    pub path: String,
    pub created_at: DateTime<Utc>,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadOutcome {
    Hot,
    RestartRequired,
    Unsupported,
    Restarted,
}

impl ReloadOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::RestartRequired => "restart_required",
            Self::Unsupported => "unsupported",
            Self::Restarted => "restarted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigWriteResult {
    pub applied: ReloadOutcome,
    pub backup: Option<BackupInfo>,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyOpResult {
    pub applied_via: String,
    pub reload: ReloadOutcome,
    pub proxy: ProxyInfo,
    pub warnings: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// 日志
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub ts: Option<String>,
    pub level: Option<String>,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LogQuery {
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub tail: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    pub total: Option<usize>,
    pub next_offset: Option<usize>,
    pub source: String,
    pub path: String,
}

/// 从一行日志里尽力解析时间戳与级别（两边格式不同，都覆盖）。
pub fn parse_log_line(seq: u64, raw: &str) -> LogLine {
    let mut ts = None;
    let mut level = None;

    // NFrp:-11-03T13:47:04.404472Z  INFO 消息
    // 官方: 2026-11-03 21:49:24.469 [I] [frps/root.go:124] 消息
    if raw.len() > 20 {
        let head = &raw[..32.min(raw.len())];
        let first = head.split_whitespace().next().unwrap_or("");
        if first.len() >= 20 && first.as_bytes().get(4) == Some(&b'-') {
            ts = Some(first.trim_end_matches(&[',', ':'][..]).to_string());
        }
    }
    for lv in ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"] {
        if raw.contains(&format!(" {lv} ")) || raw.contains(&format!("  {lv} ")) {
            level = Some(lv.to_string());
            break;
        }
    }
    if level.is_none() {
        for (tag, lv) in [
            ("[T]", "TRACE"),
            ("[D]", "DEBUG"),
            ("[I]", "INFO"),
            ("[W]", "WARN"),
            ("[E]", "ERROR"),
        ] {
            if raw.contains(tag) {
                level = Some(lv.to_string());
                break;
            }
        }
    }

    LogLine {
        seq,
        ts,
        level,
        raw: raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_spec_validation() {
        let mut p = ProxySpec {
            name: "web".into(),
            proxy_type: "tcp".into(),
            local_addr: "127.0.0.1:8080".into(),
            remote_port: Some(6000),
            ..Default::default()
        };
        assert!(p.validate().is_ok());

        p.remote_port = None;
        assert!(p.validate().is_err());

        let http = ProxySpec {
            name: "h".into(),
            proxy_type: "http".into(),
            local_addr: "127.0.0.1:80".into(),
            ..Default::default()
        };
        assert!(http.validate().is_err());
    }

    #[test]
    fn local_addr_prefers_combined_form() {
        let p = ProxySpec {
            local_ip: Some("10.0.0.1".into()),
            local_port: Some(9000),
            ..Default::default()
        };
        assert_eq!(p.effective_local_addr(), "10.0.0.1:9000");
        assert_eq!(p.split_local_addr(), ("10.0.0.1".to_string(), 9000));
    }

    #[test]
    fn parses_both_log_formats() {
        let a = parse_log_line(1, "2026-11-03T13:47:04.404472Z  INFO nfrp-server 已启动");
        assert_eq!(a.level.as_deref(), Some("INFO"));
        assert!(a.ts.unwrap().starts_with("2026-11-03T13:47:04"));

        let b = parse_log_line(2, "2026-11-03 21:49:24.469 [I] [frps/root.go:124] frps started");
        assert_eq!(b.level.as_deref(), Some("INFO"));
    }

    #[test]
    fn driver_kind_roundtrip() {
        for k in [
            DriverKind::Binary,
            DriverKind::AdminApi,
            DriverKind::SshBinary,
            DriverKind::Embedded,
        ] {
            assert_eq!(DriverKind::parse(k.as_str()), Some(k));
        }
    }
}
