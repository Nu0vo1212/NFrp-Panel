//! 部署预检：在真正动手之前把能查的都查一遍。
//!
//! 预检项对齐需求第七节 2：
//! OS/架构、systemd、sudo、磁盘空间、已有 frp、端口占用、二进制架构匹配。

use serde::{Deserialize, Serialize};

use crate::domain::BinaryFlavor;
use crate::error::ApiError;
use crate::ssh::probe::RemoteEnvironment;
use crate::ssh::session::SshSession;
use crate::ssh::exec::{self, AllowedCommand, SudoPrefix};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightItem {
    pub key: String,
    pub label: String,
    /// pass / warn / fail / skip
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl PreflightItem {
    fn pass(key: &str, label: &str, message: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            status: "pass".into(),
            message: message.into(),
            remediation: None,
        }
    }
    fn warn(key: &str, label: &str, message: impl Into<String>, hint: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            status: "warn".into(),
            message: message.into(),
            remediation: Some(hint.into()),
        }
    }
    fn fail(key: &str, label: &str, message: impl Into<String>, hint: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            status: "fail".into(),
            message: message.into(),
            remediation: Some(hint.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub ok: bool,
    pub items: Vec<PreflightItem>,
    pub environment: RemoteEnvironment,
}

pub struct PreflightInput<'a> {
    pub flavor: BinaryFlavor,
    pub arch_expected: &'a str,
    pub install_dir: &'a str,
    pub config_dir: &'a str,
    pub log_dir: &'a str,
    pub config_port: Option<u16>,
    pub admin_api_url: &'a str,
    pub service_mode_systemd: bool,
    /// 上传的二进制内容（用于架构校验；None 表示走 URL 下载，暂不校验）
    pub binary_bytes: Option<&'a [u8]>,
}

