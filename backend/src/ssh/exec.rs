//! 白名单命令执行：把高层操作翻译成**模板化**的 shell 命令。
//!
//! 安全模型（需求第七节 4）：
//! - 面板**不提供**"执行任意 shell"的接口；
//! - 所有命令都由 `AllowedCommand` 枚举生成，参数按 POSIX 规则单引号转义；
//! - `sudo` 按实例配置拼接（none / sudo / sudo -n / 自定义前缀，自定义需显式开启）；
//! - 审计与日志里只记录"动作 + 脱敏后的命令行"。

use crate::domain::SudoMode;
use crate::error::ApiError;
use crate::ssh::session::{ExecOutput, Phase, SshError, SshSession};

/// 允许执行的操作集合。**新增能力必须在这里登记**，否则无法下发。
#[derive(Debug, Clone)]
pub enum AllowedCommand {
    /// 探测：`uname -s` 等一组只读命令
    Uname,
    /// 是否有 systemd：`command -v systemctl`
    HasSystemd,
    /// 判断命令是否存在
    Which { name: String },
    /// 目录是否存在且可写：`test -d ... && test -w ...`
    DirWritable { path: String },
    /// 磁盘可用空间（MB）：`df -Pm <path> | awk 'NR==2{print $4}'`
    DiskFreeMb { path: String },
    /// 建立目录：`mkdir -p <path>`
    MkdirP { path: String },
    /// 测试文件是否存在：`test -f <path>`
    FileExists { path: String },
    /// 读取文件内容（有大小上限，由 shell 端 head 限制）
    ReadFile { path: String, max_bytes: usize },
    /// 写入文件（通过 stdin 传输）：`cat > <path>`
    WriteFile { path: String },
    /// 备份文件：`cp -p <src> <dst>`
    Copy { src: String, dst: String },
    /// 移动/原子替换：`mv -f <src> <dst>`
    Move { src: String, dst: String },
    /// 设置可执行位：`chmod 0755 <path>`
    Chmod { path: String, mode: String },
    /// 校验哈希：`sha256sum <path>`
    Sha256 { path: String },
    /// 版本号：`<binary> -v`
    BinaryVersion { binary: String },
    /// 进程是否存在：`pgrep -f <pattern>`
    Pgrep { pattern: String },
    /// systemd 操作
    Systemctl { action: String, unit: String },
    /// 写入 systemd unit（内容经 stdin）
    WriteUnit { unit: String },
    /// reload systemd 配置
    DaemonReload,
    /// 启动 nohup 守护
    NohupStart { binary: String, config: String, log: String },
    /// 停止 nohup 守护
    NohupStop { pid_file: String },
    /// tail 日志
    Tail { path: String, lines: usize },
    /// 读 PID 文件
    CatPid { path: String },
    /// 删除文件（部署临时文件清理）
    RemoveFile { path: String },
    /// 磁盘上的文件列表（部署核对）
    ListDir { path: String },
    /// kill 进程
    Kill { pid: u32, force: bool },
    /// 端口占用检查：`ss -Hltn sport = :<port>` 或 fallback
    PortListening { port: u16 },
    /// 当前用户与 sudo 权限：`id -un` / `sudo -n true`
    Whoami,
    /// 已有的 frps/frpc 安装情况
    FindFrpBinary,
}

/// 参数转义：POSIX 单引号内除 `'` 外都是字面量。
pub fn sh_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    // 只含安全字符时不加引号（便于阅读与审计）；其余一律单引号包裹
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "._-/:=@%+".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// 路径里允许出现的字符白名单。
///
/// ★ 这是**纵深防御**：即使 `sh_quote` 一定加引号，也不放行可疑字符，
///   避免未来某个模板忘记调用 `sh_quote` 时被直接注入。
fn check_path(p: &str) -> Result<(), ApiError> {
    if p.is_empty() {
        return Err(ApiError::bad_request("路径不能为空"));
    }
    if p.contains('\n') || p.contains('\r') || p.contains('\0') {
        return Err(ApiError::bad_request("路径不能包含换行或空字节"));
    }
    // shell 元字符一律拒绝（单引号转义已能防护，这里是第二道闸）
    const FORBIDDEN: &[char] = &[';', '|', '&', '`', '$', '<', '>', '(', ')', '{', '}', '*', '?', '!', '"', '\'', '\\'];
    if let Some(c) = p.chars().find(|c| FORBIDDEN.contains(c)) {
        return Err(ApiError::bad_request(format!(
            "路径包含不允许的字符 `{c}`：{p}"
        )));
    }
    if !p.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return Err(ApiError::bad_request(format!(
            "路径包含非 ASCII 或控制字符：{p}"
        )));
    }
    Ok(())
}

