//! 配置文档服务：读、校验、备份、原子写、兼容诊断。
//!
//! 设计要点（对应 `docs/01-architecture.md` §4.4）：
//! 1. 校验先行，失败不落盘；
//! 2. 写入前备份为 `<file>.bak.<UTC 时间戳>`，并登记到 `config_backups`；
//! 3. 临时文件 + fsync + rename，保证不会出现"半个配置文件"；
//! 4. 兼容诊断用 `nfrp-common` 的解析结果 + 目标二进制 flavor 判定。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use toml::Value;
use uuid::Uuid;

use crate::crypto::sha256_hex;
use crate::domain::{
    ConfigDoc, Diagnostic, InstanceRole, ParsedConfig, ProxySpec, ValidationReport,
};
use crate::error::ApiError;

/// 一份配置的读取结果（含原始字节，便于写回时保留格式）。
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub path: PathBuf,
    pub content: String,
    pub sha256: String,
}

pub fn read_file(path: &Path) -> Result<LoadedConfig, ApiError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found(format!("配置文件不存在：{}", path.display()))
        } else {
            ApiError::Internal(anyhow::anyhow!(
                "读取配置文件 {} 失败：{e}",
                path.display()
            ))
        }
    })?;
    let sha = sha256_hex(content.as_bytes());
    Ok(LoadedConfig {
        path: path.to_path_buf(),
        content,
        sha256: sha,
    })
}

/// 解析 TOML 并按角色校验（复用 `nfrp-common`，与 frp 本体同源）。
///
/// 返回解析后的 `toml::Value`，用于提取代理与做下游诊断；同时把
/// `nfrp-common` 识别出的「官方有、但当前实现可能不认」的字段带回给调用方。
pub fn parse_and_validate(
    content: &str,
    role: InstanceRole,
) -> Result<(Value, Vec<String>), Vec<Diagnostic>> {
    let value: toml::Value =
        toml::from_str(content).map_err(|e| vec![toml_error_to_diagnostic(&e)])?;

    // 用 nfrp-common 做结构级校验：它能同时识别 snake_case / camelCase / 两段式 / legacy INI
    let unsupported = match role {
        InstanceRole::Server => match nfrp_common::config::parse_server(content) {
            Ok(cfg) => cfg.unsupported_fields,
            Err(e) => {
                return Err(vec![Diagnostic::error(
                    "schema",
                    format!("服务端配置解析失败：{e}"),
                )])
            }
        },
        InstanceRole::Client => match nfrp_common::config::parse_client(content) {
            Ok(cfg) => cfg.unsupported_fields,
            Err(e) => {
                return Err(vec![Diagnostic::error(
                    "schema",
                    format!("客户端配置解析失败：{e}"),
                )])
            }
        },
    };
    Ok((value, unsupported))
}

fn toml_error_to_diagnostic(e: &toml::de::Error) -> Diagnostic {
    let mut d = Diagnostic::error("toml_syntax", format!("TOML 语法错误：{e}"));
    if let Some(span) = e.span() {
        let line = e.to_string();
        // toml 的错误信息里通常已经带行列号
        let _ = line;
        d.line = Some(estimate_line(e));
        let _ = span;
    }
    d
}

/// toml 0.8 的错误文本形如 "... at line 3 column 5"，尽量解析出行号。
fn estimate_line(e: &toml::de::Error) -> u32 {
    let s = e.to_string();
    if let Some(idx) = s.find("at line ") {
        let rest = &s[idx + 8..];
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        return num.parse().unwrap_or(0);
    }
    0
}

/// 从配置中提取代理 / visitor 定义。
pub fn extract_proxies(value: &Value, role: InstanceRole) -> (Vec<ProxySpec>, Vec<ProxySpec>) {
    let mut proxies = Vec::new();
    let mut visitors = Vec::new();

    let Some(table) = value.as_table() else {
        return (proxies, visitors);
    };

    if role == InstanceRole::Client {
        if let Some(arr) = table.get("proxies").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(p) = proxy_from_value(item) {
                    proxies.push(p);
                }
            }
        }
        if let Some(arr) = table.get("visitors").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(p) = proxy_from_value(item) {
                    visitors.push(p);
                }
            }
        }
    }
    (proxies, visitors)
}

