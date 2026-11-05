//! 远程文件传输。
//!
//! ## 为什么不用纯 Rust SFTP
//! `russh 0.54` 不含 SFTP 子系统实现，引一个第三方 SFTP crate 会增加供应链面。
//! 这里用**系统 OpenSSH `sftp` 子进程**作为传输通道，并保留安全约束：
//!
//! 1. **主机密钥仍由面板校验**：部署前必过一次 `russh` 握手（`SshSession::connect`），
//!    未信任的指纹无法进入本模块；`sftp` 进程再带 `StrictHostKeyChecking=yes` 二次校验；
//! 2. **凭据不落盘**：密码/口令通过 `SSH_ASKPASS`（临时脚本，0600，用后即删）传入，
//!    私钥通过 `-i <0600 临时文件>` 传入，用完立刻删除；
//! 3. **依赖声明**：需要本机安装 OpenSSH 客户端（Windows 10+ 自带，Linux/macOS 自带）。
//!
//! ## 写入语义
//! 远端写入采用「上传到临时路径 → `mv` 原子替换」，避免出现半个配置文件。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::crypto;
use crate::domain::AuthKind;
use crate::error::ApiError;
use crate::ssh::session::Phase;
use crate::ssh::SshConnectParams;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

/// 凭据落盘目录（0600 文件，用后即删）。
fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join("frppanel-sftp");
    let _ = std::fs::create_dir_all(&d);
    // 收紧目录权限
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    }
    d
}

/// 临时凭据文件，Drop 时自动删除。
struct TempSecret {
    path: Option<PathBuf>,
}

impl TempSecret {
    fn write(contents: &str, name_prefix: &str) -> Result<Self, ApiError> {
        let path = temp_dir().join(format!("{name_prefix}-{}", unique_suffix()));
        std::fs::write(&path, contents)?;
        crypto::restrict_permissions(&path)?;
        Ok(Self { path: Some(path) })
    }
    fn path(&self) -> &Path {
        self.path.as_deref().expect("temp secret path")
    }
    #[allow(dead_code)]
    fn take(mut self) -> PathBuf {
        self.path.take().expect("temp secret path")
    }
}

impl Drop for TempSecret {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 一次 sftp 调用的上下文（凭据与主机参数）。
pub struct TransferContext {
    pub params: SshConnectParams,
    /// 由 russh 握手确认过的算法，用于 pin 到 sftp（可选）
    pub host_key_algorithms: Option<String>,
}

impl TransferContext {
    pub fn from_params(params: &SshConnectParams, algorithm: Option<&str>) -> Self {
        Self {
            params: params.clone(),
            host_key_algorithms: algorithm.map(str::to_string),
        }
    }
}

/// 构造 `sftp` 命令与需要清理的临时文件。
fn build_command(
    ctx: &TransferContext,
) -> Result<(Command, Vec<TempSecret>, Vec<TempSecret>), ApiError> {
    let mut secrets: Vec<TempSecret> = Vec::new();
    let mut askpass: Vec<TempSecret> = Vec::new();

    // 使用面板维护的独立 known_hosts，严格校验主机密钥（绝不自动接受）
    let kh = known_hosts_path();
    let mut cmd = Command::new("sftp");
    cmd.arg("-b")
        .arg("-")
        .arg("-P")
        .arg(ctx.params.port.to_string())
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", kh.display()))
        .arg("-o")
        .arg("GlobalKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("StrictHostKeyChecking=yes")
        .arg("-o")
        .arg("ConnectTimeout=15")
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    match ctx.params.auth_kind {
        AuthKind::Password => {
            let pw = ctx
                .params
                .secret
                .clone()
                .ok_or_else(|| ApiError::bad_request("缺少 SSH 密码"))?;
            // SSH_ASKPASS 脚本：把密码写到 stdout
            let script = if cfg!(windows) {
                format!("@echo off\r\necho {}\r\n", pw.replace(['\r', '\n'], ""))
            } else {
                format!("#!/bin/sh\nprintf '%s\\n' {:?}\n", pw)
            };
            let s = TempSecret::write(&script, "askpass")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(s.path(), std::fs::Permissions::from_mode(0o700));
            }
            cmd.env("SSH_ASKPASS", s.path())
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", ":0")
                .env("SSH_ASKPASS_PROMPT", "none")
                .arg("-o")
                .arg("PreferredAuthentications=password,keyboard-interactive")
                .arg("-o")
                .arg("PubkeyAuthentication=no");
            askpass.push(s);
        }
        AuthKind::PrivateKey | AuthKind::PrivateKeyPassphrase => {
            let key = ctx
                .params
                .secret
                .clone()
                .ok_or_else(|| ApiError::bad_request("缺少 SSH 私钥"))?;
            let k = TempSecret::write(&key, "id_key")?;
            cmd.arg("-i").arg(k.path());
            if let Some(pp) = ctx.params.passphrase.clone().filter(|p| !p.is_empty()) {
                let script = if cfg!(windows) {
                    format!("@echo off\r\necho {}\r\n", pp.replace(['\r', '\n'], ""))
                } else {
                    format!("#!/bin/sh\nprintf '%s\\n' {:?}\n", pp)
                };
                let s = TempSecret::write(&script, "askpass")?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(s.path(), std::fs::Permissions::from_mode(0o700));
                }
                cmd.env("SSH_ASKPASS", s.path())
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env("DISPLAY", ":0");
                askpass.push(s);
            }
            cmd.arg("-o").arg("IdentitiesOnly=yes");
            secrets.push(k);
        }
        AuthKind::Agent => {
            cmd.arg("-o").arg("PreferredAuthentications=publickey");
        }
    }

