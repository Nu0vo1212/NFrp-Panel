//! 面板自身配置（`panel.toml`），支持环境变量覆盖。
//!
//! 安全相关的默认值一律取「最保守」：绑定回环、密钥落 0600 文件、
//! 首次启动生成随机管理员口令而不是固定口令。

use std::{
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const ENV_CONFIG: &str = "FRP_PANEL_CONFIG";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PanelConfig {
    pub server: ServerSection,
    pub database: DatabaseSection,
    pub security: SecuritySection,
    pub defaults: DefaultsSection,
    pub frp: FrpSection,
    pub ssh: SshSection,
    pub audit: AuditSection,
    pub theme: ThemeSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    /// 面板监听地址。默认回环，避免"装完即暴露"。
    pub bind: String,
    pub static_dir: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            static_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseSection {
    pub url: String,
    /// 数据目录（相对路径按配置文件所在目录解析）。
    pub data_dir: String,
}

impl Default for DatabaseSection {
    fn default() -> Self {
        Self {
            url: String::new(), // 空 => sqlite://<data_dir>/panel.db
            data_dir: "data".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecuritySection {
    pub jwt_secret_file: String,
    pub master_key_file: String,
    pub access_token_ttl_minutes: i64,
    pub refresh_token_ttl_days: i64,
    /// 首次启动时的管理员用户名；口令随机生成写入 `data/admin.initial`。
    pub initial_admin_username: String,
    /// 可选的固定初始口令（不推荐）。为空则随机生成。
    pub initial_admin_password: String,
}

impl Default for SecuritySection {
    fn default() -> Self {
        Self {
            jwt_secret_file: "data/jwt.key".into(),
            master_key_file: "data/master.key".into(),
            access_token_ttl_minutes: 120,
            refresh_token_ttl_days: 14,
            initial_admin_username: "admin".into(),
            initial_admin_password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultsSection {
    pub driver: String,
    pub flavor: String,
    pub binary_dir: String,
    pub server_binary: String,
    pub client_binary: String,
    pub config_dir: String,
    pub log_dir: String,
    pub work_dir: String,
}

impl Default for DefaultsSection {
    fn default() -> Self {
        Self {
            driver: "binary".into(),
            flavor: "nfrp".into(),
            binary_dir: "bin".into(),
            server_binary: "frps".into(),
            client_binary: "frpc".into(),
            config_dir: "config".into(),
            log_dir: "logs".into(),
            work_dir: "work".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FrpSection {
    pub admin_timeout_ms: u64,
    pub admin_retry: u32,
    pub status_poll_ms: u64,
    pub metrics_history_days: u32,
    /// 保存配置后若无法热重载，是否自动重启。
    pub auto_restart_on_save: bool,
}

impl Default for FrpSection {
    fn default() -> Self {
        Self {
            admin_timeout_ms: 5_000,
            admin_retry: 2,
            status_poll_ms: 10_000,
            metrics_history_days: 7,
            auto_restart_on_save: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SshSection {
    pub connect_timeout_ms: u64,
    pub command_timeout_ms: u64,
    pub upload_timeout_ms: u64,
    pub retry: u32,
    pub known_hosts_file: String,
    pub default_port: u16,
    pub default_user: String,
    pub default_sudo: String,
    pub default_sudo_cmd: String,
    pub default_install_dir: String,
    pub default_config_dir: String,
    pub default_log_dir: String,
    pub default_service: String,
    /// 显式开启才允许自定义提权命令（仍受白名单前缀约束）。
    pub allow_custom_command: bool,
}

impl Default for SshSection {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 10_000,
            command_timeout_ms: 20_000,
            upload_timeout_ms: 120_000,
            retry: 2,
            known_hosts_file: "data/known_hosts".into(),
            default_port: 22,
            default_user: "root".into(),
            default_sudo: "none".into(),
            default_sudo_cmd: String::new(),
            default_install_dir: "/usr/local/bin".into(),
            default_config_dir: "/etc/nfrp".into(),
            default_log_dir: "/var/log/nfrp".into(),
            default_service: "systemd".into(),
            allow_custom_command: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditSection {
    pub enabled: bool,
    pub retention_days: u32,
}

impl Default for AuditSection {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSection {
    pub default: String,
}

impl Default for ThemeSection {
    fn default() -> Self {
        Self {
            default: "light".into(),
        }
    }
}

/// 运行期配置：附带解析后的绝对路径。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub raw: PanelConfig,
    /// 配置文件的绝对路径（不存在时为默认位置）。
    pub path: PathBuf,
    /// 数据根目录（绝对路径）。
    pub data_dir: PathBuf,
}

impl RuntimeConfig {
    pub fn load(explicit: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = explicit
            .or_else(|| std::env::var_os(ENV_CONFIG).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("panel.toml"));

        let mut raw = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("读取配置文件 {} 失败：{e}", path.display()))?;
            toml::from_str::<PanelConfig>(&text)
                .map_err(|e| anyhow::anyhow!("解析配置文件 {} 失败：{e}", path.display()))?
        } else {
            PanelConfig::default()
        };

        apply_env_overrides(&mut raw);

        // 相对路径以配置文件所在目录为基准（没有配置文件时取当前目录）。
        let base = if path.exists() {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        let data_dir = base.join(&raw.database.data_dir);
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| anyhow::anyhow!("创建数据目录 {} 失败：{e}", data_dir.display()))?;

        Ok(Self {
            raw,
            path: base.join(path.file_name().unwrap_or_default()).clone(),
            data_dir,
        })
    }

    /// 解析相对路径为绝对路径（相对数据根目录）。
    pub fn resolve_data(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.data_dir.join(path)
        }
    }

    /// 解析相对路径为绝对路径（相对配置文件目录）。
    pub fn resolve_base(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.path
                .parent()
                .unwrap_or(Path::new("."))
                .join(path)
        }
    }

    pub fn database_url(&self) -> String {
        if self.raw.database.url.is_empty() {
            let db = self.data_dir.join("panel.db");
            format!("sqlite://{}?mode=rwc", normalize_path(&db))
        } else {
            self.raw.database.url.clone()
        }
    }

    pub fn jwt_secret_file(&self) -> PathBuf {
        self.resolve_base(&self.raw.security.jwt_secret_file)
    }

    pub fn master_key_file(&self) -> PathBuf {
        self.resolve_base(&self.raw.security.master_key_file)
    }

    pub fn known_hosts_file(&self) -> PathBuf {
        self.resolve_base(&self.raw.ssh.known_hosts_file)
    }

    /// 初始管理员口令文件（首个管理员创建后即删除）。
    pub fn initial_admin_password_file(&self) -> PathBuf {
        self.data_dir.join("admin.initial")
    }
}

fn apply_env_overrides(cfg: &mut PanelConfig) {
    if let Ok(v) = std::env::var("FRP_PANEL_BIND") {
        cfg.server.bind = v;
    }
    if let Ok(v) = std::env::var("FRP_PANEL_DATABASE_URL") {
        cfg.database.url = v;
    }
    if let Ok(v) = std::env::var("FRP_PANEL_DATA_DIR") {
        cfg.database.data_dir = v;
    }
    if let Ok(v) = std::env::var("FRP_PANEL_STATIC_DIR") {
        cfg.server.static_dir = v;
    }
    if let Ok(v) = std::env::var("FRP_PANEL_ADMIN_PASSWORD") {
        cfg.security.initial_admin_password = v;
    }
    if let Ok(v) = std::env::var("FRP_PANEL_ADMIN_USERNAME") {
        cfg.security.initial_admin_username = v;
    }
    if let Ok(v) = std::env::var("FRP_PANEL_AUTO_RESTART_ON_SAVE") {
        cfg.frp.auto_restart_on_save = matches!(v.as_str(), "1" | "true" | "yes");
    }
}

/// SQLite URL 里 Windows 的 `\` 要换成 `/`，否则 sqlx 解析失败。
fn normalize_path(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_loopback_and_sane() {
        let c = PanelConfig::default();
        assert!(c.server.bind.starts_with("127.0.0.1"));
        assert_eq!(c.defaults.driver, "binary");
        assert!(!c.ssh.allow_custom_command);
        assert!(c.frp.admin_timeout_ms >= 1000);
    }

    #[test]
    fn parses_partial_toml() {
        let cfg: PanelConfig = toml::from_str(
            r#"
            [server]
            bind = "0.0.0.0:9000"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.server.bind, "0.0.0.0:9000");
        // 未写的段落使用默认值
        assert_eq!(cfg.ssh.default_port, 22);
    }

    #[test]
    fn env_override_wins() {
        std::env::set_var("FRP_PANEL_BIND", "127.0.0.1:19999");
        let mut cfg = PanelConfig::default();
        apply_env_overrides(&mut cfg);
        assert_eq!(cfg.server.bind, "127.0.0.1:19999");
        std::env::remove_var("FRP_PANEL_BIND");
    }
}
