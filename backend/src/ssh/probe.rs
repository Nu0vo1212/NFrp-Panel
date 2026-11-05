//! 远程环境探测：OS/架构、systemd、sudo、磁盘、已有 frp、端口占用。

use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::ssh::exec::{self, AllowedCommand, SudoPrefix};
use crate::ssh::session::SshSession;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteEnvironment {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub hostname: String,
    pub systemd: bool,
    /// available / passwordless / unavailable
    pub sudo: String,
    pub current_user: String,
    pub disk_free_mb: u64,
    pub existing: ExistingInstall,
    pub listening_ports: Vec<u16>,
    /// 探测过程中遇到的问题（不致命）
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExistingInstall {
    pub frps: Option<String>,
    pub frpc: Option<String>,
    pub config: Option<String>,
}

pub struct ProbeOptions<'a> {
    pub config_dir: &'a str,
    pub candidate_ports: &'a [u16],
}

/// 执行探测。所有命令都是只读的，失败也不致命（记录到 notes）。
pub async fn probe(
    session: &SshSession,
    opts: &ProbeOptions<'_>,
) -> Result<RemoteEnvironment, ApiError> {
    let sudo = SudoPrefix::build(crate::domain::SudoMode::None, None, false)?;
    let mut env = RemoteEnvironment::default();

    // OS / 内核 / 架构 / 主机名（一条命令拿全）
    if let Ok(out) = session
        .exec("uname -s; uname -r; uname -m; hostname 2>/dev/null || cat /etc/hostname 2>/dev/null")
        .await
    {
        let mut lines = out.stdout.lines().map(str::trim);
        env.os = lines.next().unwrap_or_default().to_string();
        env.kernel = lines.next().unwrap_or_default().to_string();
        env.arch = lines.next().unwrap_or_default().to_string();
        env.hostname = lines.next().unwrap_or_default().to_string();
    }
    if env.arch.is_empty() {
        return Err(ApiError::upstream(
            "无法获取远程架构信息：SSH 已连接但命令执行异常",
        ));
    }

    // systemd
    if let Ok(out) = exec::run(session, AllowedCommand::HasSystemd, &sudo, None).await {
        env.systemd = out.stdout.trim() == "yes";
    }
    if !env.systemd {
        env.notes
            .push("远程主机没有 systemd，将使用 nohup 进程守护模式".into());
    }

    // 用户与 sudo
    if let Ok(out) = exec::run(session, AllowedCommand::Whoami, &sudo, None).await {
        let text = out.stdout;
        let mut parts = text.lines().map(str::trim);
        env.current_user = parts.next().unwrap_or_default().to_string();
        let sudo_line = parts.next().unwrap_or_default();
        env.sudo = if sudo_line.contains("nopasswd") {
            "passwordless".into()
        } else {
            "password".into()
        };
        if env.sudo == "password" && env.current_user != "root" {
            env.notes.push(
                "sudo 需要密码：请改用 sudo -n 免密配置，或以 root 登录后再部署".into(),
            );
        }
    }

    // 磁盘
    if let Ok(out) = exec::run(
        session,
        AllowedCommand::DiskFreeMb {
            path: opts.config_dir.to_string(),
        },
        &sudo,
        None,
    )
    .await
    {
        env.disk_free_mb = out.stdout.trim().parse().unwrap_or(0);
    } else {
        // 目录可能还不存在，退到根
        if let Ok(out) = exec::run(
            session,
            AllowedCommand::DiskFreeMb { path: "/".into() },
            &sudo,
            None,
        )
        .await
        {
            env.disk_free_mb = out.stdout.trim().parse().unwrap_or(0);
        }
    }

    // 已安装的 frp
    if let Ok(out) = exec::run(session, AllowedCommand::FindFrpBinary, &sudo, None).await {
        for line in out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line.ends_with("frps") && env.existing.frps.is_none() {
                env.existing.frps = Some(line.to_string());
            } else if line.ends_with("frpc") && env.existing.frpc.is_none() {
                env.existing.frpc = Some(line.to_string());
            }
        }
    }
    for candidate in ["frps.toml", "frpc.toml"] {
        let path = format!("{}/{}", opts.config_dir.trim_end_matches('/'), candidate);
        if let Ok(out) = exec::run(
            session,
            AllowedCommand::FileExists { path: path.clone() },
            &sudo,
            None,
        )
        .await
        {
            if out.stdout.trim() == "yes" && env.existing.config.is_none() {
                env.existing.config = Some(path);
            }
        }
    }

    // 端口占用
    for port in opts.candidate_ports {
        if let Ok(out) = exec::run(
            session,
            AllowedCommand::PortListening { port: *port },
            &sudo,
            None,
        )
        .await
        {
            if out.stdout.trim() == "yes" {
                env.listening_ports.push(*port);
            }
        }
    }

    Ok(env)
}

/// 架构名归一：`x86_64`/`amd64` → `amd64`，`aarch64`/`arm64` → `arm64`。
pub fn normalize_arch(arch: &str) -> String {
    match arch.trim().to_lowercase().as_str() {
        "x86_64" | "amd64" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        "armv7l" | "armv7" => "armv7".into(),
        "i386" | "i686" | "x86" => "386".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_arches() {
        assert_eq!(normalize_arch("x86_64"), "amd64");
        assert_eq!(normalize_arch("AMD64"), "amd64");
        assert_eq!(normalize_arch("aarch64"), "arm64");
        assert_eq!(normalize_arch("arm64"), "arm64");
        assert_eq!(normalize_arch("armv7l"), "armv7");
        assert_eq!(normalize_arch("mips"), "mips");
    }
}