/// sudo 前缀。
#[derive(Debug, Clone, Default)]
pub struct SudoPrefix {
    /// 前缀字符串（如 "" / "sudo " / "sudo -n " / "custom-cmd "）
    pub prefix: String,
}

impl SudoPrefix {
    pub fn build(mode: SudoMode, custom: Option<&str>, allow_custom: bool) -> Result<Self, ApiError> {
        let prefix = match mode {
            SudoMode::None => String::new(),
            SudoMode::Sudo => "sudo ".to_string(),
            SudoMode::SudoN => "sudo -n ".to_string(),
            SudoMode::Custom => {
                if !allow_custom {
                    return Err(ApiError::forbidden(
                        "自定义提权命令未启用：请在设置的 SSH 默认值中显式开启 allow_custom_command",
                    ));
                }
                let c = custom.unwrap_or("").trim();
                if c.is_empty() {
                    return Err(ApiError::bad_request("自定义提权命令不能为空"));
                }
                // 仅允许简单命令 + 参数（无管道/重定向/命令替换）
                if c.contains(['|', '&', ';', '`', '$', '\n', '>', '<']) {
                    return Err(ApiError::bad_request(
                        "自定义提权命令不能包含管道、重定向或命令替换",
                    ));
                }
                format!("{c} ")
            }
        };
        Ok(Self { prefix })
    }

    pub fn wrap(&self, cmd: &str) -> String {
        if self.prefix.is_empty() {
            cmd.to_string()
        } else {
            format!("{}{}", self.prefix, cmd)
        }
    }
}

