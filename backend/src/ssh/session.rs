//! russh 会话：连接、主机密钥校验、认证、命令执行、超时与阶段化错误。
//!
//! 分阶段错误（对齐需求「失败给出明确阶段（连接/认证/上传/权限/启动）」）：
//! - `connect`：TCP/握手失败
//! - `host_key`：主机密钥未知或不匹配
//! - `auth`：认证失败
//! - `exec`：命令执行失败（含权限）
//! - `transfer`：文件传输失败
//! - `startup`：服务启动/就绪校验失败

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::ssh_key;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::Mutex;

use crate::config::RuntimeConfig;
use crate::domain::AuthKind;
use crate::error::ApiError;
use crate::ssh::hostkey;
use crate::ssh::SshConnectParams;

/// 失败的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    HostKey,
    Auth,
    Exec,
    Transfer,
    Startup,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Connect => "connect",
            Phase::HostKey => "host_key",
            Phase::Auth => "auth",
            Phase::Exec => "exec",
            Phase::Transfer => "transfer",
            Phase::Startup => "startup",
        }
    }
}

/// 带阶段的 SSH 错误。
#[derive(Debug)]
pub struct SshError {
    pub phase: Phase,
    pub message: String,
    pub remediation: Option<String>,
}

impl SshError {
    pub fn new(phase: Phase, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
            remediation: None,
        }
    }
    pub fn with_remediation(mut self, r: impl Into<String>) -> Self {
        self.remediation = Some(r.into());
        self
    }
    pub fn hint_text(&self) -> String {
        match self.remediation.as_deref() {
            Some(r) => format!("{}（建议：{r}）", self.message),
            None => self.message.clone(),
        }
    }
    pub fn to_api(&self) -> ApiError {
        ApiError::upstream(format!("SSH[{}] {}", self.phase.as_str(), self.hint_text()))
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SSH[{}] {}", self.phase.as_str(), self.message)
    }
}

impl std::error::Error for SshError {}

/// russh 的 `Handler::Error` 要求能从 `russh::Error` 构造。
impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::new(Phase::Connect, format!("SSH 协议错误：{e}"))
    }
}

/// 主机密钥校验策略。
pub enum HostKeyPolicy {
    /// 与已知指纹比对（默认），未知即拒绝。
    Strict { host: String, port: u16 },
}

/// russh Handler：负责 `check_server_key`。
pub struct ClientHandler {
    policy: HostKeyPolicy,
    /// 记录本次握手看到的主机密钥（供"未知主机"场景回报指纹）
    seen: Arc<Mutex<Option<SeenKey>>>,
}

#[derive(Debug, Clone)]
pub struct SeenKey {
    pub algorithm: String,
    pub fingerprint: String,
    pub public_key: String,
}

impl ClientHandler {
    pub fn new(policy: HostKeyPolicy) -> (Self, Arc<Mutex<Option<SeenKey>>>) {
        let seen = Arc::new(Mutex::new(None));
        (
            Self {
                policy,
                seen: seen.clone(),
            },
            seen,
        )
    }
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let alg = server_public_key.algorithm().to_string();
        let fp = hostkey::fingerprint_of(server_public_key);
        let raw = server_public_key.to_openssh().unwrap_or_default();
        *self.seen.lock().await = Some(SeenKey {
            algorithm: alg.clone(),
            fingerprint: fp.clone(),
            public_key: raw,
        });

        let HostKeyPolicy::Strict { host, port } = &self.policy;
        match hostkey::lookup_known(host, *port, &fp).await {
            Ok(true) => Ok(true),
            Ok(false) => {
                // 明确拒绝，并把指纹带回给调用方展示（"首次连接需确认"）
                Err(SshError::new(
                    Phase::HostKey,
                    format!("主机 {host}:{port} 的指纹未受信任：{fp}"),
                )
                .with_remediation("在「主机指纹确认」中核对并信任该指纹"))
            }
            Err(e) => Err(e),
        }
    }
}

/// 一条命令的结果。
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u32,
}

impl ExecOutput {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
    pub fn combined(&self) -> String {
        let mut s = self.stdout.clone();
        if !self.stderr.trim().is_empty() {
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&self.stderr);
        }
        s
    }
}

/// 一个已建立的 SSH 会话。
pub struct SshSession {
    pub host: String,
    pub port: u16,
    pub username: String,
    handle: Mutex<Handle<ClientHandler>>,
    command_timeout: Duration,
    pub host_key: Option<SeenKey>,
    pub params: SshConnectParams,
}

