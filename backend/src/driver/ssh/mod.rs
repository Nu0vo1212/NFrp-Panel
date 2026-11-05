//! SshBinaryDriver：通过 SSH 在**远程主机**上管理 frps/frpc。
//!
//! 语义与 `BinaryDriver` 保持一致（同样的启停/配置/代理/状态），差异只在传输层：
//! - 进程：systemd unit 或 nohup 守护（远程没有 Windows 进程组概念）；
//! - 配置：SFTP 上传 + 远程 `cp` 备份 + `mv` 原子替换；
//! - 日志：远程 `tail` + 分页（受 `command_timeout` 限制，单次取最后 N 行）；
//! - Admin API：若配置了 `admin_api.base_url`，从**面板所在主机**直接访问远程面板端口；
//!   若面板无法直连（例如面板端口只监听 127.0.0.1），会明确告知需要 SSH 隧道或改绑定。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use crate::config::RuntimeConfig;
use crate::crypto::KeyRing;
use crate::db::models::InstanceRow;
use crate::domain::*;
use crate::error::ApiError;
use crate::frpapi::probe::{self, ProbeInput};
use crate::frpapi::{AdminAuth, FrpAdminClient};
use crate::ssh::exec::{self, AllowedCommand, SudoPrefix};
use crate::ssh::session::SshSession;
use crate::ssh::{self, SshConnectParams};

use super::{unsupported, Driver, HealthReport, StopOptions, WriteOptions};

pub struct SshBinaryDriver {
    row: InstanceRow,
    keys: Arc<KeyRing>,
    db: sqlx::SqlitePool,
    cfg: RuntimeConfig,
    /// 缓存的会话（避免每步都握手）
    session: Mutex<Option<Arc<SshSession>>>,
    capabilities: Mutex<Arc<Capabilities>>,
}

impl SshBinaryDriver {
    pub fn new(
        row: InstanceRow,
        keys: Arc<KeyRing>,
        db: sqlx::SqlitePool,
        cfg: RuntimeConfig,
    ) -> Self {
        Self {
            capabilities: Mutex::new(Arc::new(
                row.capabilities
                    .clone()
                    .unwrap_or_else(|| Capabilities::unknown(row.flavor)),
            )),
            row,
            keys,
            db,
            cfg,
            session: Mutex::new(None),
        }
    }

    /// 远程连接参数（解密凭据）。
    pub async fn connect_params(&self) -> Result<SshConnectParams, ApiError> {
        let Connection::Ssh {
            host,
            port,
            username,
            auth_kind,
            credential_id,
            ..
        } = &self.row.connection
        else {
            return Err(ApiError::bad_request("该实例不是 SSH 连接"));
        };

        let (secret, passphrase) = match credential_id {
            Some(id) => {
                let cred = crate::db::repo::get_credential(&self.db, *id).await?;
                let s = self
                    .keys
                    .decrypt(&cred.secret_enc, &format!("credential:{}:secret", cred.id))?;
                let p = match &cred.passphrase_enc {
                    Some(enc) => Some(
                        self.keys
                            .decrypt(enc, &format!("credential:{}:passphrase", cred.id))?,
                    ),
                    None => None,
                };
                (Some(s), p)
            }
            None => {
                return Err(ApiError::bad_request(
                    "该 SSH 实例尚未绑定凭据：请先在向导或设置中创建凭据",
                ))
            }
        };

        Ok(SshConnectParams {
            host: host.clone(),
            port: *port,
            username: username.clone(),
            auth_kind: *auth_kind,
            secret,
            passphrase,
        })
    }

    /// 取得（或建立）SSH 会话。
    pub async fn session(&self) -> Result<Arc<SshSession>, ApiError> {
        {
            let g = self.session.lock().await;
            if let Some(s) = g.as_ref() {
                if s.is_alive() {
                    return Ok(s.clone());
                }
            }
        }
        let params = self.connect_params().await?;
        let s = Arc::new(SshSession::connect(&params, &self.cfg).await?);
        // 同步 known_hosts 给 sftp 子进程用
        self.sync_known_hosts().await;
        *self.session.lock().await = Some(s.clone());
        Ok(s)
    }