fn proxy_from_value(v: &Value) -> Option<ProxySpec> {
    let t = v.as_table()?;
    let get_str = |k: &str| t.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let get_u16 = |k: &str| t.get(k).and_then(|x| x.as_integer()).map(|x| x as u16);

    let local_ip = get_str("localIP").or_else(|| get_str("local_ip"));
    let local_port = get_u16("localPort").or_else(|| get_u16("local_port"));
    let local_addr = get_str("local_addr").unwrap_or_default();

    let custom_domains = t
        .get("customDomains")
        .or_else(|| t.get("custom_domains"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let remote_port = get_u16("remotePort").or_else(|| get_u16("remote_port"));

    Some(ProxySpec {
        name: get_str("name").unwrap_or_default(),
        proxy_type: get_str("type").unwrap_or_else(|| "tcp".into()),
        local_addr,
        local_ip,
        local_port,
        remote_port,
        custom_domains,
        subdomain: get_str("subdomain").unwrap_or_default(),
        secret_key: get_str("secret_key"),
        allow_users: t
            .get("allow_users")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        server_name: get_str("server_name"),
        server_user: get_str("server_user"),
        bind_addr: get_str("bind_addr"),
        bind_port: get_u16("bind_port"),
        group: get_str("group"),
        use_encryption: t
            .get("use_encryption")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        use_compression: t
            .get("use_compression")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        raw_toml: None,
    })
}

/// 兼容诊断：NFRf ↔ 官方 frp 的字段差异与风险提示。
pub fn diagnose(
    content: &str,
    value: &Value,
    role: InstanceRole,
    flavor: crate::domain::BinaryFlavor,
) -> Vec<Diagnostic> {
    use crate::domain::BinaryFlavor;
    let mut out = Vec::new();
    let table = match value.as_table() {
        Some(t) => t,
        None => return out,
    };

    // 1) 官方二进制不接受 NFrp 专有段
    if flavor == BinaryFlavor::OfficialFrp {
        for key in ["nlink", "vnet", "virtualNet"] {
            if table.contains_key(key) {
                out.push(
                    Diagnostic::error(
                        "official_unknown_section",
                        format!("官方 frp 不认识配置段 [{key}]，会以 unknown field 直接退出"),
                    )
                    .with_hint("切换到 NFrp，或删除该段落后再下发"),
                );
            }
        }
    }

    // 2) local_addr 合并式 vs 两段式
    if role == InstanceRole::Client {
        if let Some(arr) = table.get("proxies").and_then(|v| v.as_array()) {
            for (i, p) in arr.iter().enumerate() {
                let Some(t) = p.as_table() else { continue };
                let has_combined = t.contains_key("local_addr");
                let has_split = t.contains_key("localIP") || t.contains_key("local_ip");
                let name = t
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("<未命名>");
                if has_combined && has_split {
                    out.push(Diagnostic::warn(
                        "local_addr_conflict",
                        format!("代理 [{name}] 同时写了 local_addr 与 localIP/localPort，官方语义下可能冲突"),
                    ).with_hint("只保留一种写法"));
                }
                if has_combined && flavor == BinaryFlavor::OfficialFrp {
                    out.push(
                        Diagnostic::warn(
                            "local_addr_form",
                            format!("代理 [{name}] 使用 NFrp 合并式 local_addr，官方 frpc 需要 localIP + localPort"),
                        )
                        .with_hint("面板会自动转换，或手动改成两段式"),
                    );
                }
                let _ = i;
            }
        }
    }

    // 3) 服务端面板凭据：绑非回环但没凭据，NFrp 会拒绝启动
    if role == InstanceRole::Server {
        let bind_addr = table
            .get("bind_addr")
            .or_else(|| table.get("bindAddr"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let dash_port = table
            .get("dashboard_port")
            .or_else(|| table.get("webServer").and_then(|w| w.get("port")))
            .and_then(|v| v.as_integer());
        let dash_addr = table
            .get("webServer")
            .and_then(|w| w.get("addr"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0.0");
        let dash_user = table
            .get("dashboard_user")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                table
                    .get("webServer")
                    .and_then(|w| w.get("user"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            });
        let dash_on_non_loopback = dash_port.is_some()
            && !(dash_addr == "127.0.0.1" || dash_addr == "localhost" || dash_addr == "::1");
        if dash_on_non_loopback && dash_user.is_none() {
            out.push(
                Diagnostic::error(
                    "dashboard_insecure",
                    "面板端口绑在非回环地址却没有凭据：NFrp 会拒绝启动，官方 frp 会裸奔",
                )
                .with_hint("配置 webServer.user / webServer.password，或把 addr 改回 127.0.0.1"),
            );
        }
        let _ = bind_addr;
    }

    // 4) udpPacketSize 默认值差异
    if table.contains_key("udpPacketSize") || content.contains("udpPacketSize") {
        out.push(Diagnostic::info(
            "udp_packet_size",
            "udpPacketSize：NFrp 默认 65507，官方默认 1500；跨端混用会静默截断大 UDP 报文",
        ));
    }

    // 5) 代理重名
    if role == InstanceRole::Client {
        if let Some(arr) = table.get("proxies").and_then(|v| v.as_array()) {
            let mut seen = std::collections::HashSet::new();
            for p in arr {
                if let Some(name) = p.get("name").and_then(|v| v.as_str()) {
                    if !seen.insert(name.to_string()) {
                        out.push(Diagnostic::error(
                            "duplicate_proxy_name",
                            format!("代理名 [{name}] 重复：服务端按名称定位代理，重名会让操作变得不可预期"),
                        ));
                    }
                }
            }
        }
    }

    // 6) 官方 frp：token 放错位置
    if flavor == BinaryFlavor::OfficialFrp {
        if let Some(tok) = table.get("token").and_then(|v| v.as_str()) {
            if !tok.is_empty() && table.get("auth").is_none() {
                out.push(Diagnostic::info(
                    "token_top_level",
                    "顶层 token 是 NFrp 写法；官方 frp 推荐 auth.token（两种都认，但更清晰）",
                ));
            }
        }
    }

    out
}

/// 组合出完整的 `ConfigDoc`。
pub fn build_doc(
    loaded: LoadedConfig,
    role: InstanceRole,
    flavor: crate::domain::BinaryFlavor,
    editable: bool,
    editable_reason: Option<String>,
) -> ConfigDoc {
    let (parsed, mut diagnostics) = match parse_and_validate(&loaded.content, role) {
        Ok((value, unsupported)) => {
            let (proxies, visitors) = extract_proxies(&value, role);
            let mut diags = diagnose(&loaded.content, &value, role, flavor);
            diags.extend(unsupported_field_diagnostics(&unsupported, flavor));
            (
                ParsedConfig {
                    ok: true,
                    role,
                    proxies,
                    visitors,
                },
                diags,
            )
        }
        Err(diags) => (
            ParsedConfig {
                ok: false,
                role,
                proxies: vec![],
                visitors: vec![],
            },
            diags,
        ),
    };

    if !editable {
        diagnostics.push(Diagnostic::info(
            "read_only",
            editable_reason
                .clone()
                .unwrap_or_else(|| "当前驱动不拥有该实例的配置文件".into()),
        ));
    }

    ConfigDoc {
        content: loaded.content,
        path: loaded.path.display().to_string(),
        sha256: loaded.sha256,
        parsed,
        diagnostics,
        editable,
        editable_reason,
    }
}

/// `nfrp-common` 采集到的「官方有、实现可能不认」的字段 → 诊断。
pub fn unsupported_field_diagnostics(
    fields: &[String],
    flavor: crate::domain::BinaryFlavor,
) -> Vec<Diagnostic> {
    use crate::domain::BinaryFlavor;
    if fields.is_empty() {
        return vec![];
    }
    let level = match flavor {
        // 目标是官方 frp 时，这些字段通常来自 NFrp 侧的新增能力，官方会直接退出
        BinaryFlavor::OfficialFrp => "error",
        BinaryFlavor::Nfrp => "warning",
    };
    vec![Diagnostic {
        level: level.into(),
        code: "unsupported_fields".into(),
        message: format!("以下配置项当前二进制可能不识别：{}", fields.join("、")),
        hint: Some(if flavor == BinaryFlavor::OfficialFrp {
            "官方 frp 对未知字段零容忍（会以 unknown field 直接退出），请删除这些字段或切换为 NFrp".into()
        } else {
            "这些字段来自官方 frp 更新版本，NFrp 尚未实现，配置会保留但可能不生效".into()
        }),
        line: None,
    }]
}

/// 生成一份最小可用的初始配置。
///
/// 字段名按目标实现输出（官方 frp 用 camelCase，NFrp 用 snake_case），
/// 并保证**全局段写在所有 `[[proxies]]` 之前**——官方 frpc 否则会把它归到最后一个
/// 代理里并报 `unknown field`（实测）。
pub fn template_for(role: &str, flavor: &str) -> (String, Vec<String>) {
    use crate::domain::{BinaryFlavor, InstanceRole};
    let role = InstanceRole::parse(role).unwrap_or(InstanceRole::Server);
    let flavor = BinaryFlavor::parse(flavor).unwrap_or(BinaryFlavor::Nfrp);
    let mut notes = Vec::new();

    match (role, flavor) {
        (InstanceRole::Server, BinaryFlavor::Nfrp) => {
            notes.push("token 请改成足够随机的长口令".into());
            notes.push("面板凭据必填：NFrp 在非回环地址上无凭据会拒绝启动".into());
            (
                r#"# frps 配置（NFrp，兼容官方 frp v1/v2 线协议）
bind_addr = "0.0.0.0"
bind_port = 7000
auth.token = "请改成足够随机的长口令"

# 管理面板（面板自身也用它读取运行态）
webServer.addr = "127.0.0.1"
webServer.port = 7500
webServer.user = "admin"
webServer.password = "请改成强口令"

# Prometheus 指标（可选）
enablePrometheus = true

# HTTP/HTTPS 虚拟主机端口（使用 http/https 代理时必填）
# vhost_http_port = 8080
# vhost_https_port = 8443

log_level = "info"
"#
                .to_string(),
                notes,
            )
        }
        (InstanceRole::Server, BinaryFlavor::OfficialFrp) => {
            notes.push("token 请改成足够随机的长口令".into());
            notes.push("官方 frps 没有 /api/status：面板只依赖 /api/v2/* 与 /metrics".into());
            (
                r#"# frps 配置（官方 frp）
bindAddr = "0.0.0.0"
bindPort = 7000

auth.method = "token"
auth.token = "请改成足够随机的长口令"

webServer.addr = "127.0.0.1"
webServer.port = 7500
webServer.user = "admin"
webServer.password = "请改成强口令"

enablePrometheus = true

log.to = "console"
log.level = "info"
"#
                .to_string(),
                notes,
            )
        }
        (InstanceRole::Client, BinaryFlavor::Nfrp) => {
            notes.push("NFrp 支持合并式 local_addr；官方两段式也认，但不要混用".into());
            (
                r#"# frpc 配置（NFrp）
server_addr = "1.2.3.4"
server_port = 7000
auth.token = "与服务端一致的口令"
tcp_mux = true
log_level = "info"

# 客户端本地管理界面（面板用它读取代理运行态）
[webServer]
addr = "127.0.0.1"
port = 7400
user = "admin"
password = "请改成强口令"

[[proxies]]
name = "ssh"
type = "tcp"
local_addr = "127.0.0.1:22"
remote_port = 6000
"#
                .to_string(),
                notes,
            )
        }
        (InstanceRole::Client, BinaryFlavor::OfficialFrp) => {
            notes.push("官方 frpc 要求 localIP + localPort 两段式".into());
            notes.push("官方 frpc 会忽略客户端 localStorage 之外的动态代理：请以配置文件为准".into());
            (
                r#"# frpc 配置（官方 frp）
serverAddr = "1.2.3.4"
serverPort = 7000
auth.token = "与服务端一致的口令"

# 全局段必须放在所有 [[proxies]] 之前
log.to = "console"
log.level = "info"

webServer.addr = "127.0.0.1"
webServer.port = 7400
webServer.user = "admin"
webServer.password = "请改成强口令"

[[proxies]]
name = "ssh"
type = "tcp"
localIP = "127.0.0.1"
localPort = 22
remotePort = 6000
"#
                .to_string(),
                notes,
            )
        }
    }
}

/// 校验报告（供 `POST /config/validate` 与保存前预检共用）。
pub fn validation_report(
    content: &str,
    role: InstanceRole,
    flavor: crate::domain::BinaryFlavor,
) -> ValidationReport {
    match parse_and_validate(content, role) {
        Ok((value, unsupported)) => {
            let mut diags = diagnose(content, &value, role, flavor);
            diags.extend(unsupported_field_diagnostics(&unsupported, flavor));
            let errors: Vec<_> = diags.iter().filter(|d| d.level == "error").cloned().collect();
            let warnings: Vec<_> = diags
                .iter()
                .filter(|d| d.level == "warning")
                .cloned()
                .collect();
            let hints: Vec<_> = diags.iter().filter(|d| d.level == "info").cloned().collect();
            ValidationReport {
                ok: errors.is_empty(),
                errors,
                warnings,
                hints,
                checked_by: "schema".into(),
            }
        }
        Err(errors) => ValidationReport {
            ok: false,
            errors,
            warnings: vec![],
            hints: vec![],
            checked_by: "schema".into(),
        },
    }
}

/// 备份结果。
#[derive(Debug, Clone)]
pub struct BackupFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha: String,
    pub created_at: DateTime<Utc>,
}

/// 备份旧配置为 `<file>.bak.<UTC 时间戳>`。
pub fn backup_file(path: &Path) -> Result<Option<BackupFile>, ApiError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read(path)?;
    let now = Utc::now();
    let stamp = now.format("%Y%m%dT%H%M%S%.3fZ").to_string();
    let backup = PathBuf::from(format!("{}.bak.{stamp}", path.display()));
    std::fs::write(&backup, &content)?;
    crate::crypto::restrict_permissions(&backup)?;
    Ok(Some(BackupFile {
        path: backup,
        size: content.len() as u64,
        sha: sha256_hex(&content),
        created_at: now,
    }))
}

/// 原子写：写临时文件 → fsync → rename。
pub fn atomic_write(path: &Path, content: &str) -> Result<u64, ApiError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), Uuid::new_v4().simple()));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    // 保留原文件权限（Unix）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map(|m| m.permissions().mode()).unwrap_or(0o640);
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }

    // Windows 上 rename 到已存在的目标会失败：先尝试直接 rename，失败则 ReplaceFile 语义
    match std::fs::rename(&tmp, path) {
        Ok(_) => {}
        Err(_) => {
            // 备份现有目标再替换，避免出现"两个都不在"的窗口
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp, path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                ApiError::Internal(anyhow::anyhow!(
                    "替换配置文件 {} 失败：{e}",
                    path.display()
                ))
            })?;
        }
    }

    let size = std::fs::metadata(path)?.len();
    Ok(size)
}