impl SshSession {
    /// 建立连接并认证。任何阶段的失败都返回带阶段的 `SshError`。
    pub async fn connect(
        params: &SshConnectParams,
        cfg: &RuntimeConfig,
    ) -> Result<Self, ApiError> {
        let connect_timeout = Duration::from_millis(cfg.raw.ssh.connect_timeout_ms);
        let command_timeout = Duration::from_millis(cfg.raw.ssh.command_timeout_ms);

        // 认证前先探测主机密钥：用"仅握手"的方式拿到指纹，
        // 避免出现"密码已经发出去但主机不可信"的情况。
        let (handler, seen) = ClientHandler::new(HostKeyPolicy::Strict {
            host: params.host.clone(),
            port: params.port,
        });

        let ssh_config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        });

        let addr = format!("{}:{}", params.host, params.port);
        let connect_fut = client::connect(ssh_config, addr.clone(), handler);
        let mut handle = match tokio::time::timeout(connect_timeout, connect_fut).await {
            Err(_) => {
                return Err(SshError::new(
                    Phase::Connect,
                    format!("连接 {addr} 超时（{} ms）", connect_timeout.as_millis()),
                )
                .with_remediation("检查主机地址/端口、防火墙与安全组")
                .to_api())
            }
            Ok(Err(e)) => {
                return Err(SshError::new(Phase::Connect, format!("无法连接 {addr}：{e}"))
                    .with_remediation("确认 SSH 服务在运行且端口可达")
                    .to_api())
            }
            Ok(Ok(h)) => h,
        };

        let seen_key = seen.lock().await.clone();

        // 认证
        let auth_result = match params.auth_kind {
            AuthKind::Password => {
                let pw = params.secret.clone().ok_or_else(|| {
                    SshError::new(Phase::Auth, "缺少 SSH 密码凭据").to_api()
                })?;
                handle
                    .authenticate_password(params.username.clone(), pw)
                    .await
            }
            AuthKind::PrivateKey | AuthKind::PrivateKeyPassphrase => {
                let key_text = params.secret.clone().ok_or_else(|| {
                    SshError::new(Phase::Auth, "缺少 SSH 私钥凭据").to_api()
                })?;
                let key = match params.passphrase.as_deref() {
                    Some(p) if !p.is_empty() => {
                        russh::keys::decode_secret_key(&key_text, Some(p))
                    }
                    _ => russh::keys::decode_secret_key(&key_text, None),
                };
                let key = match key {
                    Ok(k) => Arc::new(k),
                    Err(e) => {
                        return Err(SshError::new(
                            Phase::Auth,
                            format!("私钥解析失败：{e}"),
                        )
                        .with_remediation("确认私钥为 OpenSSH PEM 格式；带口令的私钥需填写口令")
                        .to_api())
                    }
                };
                // RSA 需要协商签名哈希
                let hash_alg = match handle.best_supported_rsa_hash().await {
                    Ok(Some(h)) => h,
                    _ => None,
                };
                let key = russh::keys::PrivateKeyWithHashAlg::new(key, hash_alg);
                handle
                    .authenticate_publickey(params.username.clone(), key)
                    .await
            }
            AuthKind::Agent => {
                return Err(SshError::new(
                    Phase::Auth,
                    "面板后端当前不支持 ssh-agent 认证",
                )
                .with_remediation("改用「密码」或「私钥」认证方式")
                .to_api())
            }
        };

        match auth_result {
            Ok(russh::client::AuthResult::Success) => {}
            Ok(_) => {
                let _ = handle
                    .disconnect(Disconnect::ByApplication, "auth failed", "")
                    .await;
                return Err(SshError::new(
                    Phase::Auth,
                    format!("用户 {} 认证失败", params.username),
                )
                .with_remediation("确认用户名与密码/私钥匹配，且该用户允许 SSH 登录")
                .to_api());
            }
            Err(e) => {
                return Err(SshError::new(Phase::Auth, format!("认证过程出错：{e}"))
                    .with_remediation("检查用户名、密码或私钥")
                    .to_api())
            }
        }

        Ok(Self {
            host: params.host.clone(),
            port: params.port,
            username: params.username.clone(),
            handle: Mutex::new(handle),
            command_timeout,
            host_key: seen_key,
            params: params.clone(),
        })
    }

    pub fn is_alive(&self) -> bool {
        match self.handle.try_lock() {
            Ok(h) => !h.is_closed(),
            Err(_) => true, // 正被其它任务使用：先当作存活
        }
    }

    /// 执行一条命令（带超时）。**调用方必须已经过 `exec::AllowedCommand` 白名单**。
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, ApiError> {
        let handle = self.handle.lock().await;
        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::new(Phase::Exec, format!("打开 SSH 通道失败：{e}")).to_api())?;

        channel
            .exec(true, command.as_bytes().to_vec())
            .await
            .map_err(|e| SshError::new(Phase::Exec, format!("下发命令失败：{e}")).to_api())?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<u32> = None;

        let collect = async {
            while let Some(msg) = channel.wait().await {
                match msg {
                    ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, ext } => {
                        if ext == 1 {
                            stderr.extend_from_slice(&data);
                        }
                    }
                    ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status),
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
        };

        match tokio::time::timeout(self.command_timeout, collect).await {
            Err(_) => Err(SshError::new(
                Phase::Exec,
                format!(
                    "命令执行超时（{} ms）：{}",
                    self.command_timeout.as_millis(),
                    crate::crypto::redact_line(command)
                ),
            )
            .to_api()),
            Ok(_) => {
                let _ = channel.close().await;
                Ok(ExecOutput {
                    stdout: String::from_utf8_lossy(&stdout).to_string(),
                    stderr: String::from_utf8_lossy(&stderr).to_string(),
                    exit_code: exit_code.unwrap_or(0),
                })
            }
        }
    }

    pub async fn disconnect(&self) {
        let h = self.handle.lock().await;
        let _ = h
            .disconnect(Disconnect::ByApplication, "panel done", "")
            .await;
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        // Handle 自身 Drop 会关闭连接，这里不需要额外处理
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_names_are_stable() {
        // 前端按这些字符串做提示分支，改动即为不兼容
        assert_eq!(Phase::Connect.as_str(), "connect");
        assert_eq!(Phase::HostKey.as_str(), "host_key");
        assert_eq!(Phase::Auth.as_str(), "auth");
        assert_eq!(Phase::Exec.as_str(), "exec");
    }

    #[test]
    fn ssh_error_carries_remediation() {
        let e = SshError::new(Phase::Auth, "bad password")
            .with_remediation("检查密码");
        assert!(e.hint_text().contains("检查密码"));
        assert!(e.to_api().to_string().contains("auth"));
    }
}