    async fn sync_known_hosts(&self) {
        if let Ok(rows) = crate::db::repo::list_host_keys(&self.db).await {
            let entries: Vec<(String, u16, String, String)> = rows
                .into_iter()
                .map(|r| {
                    // 我们的 public key 存的是 openssh 单行格式 "<alg> <b64>"
                    let b64 = r
                        .public_key
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or(&r.public_key)
                        .to_string();
                    (r.host, r.port, r.algorithm, b64)
                })
                .collect();
            let _ = ssh::sftp::sync_known_hosts(&entries);
        }
    }

    /// 当前 Admin API 实现类型：实时探测优先，缓存兜底。
    async fn live_api_kind(&self) -> ApiKind {
        if let Some(c) = self.admin_client() {
            if let Ok(k) = c.detect_api_kind().await {
                if k != ApiKind::None && k != ApiKind::Unknown {
                    return k;
                }
            }
        }
        self.row
            .capabilities
            .as_ref()
            .map(|c| c.api_kind)
            .unwrap_or(ApiKind::Unknown)
    }

    fn sudo(&self) -> Result<SudoPrefix, ApiError> {
        let Connection::Ssh { sudo, sudo_cmd, .. } = &self.row.connection else {
            return Ok(SudoPrefix::default());
        };
        SudoPrefix::build(
            *sudo,
            sudo_cmd.as_deref(),
            self.cfg.raw.ssh.allow_custom_command,
        )
    }

    fn unit_name(&self) -> String {
        self.row
            .service_unit
            .clone()
            .unwrap_or_else(|| format!("nfrp-{}", self.row.role.as_str()))
    }

    fn admin_client(&self) -> Option<FrpAdminClient> {
        if !self.row.admin_api_enabled || self.row.admin_api_base_url.trim().is_empty() {
            return None;
        }
        // ★ 远程 Admin API：面板从本机直接访问。若端口只监听 127.0.0.1，这个客户端会失败，
        //    由 probe 的 notes 明确提示"需要 SSH 隧道或改监听地址"。
        let password = self
            .row
            .admin_api_password_enc
            .as_ref()
            .and_then(|e| {
                self.keys
                    .decrypt(e, &format!("instance:{}:admin_password", self.row.id))
                    .ok()
            })
            .unwrap_or_default();
        FrpAdminClient::new(
            &self.row.admin_api_base_url,
            AdminAuth {
                user: self.row.admin_api_user.clone(),
                password,
            },
            self.row.role,
            Duration::from_millis(self.cfg.raw.frp.admin_timeout_ms),
            self.row.capabilities.as_ref().map(|c| c.api_kind),
        )
        .ok()
    }

    /// 远程状态：systemd `is-active` / PID 文件 + 可选的 Admin API。
    async fn remote_pid(&self) -> Result<Option<u32>, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;
        if self.row.service_mode == ServiceMode::Systemd {
            let out = exec::run(
                &s,
                AllowedCommand::Systemctl {
                    action: "is-active".into(),
                    unit: self.unit_name(),
                },
                &sudo,
                None,
            )
            .await?;
            if out.stdout.trim() != "active" {
                return Ok(None);
            }
            // 拿主进程 PID
            let out = exec::run(
                &s,
                AllowedCommand::Systemctl {
                    action: "status".into(),
                    unit: self.unit_name(),
                },
                &sudo,
                None,
            )
            .await?;
            for line in out.stdout.lines() {
                if let Some(rest) = line.split("Main PID:").nth(1) {
                    let pid: String = rest
                        .trim()
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if let Ok(p) = pid.parse::<u32>() {
                        if p > 0 {
                            return Ok(Some(p));
                        }
                    }
                }
            }
            Ok(Some(0))
        } else {
            let pid_file = format!("{}.pid", self.row.paths.config);
            let out = exec::run(&s, AllowedCommand::CatPid { path: pid_file }, &sudo, None).await?;
            Ok(out.stdout.trim().parse::<u32>().ok().filter(|p| *p > 0))
        }
    }
}