/// 单引号字符串（用于 heredoc/printf 的安全写入）。
pub fn sh_quote_strict(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl AllowedCommand {
    /// 渲染成实际执行的命令：`(是否需要 sudo, 命令行, 可选的 stdin 内容)`
    pub fn render(
        &self,
        sudo: &SudoPrefix,
        stdin: Option<&str>,
    ) -> Result<(String, Option<String>), ApiError> {
        let cmd = match self {
            AllowedCommand::Uname => "uname -s".to_string(),
            AllowedCommand::HasSystemd => "command -v systemctl >/dev/null 2>&1 && echo yes || echo no".into(),
            AllowedCommand::Which { name } => format!("command -v {} 2>/dev/null || echo ''", sh_quote(name)),
            AllowedCommand::DirWritable { path } => {
                check_path(path)?;
                format!("test -d {} && test -w {} && echo yes || echo no", sh_quote(path), sh_quote(path))
            }
            AllowedCommand::DiskFreeMb { path } => {
                check_path(path)?;
                format!(
                    "df -Pm {} 2>/dev/null | awk 'NR==2{{print $4}}'",
                    sh_quote(path)
                )
            }
            AllowedCommand::MkdirP { path } => {
                check_path(path)?;
                sudo.wrap(&format!("mkdir -p {}", sh_quote(path)))
            }
            AllowedCommand::FileExists { path } => {
                check_path(path)?;
                format!("test -f {} && echo yes || echo no", sh_quote(path))
            }
            AllowedCommand::ReadFile { path, max_bytes } => {
                check_path(path)?;
                format!("head -c {} {} 2>/dev/null", max_bytes, sh_quote(path))
            }
            AllowedCommand::WriteFile { path } => {
                check_path(path)?;
                sudo.wrap(&format!("cat > {}", sh_quote(path)))
            }
            AllowedCommand::Copy { src, dst } => {
                check_path(src)?;
                check_path(dst)?;
                sudo.wrap(&format!("cp -p {} {}", sh_quote(src), sh_quote(dst)))
            }
            AllowedCommand::Move { src, dst } => {
                check_path(src)?;
                check_path(dst)?;
                sudo.wrap(&format!("mv -f {} {}", sh_quote(src), sh_quote(dst)))
            }
            AllowedCommand::Chmod { path, mode } => {
                check_path(path)?;
                if !mode.chars().all(|c| c.is_ascii_digit()) || mode.len() < 3 || mode.len() > 4 {
                    return Err(ApiError::bad_request("chmod 模式必须是 3~4 位数字"));
                }
                sudo.wrap(&format!("chmod {} {}", mode, sh_quote(path)))
            }
            AllowedCommand::Sha256 { path } => {
                check_path(path)?;
                format!("sha256sum {} 2>/dev/null | awk '{{print $1}}'", sh_quote(path))
            }
            AllowedCommand::BinaryVersion { binary } => {
                check_path(binary)?;
                format!("{} -v 2>&1 | head -n1", sh_quote(binary))
            }
            AllowedCommand::Pgrep { pattern } => {
                format!("pgrep -f {} 2>/dev/null | head -n5", sh_quote(pattern))
            }
            AllowedCommand::Systemctl { action, unit } => {
                let action = action.as_str();
                if !["start", "stop", "restart", "status", "is-active", "is-enabled", "enable", "disable", "reload"]
                    .contains(&action)
                {
                    return Err(ApiError::bad_request(format!("不允许的 systemctl 动作：{action}")));
                }
                // unit 名只允许 [A-Za-z0-9_.@-]
                if !unit
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c))
                {
                    return Err(ApiError::bad_request("unit 名称只能包含字母数字与 -_.@"));
                }
                sudo.wrap(&format!("systemctl {} {}", action, sh_quote(unit)))
            }
            AllowedCommand::WriteUnit { unit } => {
                if !unit
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c))
                {
                    return Err(ApiError::bad_request("unit 名称非法"));
                }
                // 通过 tee 写入：内容从 stdin 来，避免 shell 注入
                sudo.wrap(&format!(
                    "tee {} >/dev/null",
                    sh_quote(&format!("/etc/systemd/system/{unit}.service"))
                ))
            }
            AllowedCommand::DaemonReload => sudo.wrap("systemctl daemon-reload"),
            AllowedCommand::NohupStart {
                binary,
                config,
                log,
            } => {
                check_path(binary)?;
                check_path(config)?;
                check_path(log)?;
                let pid = format!("{config}.pid");
                sudo.wrap(&format!(
                    "nohup {} -c {} >> {} 2>&1 & echo $! > {}",
                    sh_quote(binary),
                    sh_quote(config),
                    sh_quote(log),
                    sh_quote(&pid)
                ))
            }
            AllowedCommand::NohupStop { pid_file } => {
                check_path(pid_file)?;
                format!(
                    "if [ -f {p} ]; then kill $(cat {p}) 2>/dev/null; sleep 1; kill -9 $(cat {p}) 2>/dev/null; rm -f {p}; echo stopped; else echo absent; fi",
                    p = sh_quote(pid_file)
                )
            }
            AllowedCommand::Tail { path, lines } => {
                check_path(path)?;
                let n = (*lines).clamp(1, 5000);
                format!("tail -n {} {} 2>/dev/null", n, sh_quote(path))
            }
            AllowedCommand::CatPid { path } => {
                check_path(path)?;
                format!("cat {} 2>/dev/null", sh_quote(path))
            }
            AllowedCommand::RemoveFile { path } => {
                check_path(path)?;
                sudo.wrap(&format!("rm -f {}", sh_quote(path)))
            }
            AllowedCommand::ListDir { path } => {
                check_path(path)?;
                format!("ls -lA {} 2>/dev/null | head -n 50", sh_quote(path))
            }
            AllowedCommand::Kill { pid, force } => {
                format!("kill {} {}", if *force { "-9" } else { "-TERM" }, pid)
            }
            AllowedCommand::PortListening { port } => format!(
                "{{ ss -Hltn 2>/dev/null | awk '{{print $4}}' | grep -E ':{p}$' >/dev/null && echo yes; }} || \
                 {{ netstat -ltn 2>/dev/null | awk '{{print $4}}' | grep -E ':{p}$' >/dev/null && echo yes; }} || echo no",
                p = port
            ),
            AllowedCommand::Whoami => "id -un 2>/dev/null; sudo -n true 2>/dev/null && echo 'sudo:nopasswd' || echo 'sudo:password'".into(),
            AllowedCommand::FindFrpBinary => "for p in /usr/local/bin/frps /usr/local/bin/frpc /usr/bin/frps /usr/bin/frpc; do [ -x \"$p\" ] && echo \"$p\"; done; true".into(),
        };

        // 只有 WriteFile / WriteUnit 接受 stdin
        let stdin_body = match self {
            AllowedCommand::WriteFile { .. } | AllowedCommand::WriteUnit { .. } => {
                stdin.map(str::to_string)
            }
            _ => None,
        };
        Ok((cmd, stdin_body))
    }
}