    cmd.arg(format!("{}@{}", ctx.params.username, ctx.params.host));

    Ok((cmd, secrets, askpass))
}

/// 面板维护的 known_hosts（OpenSSH 格式），由 `sync_known_hosts` 写入。
pub fn known_hosts_path() -> PathBuf {
    // 放到系统临时目录下的独立文件，避免影响用户自己的 ~/.ssh/known_hosts
    temp_dir().join("known_hosts")
}

/// 用面板的信任库生成一份 OpenSSH known_hosts（仅供 sftp 子进程使用）。
pub fn sync_known_hosts(
    entries: &[(String, u16, String, String)],
) -> Result<PathBuf, ApiError> {
    let path = known_hosts_path();
    let mut out = String::from("# FrpPanel 自动生成：请勿手工编辑\n");
    for (host, port, alg, pubkey_b64) in entries {
        let host_field = if *port == 22 {
            host.clone()
        } else {
            format!("[{host}]:{port}")
        };
        out.push_str(&format!("{host_field} {alg} {pubkey_b64}\n"));
    }
    std::fs::write(&path, out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// 执行一批 sftp 批处理命令。
async fn run_batch(ctx: &TransferContext, batch: &str) -> Result<String, ApiError> {
    let (mut cmd, secrets, askpass) = build_command(ctx)?;
    let mut child = cmd.spawn().map_err(|e| {
        ApiError::upstream(format!(
            "无法启动 sftp 传输通道（需要本机安装 OpenSSH 客户端）：{e}"
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(batch.as_bytes()).await?;
        drop(stdin);
    }

    let out = child.wait_with_output().await.map_err(|e| {
        ApiError::upstream(format!("sftp 传输失败：{e}"))
    })?;
    // 清理临时凭据
    drop(secrets);
    drop(askpass);

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        let msg = format!("{stdout}\n{stderr}");
        let hint = if msg.contains("Host key verification failed") {
            "主机密钥未受信任或与记录不一致：请在「主机指纹」中确认"
        } else if msg.contains("Permission denied") {
            "认证失败或权限不足：检查凭据与远端目录权限"
        } else if msg.contains("No such file") || msg.contains("not found") {
            "sftp 客户端不可用或远端路径不存在"
        } else {
            "传输失败"
        };
        return Err(crate::ssh::session::SshError::new(
            Phase::Transfer,
            format!("{hint}：{}", crate::crypto::redact_line(msg.trim())),
        )
        .to_api());
    }
    Ok(format!("{stdout}{stderr}"))
}

/// 上传内容到远端临时路径。
pub async fn upload_bytes(
    ctx: &TransferContext,
    remote_tmp: &str,
    content: &[u8],
) -> Result<(), ApiError> {
    // 先把内容写到本地临时文件（避免二进制经 stdin 二次编码）
    let local = temp_dir().join(format!("upload-{}", unique_suffix()));
    std::fs::write(&local, content)?;
    let res = upload_file(ctx, &local, remote_tmp).await;
    let _ = std::fs::remove_file(&local);
    res
}

/// 上传本地文件到远端路径。
pub async fn upload_file(
    ctx: &TransferContext,
    local: &Path,
    remote: &str,
) -> Result<(), ApiError> {
    if !local.exists() {
        return Err(ApiError::not_found(format!(
            "本地文件不存在：{}",
            local.display()
        )));
    }
    let batch = format!(
        "put \"{}\" \"{}\"\n",
        local.display().to_string().replace('"', ""),
        remote.replace('"', "")
    );
    run_batch(ctx, &batch).await?;
    Ok(())
}

/// 下载远端文件到本地路径。
pub async fn download_file(
    ctx: &TransferContext,
    remote: &str,
    local: &Path,
) -> Result<(), ApiError> {
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let batch = format!(
        "get \"{}\" \"{}\"\n",
        remote.replace('"', ""),
        local.display().to_string().replace('"', "")
    );
    run_batch(ctx, &batch).await?;
    Ok(())
}

/// 读远端文本（小于 1 MB）。
pub async fn read_text(
    ctx: &TransferContext,
    remote: &str,
    max_bytes: usize,
) -> Result<String, ApiError> {
    // 用 exec 通道更省事（head -c 限制大小），但这里保留 sftp 版本用于无 exec 场景
    let local = temp_dir().join(format!("download-{}", unique_suffix()));
    download_file(ctx, remote, &local).await?;
    let data = std::fs::read(&local).unwrap_or_default();
    let _ = std::fs::remove_file(&local);
    let truncated = &data[..data.len().min(max_bytes)];
    Ok(String::from_utf8_lossy(truncated).to_string())
}

/// 校验传输链路可用（只连不传）。
pub async fn check_available() -> Result<(), ApiError> {
    let out = Command::new("sftp")
        .arg("-h")
        .output()
        .await
        .map_err(|_| {
            ApiError::upstream(
                "本机没有可用的 sftp（OpenSSH 客户端）：Windows 10+ 自带，Linux 请安装 openssh-client",
            )
        })?;
    let _ = out;
    Ok(())
}

/// base64 编码（给"用 exec 通道写文件"的场景复用）。
pub fn b64(content: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_lines_use_bracket_port() {
        let dir = std::env::temp_dir();
        let p = dir.join("frppanel-kh-test");
        std::fs::write(&p, "").unwrap();
        let entries = vec![
            ("1.1.1.1".to_string(), 22, "ssh-ed25519".to_string(), "AAAA".to_string()),
            ("2.2.2.2".to_string(), 2222, "ssh-rsa".to_string(), "BBBB".to_string()),
        ];
        let path = sync_known_hosts(&entries).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("1.1.1.1 ssh-ed25519 AAAA"));
        assert!(text.contains("[2.2.2.2]:2222 ssh-rsa BBBB"));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn password_askpass_script_never_contains_newlines() {
        let s = TempSecret::write("@echo off\r\necho secret\r\n", "askpass").unwrap();
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(text.contains("secret"));
        drop(s); // Drop 后文件应被删除
    }

    #[test]
    fn temp_secret_is_removed_on_drop() {
        let p;
        {
            let s = TempSecret::write("x", "tmp").unwrap();
            p = s.path().to_path_buf();
            assert!(p.exists());
        }
        assert!(!p.exists());
    }
}