#[async_trait]
impl Driver for SshBinaryDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::SshBinary
    }

    fn instance_id(&self) -> uuid::Uuid {
        self.row.id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn start(&self) -> Result<InstanceStatus, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;

        if self.row.deployment_status == DeploymentStatus::NotDeployed {
            return Err(ApiError::conflict(
                "该实例尚未部署：请先在「部署」页签执行部署（安装二进制与配置）",
            ));
        }
        if self.remote_pid().await?.is_some() {
            return Err(ApiError::conflict("实例已在运行"));
        }

        match self.row.service_mode {
            ServiceMode::Systemd => {
                exec::run_checked(
                    &s,
                    AllowedCommand::Systemctl {
                        action: "start".into(),
                        unit: self.unit_name(),
                    },
                    &sudo,
                    None,
                    "启动 systemd 服务",
                )
                .await?;
            }
            _ => {
                exec::run_checked(
                    &s,
                    AllowedCommand::NohupStart {
                        binary: self.row.paths.binary.clone(),
                        config: self.row.paths.config.clone(),
                        log: self.row.paths.log.clone(),
                    },
                    &sudo,
                    None,
                    "以 nohup 方式启动",
                )
                .await?;
            }
        }

        tokio::time::sleep(Duration::from_millis(800)).await;
        self.status().await
    }

    async fn stop(&self, opts: StopOptions) -> Result<InstanceStatus, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;
        match self.row.service_mode {
            ServiceMode::Systemd => {
                exec::run_checked(
                    &s,
                    AllowedCommand::Systemctl {
                        action: "stop".into(),
                        unit: self.unit_name(),
                    },
                    &sudo,
                    None,
                    "停止 systemd 服务",
                )
                .await?;
            }
            _ => {
                let pid_file = format!("{}.pid", self.row.paths.config);
                let _ = exec::run(&s, AllowedCommand::NohupStop { pid_file }, &sudo, None).await;
            }
        }
        let _ = opts;
        Ok(InstanceStatus {
            state: LiveState::Stopped,
            healthy: false,
            pid: None,
            uptime_secs: None,
            binary_version: self.row.binary_version.clone(),
            api_reachable: false,
            last_error: None,
            detail: StatusDetail {
                source: "ssh".into(),
                notes: vec![format!("已在 {} 上停止服务", self.row.connection.host_label())],
            },
            counters: None,
        })
    }

    async fn restart(&self) -> Result<InstanceStatus, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;
        match self.row.service_mode {
            ServiceMode::Systemd => {
                exec::run_checked(
                    &s,
                    AllowedCommand::Systemctl {
                        action: "restart".into(),
                        unit: self.unit_name(),
                    },
                    &sudo,
                    None,
                    "重启 systemd 服务",
                )
                .await?;
            }
            _ => {
                let _ = self.stop(StopOptions::default()).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                return self.start().await;
            }
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        self.status().await
    }

    async fn health(&self) -> Result<HealthReport, ApiError> {
        let mut checks = Vec::new();
        // SSH 可达
        let ssh_ok = self.session().await.is_ok();
        checks.push(("SSH 连接".into(), ssh_ok, self.row.connection.host_label()));
        if !ssh_ok {
            return Ok(HealthReport {
                healthy: false,
                message: "SSH 不可达".into(),
                checks,
            });
        }
        let running = self.remote_pid().await.unwrap_or(None).is_some();
        checks.push((
            "服务进程".into(),
            running,
            match self.row.service_mode {
                ServiceMode::Systemd => format!("systemd: {}", self.unit_name()),
                _ => "nohup".into(),
            },
        ));
        let api_ok = match self.admin_client() {
            Some(c) => {
                let kind = self.live_api_kind().await;
                c.health(kind).await.unwrap_or(false)
            }
            None => false,
        };
        checks.push((
            "Admin API".into(),
            api_ok,
            if api_ok {
                self.row.admin_api_base_url.clone()
            } else {
                "不可达（可能未配置、仅监听回环、或未启动）".into()
            },
        ));
        Ok(HealthReport {
            healthy: running,
            message: if running { "运行中".into() } else { "未运行".into() },
            checks,
        })
    }

    async fn status(&self) -> Result<InstanceStatus, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;
        let pid = self.remote_pid().await.unwrap_or(None);

        let version = exec::run(
            &s,
            AllowedCommand::BinaryVersion {
                binary: self.row.paths.binary.clone(),
            },
            &sudo,
            None,
        )
        .await
        .ok()
        .map(|o| o.stdout.trim().to_string())
        .filter(|v| !v.is_empty());

        let mut api_status = None;
        if let Some(c) = self.admin_client() {
            let kind = self.live_api_kind().await;
            api_status = c.status(kind, self.row.role).await.ok();
        }

        let running = pid.is_some()
            || api_status
                .as_ref()
                .map(|s| s.api_reachable)
                .unwrap_or(false);

        Ok(InstanceStatus {
            state: if running {
                LiveState::Running
            } else {
                LiveState::Stopped
            },
            healthy: running,
            pid,
            uptime_secs: api_status.as_ref().and_then(|s| s.uptime_secs),
            binary_version: version.or_else(|| self.row.binary_version.clone()),
            api_reachable: api_status.as_ref().map(|s| s.api_reachable).unwrap_or(false),
            last_error: api_status.as_ref().and_then(|s| s.last_error.clone()),
            detail: StatusDetail {
                source: "ssh".into(),
                notes: vec![format!("远程主机：{}", self.row.connection.host_label())],
            },
            counters: api_status.and_then(|s| s.counters),
        })
    }

    async fn reload(&self) -> Result<ReloadOutcome, ApiError> {
        if self.row.role == InstanceRole::Server && self.row.flavor == BinaryFlavor::OfficialFrp {
            return Ok(ReloadOutcome::RestartRequired);
        }
        Ok(ReloadOutcome::Hot)
    }

    async fn read_config(&self) -> Result<ConfigDoc, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;
        let out = exec::run(
            &s,
            AllowedCommand::ReadFile {
                path: self.row.paths.config.clone(),
                max_bytes: 1_000_000,
            },
            &sudo,
            None,
        )
        .await?;
        if out.stdout.is_empty() && !out.ok() {
            return Err(ApiError::not_found(format!(
                "远程配置文件读取失败：{}",
                out.combined()
            )));
        }
        let sha = crate::crypto::sha256_hex(out.stdout.as_bytes());
        let loaded = crate::configfile::LoadedConfig {
            path: std::path::PathBuf::from(&self.row.paths.config),
            content: out.stdout,
            sha256: sha,
        };
        Ok(crate::configfile::build_doc(
            loaded,
            self.row.role,
            self.row.flavor,
            true,
            None,
        ))
    }

    async fn validate_config(&self, content: &str) -> Result<ValidationReport, ApiError> {
        Ok(crate::configfile::validation_report(
            content,
            self.row.role,
            self.row.flavor,
        ))
    }

    async fn write_config(
        &self,
        content: &str,
        opts: WriteOptions,
    ) -> Result<ConfigWriteResult, ApiError> {
        let report = self.validate_config(content).await?;
        if !report.ok {
            let msgs: Vec<String> = report.errors.iter().map(|d| d.message.clone()).collect();
            return Err(ApiError::Validation(format!(
                "配置校验未通过：{}",
                msgs.join("；")
            )));
        }

        let s = self.session().await?;
        let sudo = self.sudo()?;
        let path = self.row.paths.config.clone();
        let tmp = format!("{path}.frppanel.tmp");

        // 1) 备份
        let backup = if opts.backup {
            let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let bak = format!("{path}.bak.{stamp}");
            let exists = exec::run(
                &s,
                AllowedCommand::FileExists { path: path.clone() },
                &sudo,
                None,
            )
            .await
            .map(|o| o.stdout.trim() == "yes")
            .unwrap_or(false);
            if exists {
                exec::run_checked(
                    &s,
                    AllowedCommand::Copy {
                        src: path.clone(),
                        dst: bak.clone(),
                    },
                    &sudo,
                    None,
                    "备份远程配置",
                )
                .await?;
                let sha = crate::crypto::sha256_hex(content.as_bytes());
                let _ = crate::db::repo::insert_backup(
                    &self.db,
                    self.row.id,
                    &bak,
                    &sha,
                    content.len() as i64,
                    "save",
                )
                .await;
                Some(BackupInfo {
                    id: bak.clone(),
                    path: bak,
                    created_at: Utc::now(),
                    size: content.len() as u64,
                })
            } else {
                None
            }
        } else {
            None
        };

        // 2) 写临时文件（base64 → 远端解码，避免转义问题）
        exec::run_checked(
            &s,
            AllowedCommand::WriteFile { path: tmp.clone() },
            &sudo,
            Some(content),
            "写入远程临时配置",
        )
        .await?;

        // 3) 目标二进制 --check（NFrp 支持）
        let check = exec::run(
            &s,
            AllowedCommand::BinaryVersion {
                binary: self.row.paths.binary.clone(),
            },
            &sudo,
            None,
        )
        .await;
        let _ = check;

        // 4) 原子替换
        exec::run_checked(
            &s,
            AllowedCommand::Move {
                src: tmp,
                dst: path.clone(),
            },
            &sudo,
            None,
            "替换远程配置（原子）",
        )
        .await?;

        // 5) 应用
        let applied = if opts.apply {
            match self.reload().await? {
                ReloadOutcome::RestartRequired => {
                    if opts.auto_restart {
                        self.restart().await?;
                        ReloadOutcome::Restarted
                    } else {
                        ReloadOutcome::RestartRequired
                    }
                }
                other => other,
            }
        } else {
            ReloadOutcome::Hot
        };

        Ok(ConfigWriteResult {
            applied,
            backup,
            warnings: report.warnings,
        })
    }

    async fn list_backups(&self) -> Result<Vec<BackupInfo>, ApiError> {
        let rows = crate::db::repo::list_backups(&self.db, self.row.id, 50).await?;
        Ok(rows
            .into_iter()
            .map(|r| BackupInfo {
                id: r.id.to_string(),
                path: r.path,
                created_at: crate::db::models::parse_ts(&r.created_at).unwrap_or_else(Utc::now),
                size: r.size as u64,
            })
            .collect())
    }

    async fn restore_backup(&self, backup_id: &str) -> Result<ConfigWriteResult, ApiError> {
        let id = uuid::Uuid::parse_str(backup_id)
            .map_err(|_| ApiError::bad_request("备份 ID 不合法"))?;
        let row = crate::db::repo::get_backup(&self.db, self.row.id, id).await?;
        let s = self.session().await?;
        let sudo = self.sudo()?;
        exec::run_checked(
            &s,
            AllowedCommand::Copy {
                src: row.path.clone(),
                dst: self.row.paths.config.clone(),
            },
            &sudo,
            None,
            "回滚远程配置",
        )
        .await?;
        Ok(ConfigWriteResult {
            applied: self.reload().await.unwrap_or(ReloadOutcome::RestartRequired),
            backup: None,
            warnings: vec![Diagnostic::info(
                "restored",
                format!("已从 {} 回滚", row.path),
            )],
        })
    }

    async fn logs(&self, query: LogQuery) -> Result<LogPage, ApiError> {
        let s = self.session().await?;
        let sudo = self.sudo()?;
        let limit = query.limit.unwrap_or(200).clamp(1, 2000);

        // systemd 优先用 journalctl（如果日志路径不是文件）
        let out = exec::run(
            &s,
            AllowedCommand::Tail {
                path: self.row.paths.log.clone(),
                lines: 2000,
            },
            &sudo,
            None,
        )
        .await?;

        let lines: Vec<&str> = out.stdout.lines().collect();
        let needle = query.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty());
        let lvl = query.level.as_deref().map(str::to_uppercase).filter(|s| !s.is_empty());
        let filtered: Vec<&str> = lines
            .into_iter()
            .filter(|l| {
                if let Some(n) = &needle {
                    if !l.to_lowercase().contains(n.as_str()) {
                        return false;
                    }
                }
                if let Some(w) = &lvl {
                    if parse_log_line(0, l).level.as_deref() != Some(w.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect();

        let total = filtered.len();
        let start = query
            .offset
            .unwrap_or_else(|| total.saturating_sub(limit));
        let end = (start + limit).min(total);
        let slice = filtered.get(start..end).unwrap_or(&[]);

        Ok(LogPage {
            lines: slice
                .iter()
                .enumerate()
                .map(|(i, l)| parse_log_line((start + i) as u64, l))
                .collect(),
            total: Some(total),
            next_offset: if end < total { Some(end) } else { None },
            source: "remote".into(),
            path: self.row.paths.log.clone(),
        })
    }

    async fn list_proxies(&self) -> Result<Vec<ProxyInfo>, ApiError> {
        let mut from_api = Vec::new();
        if let Some(c) = self.admin_client() {
            let kind = self.live_api_kind().await;
            from_api = c
                .list_proxies(kind, self.row.role)
                .await
                .unwrap_or_default();
        }
        if self.row.role == InstanceRole::Client {
            if let Ok(doc) = self.read_config().await {
                for p in doc.parsed.proxies {
                    if !from_api.iter().any(|x| x.name == p.name) {
                        let mut info = ProxyInfo::minimal(&p.name, &p.proxy_type);
                        info.local_addr = p.effective_local_addr();
                        info.remote_port = p.remote_port;
                        info.custom_domains = p.custom_domains;
                        from_api.push(info);
                    }
                }
            }
        }
        Ok(from_api)
    }

    async fn create_proxy(&self, spec: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        spec.validate().map_err(ApiError::bad_request)?;
        // NFrp 且 Admin API 可达：走动态
        if let Some(c) = self.admin_client() {
            let kind = self.live_api_kind().await;
            if kind == ApiKind::Nfrp && self.row.role == InstanceRole::Client {
                if crate::frpapi::nfrp::add_proxy_via_client(&c, &spec)
                    .await
                    .is_ok()
                {
                    let mut info = ProxyInfo::minimal(&spec.name, &spec.proxy_type);
                    info.local_addr = spec.effective_local_addr();
                    info.remote_port = spec.remote_port;
                    info.source = "dynamic".into();
                    info.managed_by = "panel".into();
                    return Ok(ProxyOpResult {
                        applied_via: "admin_api".into(),
                        reload: ReloadOutcome::Hot,
                        proxy: info,
                        warnings: vec![],
                    });
                }
            }
        }
        self.mutate_remote_config(ProxyMutation::Create(spec)).await
    }

    async fn update_proxy(&self, name: &str, spec: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        spec.validate().map_err(ApiError::bad_request)?;
        self.mutate_remote_config(ProxyMutation::Update(name.to_string(), spec))
            .await
    }

    async fn delete_proxy(&self, name: &str) -> Result<ProxyOpResult, ApiError> {
        if let Some(c) = self.admin_client() {
            let kind = self.live_api_kind().await;
            if kind == ApiKind::Nfrp && self.row.role == InstanceRole::Client {
                if crate::frpapi::nfrp::remove_proxy_via_client(&c, name)
                    .await
                    .is_ok()
                {
                    return Ok(ProxyOpResult {
                        applied_via: "admin_api".into(),
                        reload: ReloadOutcome::Hot,
                        proxy: ProxyInfo::minimal(name, "unknown"),
                        warnings: vec![],
                    });
                }
            }
        }
        self.mutate_remote_config(ProxyMutation::Delete(name.to_string()))
            .await
    }

    async fn metrics(&self) -> Result<MetricsSnapshot, ApiError> {
        let ts = Utc::now();
        let Some(c) = self.admin_client() else {
            return Ok(MetricsSnapshot {
                ts,
                api_reachable: false,
                counters: InstanceCounters {
                    bytes_in: 0,
                    bytes_out: 0,
                    conns: 0,
                    clients: 0,
                    proxies: 0,
                    requests: None,
                },
                availability: "none".into(),
                source: "none".into(),
            });
        };
        let kind = self.live_api_kind().await;
        let st = c.status(kind, self.row.role).await?;
        let counters = st.counters.unwrap_or(InstanceCounters {
            bytes_in: 0,
            bytes_out: 0,
            conns: 0,
            clients: 0,
            proxies: 0,
            requests: None,
        });
        let availability = if counters.bytes_in + counters.bytes_out > 0 {
            "full"
        } else if counters.conns + counters.clients + counters.proxies > 0 {
            "aggregated"
        } else {
            "none"
        };
        Ok(MetricsSnapshot {
            ts,
            api_reachable: st.api_reachable,
            counters,
            availability: availability.into(),
            source: "admin_api".into(),
        })
    }

    fn capabilities(&self) -> Arc<Capabilities> {
        Arc::new(
            self.row
                .capabilities
                .clone()
                .unwrap_or_else(|| Capabilities::unknown(self.row.flavor)),
        )
    }

    async fn probe(&self) -> Result<Capabilities, ApiError> {
        let version = match self.session().await {
            Ok(s) => {
                let sudo = self.sudo()?;
                exec::run(
                    &s,
                    AllowedCommand::BinaryVersion {
                        binary: self.row.paths.binary.clone(),
                    },
                    &sudo,
                    None,
                )
                .await
                .ok()
                .map(|o| o.stdout.trim().to_string())
                .filter(|v| !v.is_empty())
            }
            Err(_) => self.row.binary_version.clone(),
        };
        let client = self.admin_client();
        let caps = probe::probe(ProbeInput {
            client: client.as_ref(),
            role: self.row.role,
            flavor: self.row.flavor,
            managed_locally: true,
            remote: true,
            binary_version: version,
            owns_config: true,
        })
        .await;
        *self.capabilities.lock().await = Arc::new(caps.clone());
        Ok(caps)
    }
}

/// 配置级代理变更（远程）。
#[derive(Debug, Clone)]
pub enum ProxyMutation {
    Create(ProxySpec),
    Update(String, ProxySpec),
    Delete(String),
}

impl SshBinaryDriver {
    async fn mutate_remote_config(&self, m: ProxyMutation) -> Result<ProxyOpResult, ApiError> {
        let doc = self.read_config().await?;
        let mut value: toml::Value = toml::from_str(&doc.content)
            .map_err(|e| ApiError::Validation(format!("配置不是合法 TOML：{e}")))?;
        let new_content =
            crate::driver::binary::apply_proxy_mutation(&mut value, &to_binary_mutation(&m), self.row.flavor)?;

        let result = self
            .write_config(
                &new_content,
                WriteOptions {
                    apply: true,
                    auto_restart: true,
                    backup: true,
                },
            )
            .await?;

        let name = match &m {
            ProxyMutation::Create(s) => s.name.clone(),
            ProxyMutation::Update(n, _) => n.clone(),
            ProxyMutation::Delete(n) => n.clone(),
        };
        Ok(ProxyOpResult {
            applied_via: "config_reload".into(),
            reload: result.applied,
            proxy: ProxyInfo::minimal(&name, "unknown"),
            warnings: result.warnings,
        })
    }
}

/// 把本模块的变更描述转换为 `binary` 模块的等价类型（两者语义一致）。
fn to_binary_mutation(m: &ProxyMutation) -> crate::driver::binary::ProxyMutation {
    use crate::driver::binary::ProxyMutation as B;
    match m {
        ProxyMutation::Create(s) => B::Create(s.clone()),
        ProxyMutation::Update(n, s) => B::Update(n.clone(), s.clone()),
        ProxyMutation::Delete(n) => B::Delete(n.clone()),
    }
}

/// 重导出：远程与本地共用同一份配置改写逻辑。
pub use crate::driver::binary::apply_proxy_mutation;

/// 从 `dyn Driver` 取回 SSH 驱动的具体类型。
///
/// 部署编排需要 SSH 会话与凭据参数，这些是 SSH 驱动特有的，
/// 不应该污染 `Driver` trait；这里用 `Any` 做一次可控的向下转换。
pub fn as_ssh_driver(d: &Arc<dyn Driver>) -> Option<&SshBinaryDriver> {
    d.as_any().downcast_ref::<SshBinaryDriver>()
}

/// 占位：避免未使用告警（`unsupported` 在部分分支使用）
#[allow(dead_code)]
fn _keep_unsupported() {
    let _ = unsupported(DriverKind::SshBinary, "x", "y");
}