/// 执行一条白名单命令，可选地把 `stdin` 作为文件内容（用 base64 传输避免转义问题）。
pub async fn run(
    session: &SshSession,
    cmd: AllowedCommand,
    sudo: &SudoPrefix,
    stdin: Option<&str>,
) -> Result<ExecOutput, ApiError> {
    let (shell, body) = cmd.render(sudo, stdin)?;

    if let Some(content) = body {
        // 用 base64 双重保险：内容先编码，远端解码后管道写入
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
        if b64.len() > 200_000 {
            return Err(ApiError::bad_request(
                "单次写入内容过大（base64 后 > 200 KB），请拆分或改用部署接口",
            ));
        }
        let full = format!("printf %s {} | base64 -d | {} ", sh_quote(&b64), shell);
        return session.exec(&full).await;
    }

    session.exec(&shell).await
}

/// 执行并断言成功（失败时给出阶段化错误）。
pub async fn run_checked(
    session: &SshSession,
    cmd: AllowedCommand,
    sudo: &SudoPrefix,
    stdin: Option<&str>,
    what: &str,
) -> Result<ExecOutput, ApiError> {
    let out = run(session, cmd, sudo, stdin).await?;
    if !out.ok() {
        let detail = out.combined();
        let hint = if detail.to_lowercase().contains("permission denied")
            || detail.contains("Permission denied")
        {
            "权限不足：请检查 sudo 配置（可能需要 sudo -n 免密或提供密码）"
        } else if detail.contains("command not found") || detail.contains("not found") {
            "远程缺少所需命令：请确认目标系统已安装相应工具"
        } else if detail.contains("No space left") {
            "磁盘空间不足"
        } else {
            "命令执行失败，详见日志"
        };
        return Err(SshError::new(
            Phase::Exec,
            format!("{what}失败（exit {}）：{}", out.exit_code, first_lines(&detail, 6)),
        )
        .with_remediation(hint)
        .to_api());
    }
    Ok(out)
}