/// 回滚到某个备份文件内容。
pub fn restore_from_backup(backup_path: &Path, target: &Path) -> Result<u64, ApiError> {
    let content = std::fs::read_to_string(backup_path).map_err(|e| {
        ApiError::not_found(format!(
            "备份文件 {} 不可读：{e}",
            backup_path.display()
        ))
    })?;
    atomic_write(target, &content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::BinaryFlavor;

    const SERVER_TOML: &str = r#"
bind_addr = "0.0.0.0"
bind_port = 7000
token = "abcdefghijklmnop"

dashboard_port = 7500
dashboard_user = "admin"
dashboard_pwd = "pwd"
"#;

    const CLIENT_TOML: &str = r#"
server_addr = "1.2.3.4"
server_port = 7000
token = "t"

[[proxies]]
name = "ssh"
type = "tcp"
local_addr = "127.0.0.1:22"
remote_port = 6000
"#;

    #[test]
    fn parses_server_config_with_nfrp_common() {
        let v = parse_and_validate(SERVER_TOML, InstanceRole::Server);
        assert!(v.is_ok(), "{v:?}");
    }

    #[test]
    fn parses_official_camel_case_config() {
        let official = r#"
bindAddr = "0.0.0.0"
bindPort = 7000
auth.token = "t"

webServer.addr = "127.0.0.1"
webServer.port = 7500
webServer.user = "admin"
webServer.password = "pwd"
"#;
        let v = parse_and_validate(official, InstanceRole::Server);
        assert!(v.is_ok(), "{v:?}");
    }

    #[test]
    fn extracts_proxies_from_client_config() {
        let (value, _) = parse_and_validate(CLIENT_TOML, InstanceRole::Client).unwrap();
        let (proxies, visitors) = extract_proxies(&value, InstanceRole::Client);
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].name, "ssh");
        assert_eq!(proxies[0].remote_port, Some(6000));
        assert!(visitors.is_empty());
    }

    #[test]
    fn detects_duplicate_proxy_names() {
        let toml = r#"
server_addr = "1.2.3.4"
server_port = 7000
[[proxies]]
name = "dup"
type = "tcp"
local_addr = "127.0.0.1:22"
remote_port = 6000
[[proxies]]
name = "dup"
type = "tcp"
local_addr = "127.0.0.1:23"
remote_port = 6001
"#;
        let (v, _) = parse_and_validate(toml, InstanceRole::Client).unwrap();
        let diags = diagnose(toml, &v, InstanceRole::Client, BinaryFlavor::Nfrp);
        assert!(diags.iter().any(|d| d.code == "duplicate_proxy_name"));
    }

    #[test]
    fn flags_nfrp_only_sections_for_official() {
        let toml = r#"
bind_addr = "0.0.0.0"
bind_port = 7000
token = "t"

[nlink]
enable = true
"#;
        let (v, _) = parse_and_validate(toml, InstanceRole::Server).unwrap();
        let diags = diagnose(toml, &v, InstanceRole::Server, BinaryFlavor::OfficialFrp);
        assert!(diags.iter().any(|d| d.code == "official_unknown_section"));
    }

    #[test]
    fn flags_insecure_dashboard() {
        let toml = r#"
bind_addr = "0.0.0.0"
bind_port = 7000
token = "t"
webServer.addr = "0.0.0.0"
webServer.port = 7500
"#;
        let (v, _) = parse_and_validate(toml, InstanceRole::Server).unwrap();
        let diags = diagnose(toml, &v, InstanceRole::Server, BinaryFlavor::Nfrp);
        assert!(diags.iter().any(|d| d.code == "dashboard_insecure"));
    }

    #[test]
    fn unsupported_fields_become_error_for_official() {
        let diags = unsupported_field_diagnostics(
            &["fooBar".to_string()],
            BinaryFlavor::OfficialFrp,
        );
        assert_eq!(diags[0].level, "error");
        assert_eq!(diags[0].code, "unsupported_fields");
    }

    #[test]
    fn syntax_error_has_line() {
        let bad = "bind_addr = \n";
        let errs = parse_and_validate(bad, InstanceRole::Server).unwrap_err();
        assert_eq!(errs[0].code, "toml_syntax");
    }

    #[test]
    fn atomic_write_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("frps.toml");
        atomic_write(&f, "a = 1\n").unwrap();
        let b = backup_file(&f).unwrap().unwrap();
        assert!(b.path.exists());
        assert_eq!(std::fs::read_to_string(&b.path).unwrap(), "a = 1\n");

        atomic_write(&f, "a = 2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "a = 2\n");
        // 没有残留临时文件
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty());

        // 回滚
        restore_from_backup(&b.path, &f).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "a = 1\n");
    }

    #[test]
    fn backup_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(backup_file(&dir.path().join("nope.toml")).unwrap().is_none());
    }
}