/// 执行预检。
pub async fn run(
    session: &SshSession,
    env: &RemoteEnvironment,
    input: &PreflightInput<'_>,
) -> Result<PreflightReport, ApiError> {
    let sudo = SudoPrefix::default();
    let mut items = Vec::new();

    // 1) OS
    if env.os.eq_ignore_ascii_case("Linux") {
        items.push(PreflightItem::pass("os", "操作系统", format!("{} {}", env.os, env.kernel)));
    } else {
        items.push(PreflightItem::fail(
            "os",
            "操作系统",
            format!("当前探测到 {}，面板的 systemd/nohup 方案面向 Linux", env.os),
            "如目标为 Windows，请改用本机二进制驱动",
        ));
    }

    // 2) 架构
    let arch = crate::ssh::probe::normalize_arch(&env.arch);
    if arch == input.arch_expected {
        items.push(PreflightItem::pass("arch", "CPU 架构", arch.clone()));
    } else {
        items.push(PreflightItem::warn(
            "arch",
            "CPU 架构",
            format!("目标机为 {}，所选二进制为 {}", arch, input.arch_expected),
            "请选择与目标架构一致的安装包（否则会 exec format error）",
        ));
    }

    // 3) 二进制与架构匹配（有字节内容时）
    if let Some(bytes) = input.binary_bytes {
        match crate::deploy::catalog::elf_machine(bytes) {
            Some(m) if m == arch => items.push(PreflightItem::pass(
                "binary_arch",
                "二进制架构",
                format!("ELF 头匹配 {m}"),
            )),
            Some(m) => items.push(PreflightItem::fail(
                "binary_arch",
                "二进制架构",
                format!("二进制是 {m}，目标机是 {arch}"),
                "换成与目标架构一致的二进制",
            )),
            None => {
                if crate::deploy::catalog::is_pe(bytes) {
                    items.push(PreflightItem::fail(
                        "binary_arch",
                        "二进制架构",
                        "这是一个 Windows 可执行文件，不能部署到 Linux",
                        "选择 Linux 平台的安装包",
                    ));
                } else {
                    items.push(PreflightItem::warn(
                        "binary_arch",
                        "二进制架构",
                        "无法从文件头识别架构（不是 ELF）",
                        "确认上传的是 Linux 平台的可执行文件",
                    ));
                }
            }
        }
    }

    // 4) systemd
    if env.systemd {
        items.push(PreflightItem::pass("systemd", "systemd", "可用"));
    } else if input.service_mode_systemd {
        items.push(PreflightItem::warn(
            "systemd",
            "systemd",
            "目标机没有 systemd，将回退到 nohup 守护模式",
            "非 systemd 模式下服务不会开机自启，重启后需手动拉起",
        ));
    } else {
        items.push(PreflightItem::pass("systemd", "systemd", "未使用（nohup 模式）"));
    }

    // 5) sudo
    match env.sudo.as_str() {
        "passwordless" => items.push(PreflightItem::pass(
            "sudo",
            "sudo 权限",
            format!("{} 可免密 sudo", env.current_user),
        )),
        "password" if env.current_user == "root" => items.push(PreflightItem::pass(
            "sudo",
            "sudo 权限",
            "当前为 root，无需 sudo",
        )),
        _ => items.push(PreflightItem::fail(
            "sudo",
            "sudo 权限",
            "sudo 需要密码，面板无法交互输入",
            "为该用户配置 NOPASSWD，或改用「sudo -n」/ root 登录",
        )),
    }

    // 6) 磁盘空间
    if env.disk_free_mb >= 200 {
        items.push(PreflightItem::pass(
            "disk",
            "磁盘空间",
            format!("{} MB 可用", env.disk_free_mb),
        ));
    } else if env.disk_free_mb > 0 {
        items.push(PreflightItem::fail(
            "disk",
            "磁盘空间",
            format!("仅剩 {} MB，不足以安装", env.disk_free_mb),
            "清理磁盘空间后重试",
        ));
    } else {
        items.push(PreflightItem::warn(
            "disk",
            "磁盘空间",
            "无法读取磁盘空间",
            "确认目标目录存在",
        ));
    }

    // 7) 已有安装
    let existing = match input.flavor {
        BinaryFlavor::Nfrp => env.existing.frps.as_ref().or(env.existing.frpc.as_ref()),
        BinaryFlavor::OfficialFrp => env.existing.frps.as_ref().or(env.existing.frpc.as_ref()),
    };
    if let Some(p) = existing {
        items.push(PreflightItem::warn(
            "existing",
            "已有安装",
            format!("检测到已安装：{p}"),
            "部署会先备份旧二进制与配置；如需保留请确认版本一致",
        ));
    } else {
        items.push(PreflightItem::pass("existing", "已有安装", "未检测到冲突"));
    }

    // 8) 目录可写（覆盖/创建检查）
    for (key, label, dir) in [
        ("install_dir", "安装目录", input.install_dir),
        ("config_dir", "配置目录", input.config_dir),
        ("log_dir", "日志目录", input.log_dir),
    ] {
        let parent = std::path::Path::new(dir)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".into());
        let out = exec::run(
            session,
            AllowedCommand::DirWritable { path: parent.clone() },
            &sudo,
            None,
        )
        .await;
        match out {
            Ok(o) if o.stdout.trim() == "yes" => {
                items.push(PreflightItem::pass(key, label, format!("{dir} 可写")))
            }
            Ok(_) => items.push(PreflightItem::fail(
                key,
                label,
                format!("{parent} 不可写"),
                "确认 sudo 权限与目录属主",
            )),
            Err(e) => items.push(PreflightItem::warn(
                key,
                label,
                format!("无法检查 {dir}：{e}"),
                "如果目录不存在，部署时会自动创建",
            )),
        }
    }

    // 9) 端口占用
    if let Some(port) = input.config_port {
        let out = exec::run(
            session,
            AllowedCommand::PortListening { port },
            &sudo,
            None,
        )
        .await;
        match out {
            Ok(o) if o.stdout.trim() == "yes" => items.push(PreflightItem::warn(
                "port",
                "控制端口",
                format!("端口 {port} 已被占用"),
                "如果占用者就是旧的 frps，请先停止它；否则换一个端口",
            )),
            Ok(_) => items.push(PreflightItem::pass("port", "控制端口", format!("端口 {port} 空闲"))),
            Err(_) => items.push(PreflightItem::warn(
                "port",
                "控制端口",
                "无法检查端口占用（缺少 ss/netstat）",
                "可忽略；启动阶段会再次验证",
            )),
        }
    }

    // 10) Admin API 可达性（从面板本机访问）
    if !input.admin_api_url.trim().is_empty() {
        let ok = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .build()
            .ok()
            .map(|c| c.get(input.admin_api_url.trim()).send())
            .is_some();
        let _ = ok;
        items.push(PreflightItem::warn(
            "admin_api",
            "Admin API 可达性",
            format!(
                "面板将从本机访问 {}；若该端口只监听 127.0.0.1 会不可达",
                input.admin_api_url
            ),
            "把 webServer.addr 设为 0.0.0.0（并配强口令），或使用 SSH 隧道",
        ));
    }

    let ok = !items.iter().any(|i| i.status == "fail");
    Ok(PreflightReport {
        ok,
        items,
        environment: env.clone(),
    })
}