fn first_lines(s: &str, n: usize) -> String {
    let t = crate::crypto::redact_line(s.trim());
    t.lines()
        .take(n)
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_shell_metachars() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
        assert_eq!(sh_quote("/usr/local/bin/frps"), "/usr/local/bin/frps");
        assert_eq!(sh_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn sudo_modes() {
        assert_eq!(SudoPrefix::build(SudoMode::None, None, false).unwrap().prefix, "");
        assert_eq!(SudoPrefix::build(SudoMode::Sudo, None, false).unwrap().prefix, "sudo ");
        assert_eq!(SudoPrefix::build(SudoMode::SudoN, None, false).unwrap().prefix, "sudo -n ");
    }

    #[test]
    fn custom_sudo_requires_flag() {
        assert!(SudoPrefix::build(SudoMode::Custom, Some("doas "), false).is_err());
        assert!(SudoPrefix::build(SudoMode::Custom, Some("doas "), true).is_ok());
        // 注入被拒绝
        assert!(SudoPrefix::build(SudoMode::Custom, Some("sudo; rm -rf /"), true).is_err());
    }

    #[test]
    fn rejects_shell_metachars_in_paths() {
        let sudo = SudoPrefix::default();
        // 路径白名单在渲染阶段直接拒绝，命令根本不会下发
        let cmd = AllowedCommand::MkdirP {
            path: "/tmp/x; rm -rf /".into(),
        };
        let err = cmd.render(&sudo, None).unwrap_err();
        assert!(err.to_string().contains("不允许的字符"), "{err}");

        // 命令替换、管道、重定向同样被拒
        for bad in [
            "/tmp/$(whoami)",
            "/tmp/a | cat",
            "/tmp/a && reboot",
            "/tmp/`id`",
            "/tmp/a > /etc/passwd",
        ] {
            let c = AllowedCommand::MkdirP { path: bad.into() };
            assert!(c.render(&sudo, None).is_err(), "应拒绝：{bad}");
        }
    }

    #[test]
    fn legitimate_paths_are_not_quoted_needlessly() {
        let sudo = SudoPrefix::default();
        let (cmd, _) = AllowedCommand::MkdirP {
            path: "/usr/local/bin".into(),
        }
        .render(&sudo, None)
        .unwrap();
        assert_eq!(cmd, "mkdir -p /usr/local/bin");
    }

    #[test]
    fn rejects_newline_in_path() {
        let sudo = SudoPrefix::default();
        let cmd = AllowedCommand::WriteFile {
            path: "/tmp/a\nb".into(),
        };
        assert!(cmd.render(&sudo, Some("x")).is_err());
    }

    #[test]
    fn systemctl_actions_are_whitelisted() {
        let sudo = SudoPrefix::build(SudoMode::SudoN, None, false).unwrap();
        let ok = AllowedCommand::Systemctl {
            action: "restart".into(),
            unit: "nfrp-server".into(),
        };
        assert_eq!(
            ok.render(&sudo, None).unwrap().0,
            "sudo -n systemctl restart nfrp-server"
        );
        let bad = AllowedCommand::Systemctl {
            action: "mask".into(),
            unit: "x".into(),
        };
        assert!(bad.render(&sudo, None).is_err());
        let bad_unit = AllowedCommand::Systemctl {
            action: "start".into(),
            unit: "x; reboot".into(),
        };
        assert!(bad_unit.render(&sudo, None).is_err());
    }

    #[test]
    fn write_file_uses_stdin_body() {
        let sudo = SudoPrefix::build(SudoMode::SudoN, None, false).unwrap();
        let cmd = AllowedCommand::WriteFile {
            path: "/etc/nfrp/frps.toml".into(),
        };
        let (shell, body) = cmd.render(&sudo, Some("a = 1\n")).unwrap();
        assert_eq!(shell, "sudo -n cat > /etc/nfrp/frps.toml");
        assert_eq!(body.as_deref(), Some("a = 1\n"));

        // 非写文件命令不接受 stdin
        let (_, body2) = AllowedCommand::Uname.render(&sudo, Some("evil")).unwrap();
        assert!(body2.is_none());
    }

    #[test]
    fn chmod_mode_validated() {
        let sudo = SudoPrefix::default();
        let ok = AllowedCommand::Chmod {
            path: "/usr/local/bin/frps".into(),
            mode: "0755".into(),
        };
        assert!(ok.render(&sudo, None).is_ok());
        let bad = AllowedCommand::Chmod {
            path: "/x".into(),
            mode: "u+s".into(),
        };
        assert!(bad.render(&sudo, None).is_err());
    }

    #[test]
    fn disk_free_has_escaped_braces() {
        let sudo = SudoPrefix::default();
        let (cmd, _) = AllowedCommand::DiskFreeMb {
            path: "/var".into(),
        }
        .render(&sudo, None)
        .unwrap();
        assert!(cmd.contains("NR==2{print $4}"), "{cmd}");
    }
}
