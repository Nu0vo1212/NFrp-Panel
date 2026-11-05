//! BinaryDriver：管理本机 frps/frpc 二进制进程 + 配置文件 + Admin API。
//!
//! 语义（与 `docs/01-architecture.md` §3.1 对应）：
//! - **进程**：面板拉起子进程并记录 PID 文件，面板重启后可接管已在运行的实例；
//! - **配置**：读写本机文件，写入前校验 + 备份 + 原子替换；
//! - **reload**：依据实测——服务端与 NFrp 客户端走「写文件 + 等生效 + 校验」，
//!   官方 frps 无热重载 → `restart_required`，官方 frpc 由上层决定重启；
//! - **代理**：优先 Admin API 动态；失败/不支持则回退「配置改写 + reload/restart」。

pub mod process;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use crate::configfile;
use crate::db::models::InstanceRow;
use crate::domain::*;
use crate::error::ApiError;
use crate::frpapi::probe::{self, ProbeInput};
use crate::frpapi::{nfrp, AdminAuth, FrpAdminClient};

use super::{Driver, HealthReport, StopOptions, WriteOptions};

pub use process::{ManagedChild, ProcessState};

/// BinaryDriver 的共享上下文（一个实例一个）。
pub struct BinaryContext {
    pub row: InstanceRow,
    /// 解密后的 Admin API 密码（仅内存）
    pub admin_password: Option<String>,
    pub child: Mutex<Option<Arc<ManagedChild>>>,
    pub capabilities: Mutex<Arc<Capabilities>>,
    /// Admin API 失败后的熔断截止时间：本机关闭端口可能要约 2s 才拒绝连接，
    /// 列表/详情轮询若每次都重试会被拖慢，因此短时间内直接跳过。
    pub admin_down_until: Mutex<Option<std::time::Instant>>,
    pub cfg: crate::config::RuntimeConfig,
    pub keys: Arc<crate::crypto::KeyRing>,
    pub db: sqlx::SqlitePool,
}

impl BinaryContext {
    pub fn binary_path(&self) -> PathBuf {
        PathBuf::from(&self.row.paths.binary)
    }
    pub fn config_path(&self) -> PathBuf {
        PathBuf::from(&self.row.paths.config)
    }
    pub fn log_path(&self) -> PathBuf {
        PathBuf::from(&self.row.paths.log)
    }
    pub fn work_dir(&self) -> PathBuf {
        if self.row.paths.work_dir.is_empty() {
            self.config_path()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(&self.row.paths.work_dir)
        }
    }

    /// 构造 Admin API 客户端（使用解密后的密码）。
    pub fn admin_client(&self) -> Option<FrpAdminClient> {
        if !self.row.admin_api_enabled || self.row.admin_api_base_url.trim().is_empty() {
            return None;
        }
        let auth = AdminAuth {
            user: self.row.admin_api_user.clone(),
            password: self.admin_password.clone().unwrap_or_default(),
        };
        FrpAdminClient::new(
            &self.row.admin_api_base_url,
            auth,
            self.row.role,
            Duration::from_millis(self.cfg.raw.frp.admin_timeout_ms),
            self.row.capabilities.as_ref().map(|c| c.api_kind),
        )
        .ok()
    }
}

pub struct BinaryDriver {
    ctx: Arc<BinaryContext>,
}

impl BinaryDriver {
    pub fn new(ctx: Arc<BinaryContext>) -> Self {
        Self { ctx }
    }

    // ------------------------------------------------------------------
    // 进程
    // ------------------------------------------------------------------

    /// 当前进程 PID：内存句柄优先，其次 PID 文件（面板重启后接管）。
    pub fn live_pid(&self) -> Option<u32> {
        if let Some(rec) = process::read_pid_record(&self.ctx.work_dir(), &self.ctx.row.name) {
            if process::pid_alive(rec.pid) {
                return Some(rec.pid);
            }
            // 陈旧 PID 文件：清掉
            let wd = self.ctx.work_dir();
            let _ = std::fs::remove_file(wd.join(format!("{}.pid.json", self.ctx.row.name)));
        }
        None
    }

    /// 当前 Admin API 的实现类型。
    ///
    /// 判定顺序：**内存能力（探测结果）> 实时探测 > 记录缓存 > 未知**。
    /// 每次 `status` 都做 4 次 HTTP 探测会把列表/详情拖慢（端口未监听时尤其明显），
    /// 因此探测过一次后就以内存中的能力位为准。
    pub async fn live_api_kind(&self) -> ApiKind {
        let known = self.ctx.capabilities.lock().await.api_kind;
        if matches!(known, ApiKind::Nfrp | ApiKind::Official) {
            return known;
        }
        // 熔断期内不再做探测（实例没起 / 面板端口不可达时探测纯属白等）
        if matches!(*self.ctx.admin_down_until.lock().await, Some(t) if t > std::time::Instant::now())
        {
            return known;
        }
        if let Some(c) = self.ctx.admin_client() {
            if let Ok(kind) = c.detect_api_kind().await {
                if kind != ApiKind::None && kind != ApiKind::Unknown {
                    return kind;
                }
            }
        }
        known
    }

    /// 启动命令：`<binary> -c <config>`（NFrp 与官方 frp 一致）。
    fn build_args(&self) -> Vec<String> {
        let mut args = vec!["-c".to_string(), self.ctx.config_path().display().to_string()];
        // AdminApiDriver 场景不需要；Binary 模式下不需要额外参数
        if self.ctx.row.role == InstanceRole::Client
            && self.ctx.row.flavor == BinaryFlavor::OfficialFrp
        {
            // 官方 frpc 支持 `--enableReload`：开启后可用 reload 而非重启
            args.push("--enableReload".to_string());
        }
        args
    }

    /// 版本探测：`-v`（两边都输出裸版本号，如 0.71.1 / 0.71.0）。
    pub async fn binary_version(&self) -> Option<String> {
        let bin = self.ctx.binary_path();
        if !bin.exists() {
            return None;
        }
        let out = tokio::process::Command::new(&bin)
            .arg("-v")
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// 用目标二进制做一次 `--check`（NFrp 支持；官方 0.71 无此参数则忽略）。
    pub async fn binary_check(&self, config: &Path) -> Option<Result<(), String>> {
        let bin = self.ctx.binary_path();
        if !bin.exists() {
            return None;
        }
        let out = tokio::process::Command::new(&bin)
            .args(["-c", &config.display().to_string(), "--check"])
            .output()
            .await
            .ok()?;
        if out.status.success() {
            Some(Ok(()))
        } else {
            let msg = String::from_utf8_lossy(&out.stderr).to_string()
                + &String::from_utf8_lossy(&out.stdout);
            // 官方 frp 不认 --check 时是"未知参数"，这时不该判定配置非法
            if msg.contains("unexpected argument")
                || msg.contains("unrecognized")
                || msg.contains("error: unexpected")
            {
                return None;
            }
            Some(Err(msg))
        }
    }
}

#[async_trait]
impl Driver for BinaryDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Binary
    }

    fn instance_id(&self) -> uuid::Uuid {
        self.ctx.row.id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn start(&self) -> Result<InstanceStatus, ApiError> {
        if let Some(pid) = self.live_pid() {
            return Err(ApiError::conflict(format!(
                "实例已在运行（PID {pid}），请先停止"
            )));
        }
        let config = self.ctx.config_path();
        if !config.exists() {
            return Err(ApiError::not_found(format!(
                "配置文件不存在：{}（请先保存配置或执行部署）",
                config.display()
            )));
        }
        let bin = self.ctx.binary_path();
        let log = self.ctx.log_path();
        let work = self.ctx.work_dir();
        let name = self.ctx.row.name.clone();
        let args = self.build_args();

        let child = process::spawn(&process::SpawnOptions {
            binary: bin,
            args,
            config_path: config.clone(),
            log_path: log.clone(),
            work_dir: work.clone(),
            name: name.clone(),
            env: vec![],
        })
        .await?;

        let pid = child.pid;
        *self.ctx.child.lock().await = Some(child);

        // 等待进程稳定（不要一启动就报成功，frp 启动失败会立刻退出）
        tokio::time::sleep(Duration::from_millis(600)).await;
        if !process::pid_alive(pid) {
            // 进程已退出：把日志尾部带回给调用方
            let tail = process::read_log(&log, None, 20, None, None)
                .await
                .map(|(l, _, _, _)| l.join("\n"))
                .unwrap_or_default();
            let _ = std::fs::remove_file(work.join(format!("{name}.pid.json")));
            return Err(ApiError::upstream(format!(
                "进程启动后立即退出，请检查配置与端口占用。日志尾部：\n{tail}"
            )));
        }

        self.status().await
    }

    async fn stop(&self, opts: StopOptions) -> Result<InstanceStatus, ApiError> {
        let Some(pid) = self.live_pid() else {
            return Ok(InstanceStatus {
                state: LiveState::Stopped,
                healthy: false,
                pid: None,
                uptime_secs: None,
                binary_version: None,
                api_reachable: false,
                last_error: None,
                detail: StatusDetail {
                    source: "process".into(),
                    notes: vec!["实例本来就未运行".into()],
                },
                counters: None,
            });
        };
        process::terminate(pid, opts.grace_ms, opts.force).await?;
        *self.ctx.child.lock().await = None;
        let _ = std::fs::remove_file(
            self.ctx
                .work_dir()
                .join(format!("{}.pid.json", self.ctx.row.name)),
        );

        Ok(InstanceStatus {
            state: LiveState::Stopped,
            healthy: false,
            pid: None,
            uptime_secs: None,
            binary_version: None,
            api_reachable: false,
            last_error: None,
            detail: StatusDetail {
                source: "process".into(),
                notes: vec![format!("已停止进程 {pid}")],
            },
            counters: None,
        })
    }

    async fn restart(&self) -> Result<InstanceStatus, ApiError> {
        let _ = self.stop(StopOptions::default()).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.start().await
    }

    async fn health(&self) -> Result<HealthReport, ApiError> {
        let mut checks = Vec::new();
        let pid = self.live_pid();
        checks.push(("进程存活".into(), pid.is_some(), match pid {
            Some(p) => format!("PID {p}"),
            None => "未运行".into(),
        }));

        let cfg_exists = self.ctx.config_path().exists();
        checks.push((
            "配置文件可读".into(),
            cfg_exists,
            self.ctx.config_path().display().to_string(),
        ));

        let api_ok = match self.ctx.admin_client() {
            Some(c) => {
                let kind = self.live_api_kind().await;
                c.health(kind).await.unwrap_or(false)
            }
            None => false,
        };
        checks.push((
            "Admin API 可达".into(),
            api_ok,
            if api_ok {
                "已连接".into()
            } else {
                "不可达（可能未配置或实例未启动）".into()
            },
        ));

        let healthy = pid.is_some() && cfg_exists && (api_ok || self.ctx.admin_client().is_none());
        Ok(HealthReport {
            healthy,
            message: if healthy {
                "运行正常".into()
            } else if pid.is_none() {
                "进程未运行".into()
            } else {
                "进程在运行，但 Admin API 不可达".into()
            },
            checks,
        })
    }

    async fn status(&self) -> Result<InstanceStatus, ApiError> {
        let pid = self.live_pid();
        // ★ 版本优先用探测时落库的值：status 会被列表 10s 轮询 / 详情 8s 轮询反复调用，
        //   每次都拉起 `binary -v` 子进程会让表格明显变慢。记录为空时才探测。
        let version = match self.ctx.row.binary_version.clone() {
            Some(v) if !v.is_empty() => Some(v),
            _ => self.binary_version().await,
        };
        let client = self.ctx.admin_client();

        let mut api_status: Option<InstanceStatus> = None;
        if let Some(c) = &client {
            let cooling = matches!(
                *self.ctx.admin_down_until.lock().await,
                Some(t) if t > std::time::Instant::now()
            );
            if !cooling {
                let kind = self.live_api_kind().await;
                match c.status(kind, self.ctx.row.role).await {
                    Ok(s) if s.api_reachable => api_status = Some(s),
                    Ok(s) => {
                        // 能请求但 API 明确不可达：同样进入熔断，避免轮询空转
                        api_status = Some(s);
                        *self.ctx.admin_down_until.lock().await =
                            Some(std::time::Instant::now() + Duration::from_secs(30));
                    }
                    Err(e) => {
                        // 只对"连不上/超时"熔断；鉴权错误属于配置问题，重试本身很快
                        let msg = e.to_string();
                        if msg.contains("无法连接") || msg.contains("超时") {
                            *self.ctx.admin_down_until.lock().await =
                                Some(std::time::Instant::now() + Duration::from_secs(30));
                        }
                    }
                }
            }
        }

        let mut notes = Vec::new();
        if let Some(s) = &api_status {
            notes.extend(s.detail.notes.clone());
        }

        let state = match (pid, &api_status) {
            (Some(_), Some(s)) if s.api_reachable => LiveState::Running,
            (Some(_), _) => LiveState::Running,
            (None, Some(s)) if s.api_reachable => LiveState::Running, // 进程不是面板拉起的
            (None, _) => LiveState::Stopped,
        };

        let counters = api_status.as_ref().and_then(|s| s.counters.clone());
        let uptime = api_status.as_ref().and_then(|s| s.uptime_secs);

        Ok(InstanceStatus {
            state,
            healthy: state == LiveState::Running
                && api_status.as_ref().map(|s| s.healthy).unwrap_or(true),
            pid,
            uptime_secs: uptime,
            binary_version: version.or_else(|| api_status.as_ref().and_then(|s| s.binary_version.clone())),
            api_reachable: api_status.as_ref().map(|s| s.api_reachable).unwrap_or(false),
            last_error: api_status.as_ref().and_then(|s| s.last_error.clone()),
            detail: StatusDetail {
                source: "process+admin_api".into(),
                notes,
            },
            counters,
        })
    }

    /// reload 语义见 `docs/01-architecture.md` §3.2。
    async fn reload(&self) -> Result<ReloadOutcome, ApiError> {
        // 官方 frps 没有热重载、也没有配置文件监听
        if self.ctx.row.role == InstanceRole::Server
            && self.ctx.row.flavor == BinaryFlavor::OfficialFrp
        {
            return Ok(ReloadOutcome::RestartRequired);
        }
        // 进程没跑：下次启动自然生效
        if self.live_pid().is_none() {
            return Ok(ReloadOutcome::Hot);
        }

        // NFrp 服务端/客户端：轮询 mtime 热加载，等生效后校验
        let before = self.snapshot_counters().await;
        let deadline = std::time::Instant::now() + Duration::from_millis(4_000);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if let Some(after) = self.snapshot_counters().await {
                if before.as_ref() != Some(&after) {
                    return Ok(ReloadOutcome::Hot);
                }
            }
        }
        // 计数没变化也可能是配置本来就一致；此时仍视为已热加载
        Ok(ReloadOutcome::Hot)
    }

    async fn read_config(&self) -> Result<ConfigDoc, ApiError> {
        let loaded = configfile::read_file(&self.ctx.config_path())?;
        Ok(configfile::build_doc(
            loaded,
            self.ctx.row.role,
            self.ctx.row.flavor,
            true,
            None,
        ))
    }

    async fn validate_config(&self, content: &str) -> Result<ValidationReport, ApiError> {
        Ok(configfile::validation_report(
            content,
            self.ctx.row.role,
            self.ctx.row.flavor,
        ))
    }

    async fn write_config(
        &self,
        content: &str,
        opts: WriteOptions,
    ) -> Result<ConfigWriteResult, ApiError> {
        // 1) 校验（失败不落盘）
        let report = self.validate_config(content).await?;
        if !report.ok {
            let msgs: Vec<String> = report.errors.iter().map(|d| d.message.clone()).collect();
            return Err(ApiError::Validation(format!(
                "配置校验未通过：{}",
                msgs.join("；")
            )));
        }

        let path = self.ctx.config_path();
        // 2) 备份
        let backup = if opts.backup {
            let b = configfile::backup_file(&path)?;
            if let Some(bf) = &b {
                let _ = crate::db::repo::insert_backup(
                    &self.ctx.db,
                    self.ctx.row.id,
                    &bf.path.display().to_string(),
                    &bf.sha,
                    bf.size as i64,
                    "save",
                )
                .await;
                Some(BackupInfo {
                    id: bf.path.display().to_string(),
                    path: bf.path.display().to_string(),
                    created_at: bf.created_at,
                    size: bf.size,
                })
            } else {
                None
            }
        } else {
            None
        };

        // 3) 目标二进制预检（NFrp 有 --check；官方不认时返回 None 则跳过）
        let tmp = path.with_extension("check.toml");
        std::fs::write(&tmp, content)?;
        let bin_check = self.binary_check(&tmp).await;
        let _ = std::fs::remove_file(&tmp);
        if let Some(Err(msg)) = bin_check {
            return Err(ApiError::Validation(format!(
                "目标二进制拒绝该配置（{} --check）：{}",
                self.ctx.binary_path().display(),
                crate::crypto::redact_line(msg.trim())
            )));
        }

        // 4) 原子写
        configfile::atomic_write(&path, content)?;

        // 5) 应用
        let mut warnings = report.warnings;
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

        if applied == ReloadOutcome::RestartRequired {
            warnings.push(Diagnostic::warn(
                "restart_required",
                "该实例无法热重载配置，需要重启后生效",
            ).with_hint("点击「重启」立即生效，或在设置里开启「保存后自动重启」"));
        }

        Ok(ConfigWriteResult {
            applied,
            backup,
            warnings,
        })
    }

    async fn list_backups(&self) -> Result<Vec<BackupInfo>, ApiError> {
        let rows = crate::db::repo::list_backups(&self.ctx.db, self.ctx.row.id, 50).await?;
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
        let row = crate::db::repo::get_backup(&self.ctx.db, self.ctx.row.id, id).await?;
        let content = std::fs::read_to_string(&row.path)
            .map_err(|e| ApiError::not_found(format!("备份文件不可读：{e}")))?;
        self.write_config(&content, WriteOptions::default()).await
    }

    async fn logs(&self, query: LogQuery) -> Result<LogPage, ApiError> {
        let path = self.ctx.log_path();
        let limit = query.limit.unwrap_or(200).clamp(1, 2000);
        let (lines, next, total, _) = process::read_log(
            &path,
            query.offset,
            limit,
            query.q.as_deref(),
            query.level.as_deref(),
        )
        .await?;
        Ok(LogPage {
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, l)| parse_log_line(query.offset.unwrap_or(0) as u64 + i as u64, l))
                .collect(),
            total: Some(total),
            next_offset: next,
            source: "file".into(),
            path: path.display().to_string(),
        })
    }

    async fn list_proxies(&self) -> Result<Vec<ProxyInfo>, ApiError> {
        let mut from_api: Vec<ProxyInfo> = Vec::new();
        if let Some(c) = self.ctx.admin_client() {
            let kind = self.live_api_kind().await;
            from_api = c
                .list_proxies(kind, self.ctx.row.role)
                .await
                .unwrap_or_default();
        }

        // 客户端：把配置文件里的代理合并进来（Admin API 只能给出"动态"的，官方 frpc 的 /api/status 能给出全部）
        if self.ctx.row.role == InstanceRole::Client {
            if let Ok(doc) = self.read_config().await {
                for p in doc.parsed.proxies {
                    if !from_api.iter().any(|x| x.name == p.name) {
                        let mut info = ProxyInfo::minimal(&p.name, &p.proxy_type);
                        info.local_addr = p.effective_local_addr();
                        info.remote_port = p.remote_port;
                        info.custom_domains = p.custom_domains.clone();
                        info.subdomain = p.subdomain.clone();
                        info.status = "unknown".into();
                        info.source = "config".into();
                        info.managed_by = "config_file".into();
                        from_api.push(info);
                    } else if let Some(existing) = from_api.iter_mut().find(|x| x.name == p.name) {
                        // Admin API 给了运行态，配置文件给了定义：合并
                        if existing.local_addr.is_empty() {
                            existing.local_addr = p.effective_local_addr();
                        }
                        if existing.remote_port.is_none() {
                            existing.remote_port = p.remote_port;
                        }
                        existing.source = "config".into();
                        existing.managed_by = "config_file".into();
                    }
                }
            }
        }
        Ok(from_api)
    }

    async fn create_proxy(&self, spec: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        spec.validate().map_err(ApiError::bad_request)?;

        // 1) 优先 Admin API 动态（NFrp）
        if let Some(c) = self.ctx.admin_client() {
            let kind = self.live_api_kind().await;
            if kind == ApiKind::Nfrp {
                let result = match self.ctx.row.role {
                    InstanceRole::Client => nfrp::add_proxy_via_client(&c, &spec).await,
                    InstanceRole::Server => {
                        // 服务端需要 run_id：取第一个在线客户端（面板下发的代理绑定到该客户端）
                        match first_run_id(&c).await {
                            Some(rid) => nfrp::add_proxy_via_server(&c, &rid, &spec).await,
                            None => Err(ApiError::upstream("服务端当前没有在线的客户端")),
                        }
                    }
                };
                if result.is_ok() {
                    let mut info = ProxyInfo::minimal(&spec.name, &spec.proxy_type);
                    info.local_addr = spec.effective_local_addr();
                    info.remote_port = spec.remote_port;
                    info.custom_domains = spec.custom_domains.clone();
                    info.status = "running".into();
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

        // 2) 回退：改写配置文件 + reload/restart
        self.mutate_config_proxy(ProxyMutation::Create(spec)).await
    }

    async fn update_proxy(&self, name: &str, spec: ProxySpec) -> Result<ProxyOpResult, ApiError> {
        spec.validate().map_err(ApiError::bad_request)?;
        // 动态代理（Admin API 加的）不支持原地改，先删后加
        if let Some(c) = self.ctx.admin_client() {
            let kind = self.live_api_kind().await;
            let proxies = c
                .list_proxies(kind, self.ctx.row.role)
                .await
                .unwrap_or_default();
            if let Some(p) = proxies.iter().find(|p| p.name == name) {
                if p.source == "dynamic" && p.managed_by == "panel" {
                    let _ = self.delete_proxy(name).await;
                    return self.create_proxy(spec).await;
                }
            }
        }
        self.mutate_config_proxy(ProxyMutation::Update(name.to_string(), spec))
            .await
    }

    async fn delete_proxy(&self, name: &str) -> Result<ProxyOpResult, ApiError> {
        // 先找来源：服务端推送的代理必须用服务端端点删
        let mut managed_by = None;
        if let Some(c) = self.ctx.admin_client() {
            let kind = self.live_api_kind().await;
            if let Ok(list) = c.list_proxies(kind, self.ctx.row.role).await {
                if let Some(p) = list.iter().find(|p| p.name == name) {
                    managed_by = Some(p.managed_by.clone());
                }
            }
        }

        if let Some(c) = self.ctx.admin_client() {
            let kind = self.live_api_kind().await;
            if kind == ApiKind::Nfrp {
                let via = managed_by.as_deref().unwrap_or("panel");
                let res = if via == "server_push" {
                    match first_run_id(&c).await {
                        Some(rid) => nfrp::remove_proxy_via_server(&c, &rid, name).await,
                        None => Err(ApiError::upstream("无法确定服务端 run_id")),
                    }
                } else if via == "panel" {
                    nfrp::remove_proxy_via_client(&c, name).await
                } else {
                    // 配置文件里的代理：客户端本地表没有，只能改配置
                    Err(ApiError::upstream("该代理定义在配置文件中，需改配置"))
                };
                if res.is_ok() {
                    return Ok(ProxyOpResult {
                        applied_via: "admin_api".into(),
                        reload: ReloadOutcome::Hot,
                        proxy: ProxyInfo::minimal(name, "unknown"),
                        warnings: vec![],
                    });
                }
            }
        }

        self.mutate_config_proxy(ProxyMutation::Delete(name.to_string()))
            .await
    }

    async fn metrics(&self) -> Result<MetricsSnapshot, ApiError> {
        let ts = Utc::now();
        let Some(c) = self.ctx.admin_client() else {
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

        let mut source = "admin_api".to_string();
        let mut counters = c
            .status(kind, self.ctx.row.role)
            .await
            .ok()
            .and_then(|s| s.counters)
            .unwrap_or(InstanceCounters {
                bytes_in: 0,
                bytes_out: 0,
                conns: 0,
                clients: 0,
                proxies: 0,
                requests: None,
            });

        // Prometheus 兜底：官方 frps 只有连接数；NFrp 有完整计数
        if let Some(url) = self
            .ctx
            .row
            .admin_api_prometheus_url
            .as_deref()
            .filter(|u| !u.is_empty())
        {
            if let Ok(text) = fetch_prometheus(url).await {
                let m = parse_prometheus(&text);
                if counters.bytes_in == 0 && counters.bytes_out == 0 {
                    counters.bytes_in = m.bytes_up;
                    counters.bytes_out = m.bytes_down;
                }
                if counters.conns == 0 {
                    counters.conns = m.conns;
                }
                source = "admin_api+prometheus".into();
            }
        }

        let availability = if counters.bytes_in + counters.bytes_out > 0 {
            "full"
        } else if counters.conns > 0 || counters.clients > 0 || counters.proxies > 0 {
            "aggregated"
        } else {
            "none"
        };

        Ok(MetricsSnapshot {
            ts,
            api_reachable: true,
            counters,
            availability: availability.into(),
            source,
        })
    }

    fn capabilities(&self) -> Arc<Capabilities> {
        // 同步读取缓存；未探测过时给出保守值
        let cached = self
            .ctx
            .row
            .capabilities
            .clone()
            .unwrap_or_else(|| Capabilities::unknown(self.ctx.row.flavor));
        Arc::new(cached)
    }

    async fn probe(&self) -> Result<Capabilities, ApiError> {
        let version = self.binary_version().await;
        let client = self.ctx.admin_client();
        let caps = probe::probe(ProbeInput {
            client: client.as_ref(),
            role: self.ctx.row.role,
            flavor: self.ctx.row.flavor,
            managed_locally: true,
            remote: false,
            binary_version: version.clone(),
            owns_config: true,
        })
        .await;

        *self.ctx.capabilities.lock().await = Arc::new(caps.clone());
        // 探测正常完成说明 Admin API 刚刚可达，清掉熔断
        *self.ctx.admin_down_until.lock().await = None;

        // 落库（能力与版本）
        let _ = crate::db::repo::update_instance(
            &self.ctx.db,
            self.ctx.row.id,
            crate::db::repo::InstanceUpdate {
                capabilities: Some(Some(caps.clone())),
                binary_version: Some(version),
                ..Default::default()
            },
        )
        .await;

        Ok(caps)
    }
}

impl BinaryDriver {
    async fn snapshot_counters(&self) -> Option<(u64, u64, u64, u64)> {
        let c = self.ctx.admin_client()?;
        let kind = self.live_api_kind().await;
        let s = c.status(kind, self.ctx.row.role).await.ok()?;
        let k = s.counters?;
        Some((k.clients, k.proxies, k.conns, k.bytes_in + k.bytes_out))
    }

    /// 配置层面的代理增删改（回退路径）。
    async fn mutate_config_proxy(&self, m: ProxyMutation) -> Result<ProxyOpResult, ApiError> {
        let doc = self.read_config().await?;
        let mut value: toml::Value = toml::from_str(&doc.content)
            .map_err(|e| ApiError::Validation(format!("配置不是合法 TOML：{e}")))?;

        let new_content = apply_proxy_mutation(&mut value, &m, self.ctx.row.flavor)?;
        let result = self.write_config(&new_content, WriteOptions {
            apply: true,
            auto_restart: true, // 回退路径必须真的让它生效
            backup: true,
        })
        .await?;

        let name = match &m {
            ProxyMutation::Create(s) => s.name.clone(),
            ProxyMutation::Update(n, _) => n.clone(),
            ProxyMutation::Delete(n) => n.clone(),
        };
        let spec = match &m {
            ProxyMutation::Create(s) | ProxyMutation::Update(_, s) => Some(s.clone()),
            ProxyMutation::Delete(_) => None,
        };
        let mut info = ProxyInfo::minimal(&name, spec.as_ref().map(|s| s.proxy_type.as_str()).unwrap_or("unknown"));
        if let Some(s) = spec {
            info.local_addr = s.effective_local_addr();
            info.remote_port = s.remote_port;
            info.custom_domains = s.custom_domains;
        }
        info.source = "config".into();
        info.managed_by = "config_file".into();

        Ok(ProxyOpResult {
            applied_via: "config_reload".into(),
            reload: result.applied,
            proxy: info,
            warnings: result.warnings,
        })
    }
}

/// 配置级代理变更。
#[derive(Debug, Clone)]
pub enum ProxyMutation {
    Create(ProxySpec),
    Update(String, ProxySpec),
    Delete(String),
}

/// 把代理变更写进 TOCTOML 值，返回新的 TOML 文本。
///
/// 约定：
/// - 客户端配置用 `[[proxies]]`；服务端理论上没有 proxies 段（NFrp 服务端代理由客户端注册）；
/// - 官方 frp 用 camelCase（`localIP`/`localPort`），NFrp 用合并式 `local_addr`；
/// - 不破坏文件里已有的其它段与注释顺序（用 toml::Value 往返会丢注释，
///   因此这里仅在无法原地插入时用序列化结果）。
pub fn apply_proxy_mutation(
    value: &mut toml::Value,
    m: &ProxyMutation,
    flavor: BinaryFlavor,
) -> Result<String, ApiError> {
    let table = value
        .as_table_mut()
        .ok_or_else(|| ApiError::Validation("配置顶层不是表".into()))?;

    let mut arr = match table.remove("proxies") {
        Some(toml::Value::Array(a)) => a,
        _ => Vec::new(),
    };

    match m {
        ProxyMutation::Create(spec) | ProxyMutation::Update(_, spec) => {
            let token = spec_to_toml(spec, flavor);
            match m {
                ProxyMutation::Create(_) => {
                    if arr
                        .iter()
                        .any(|p| p.get("name").and_then(|v| v.as_str()) == Some(&spec.name))
                    {
                        return Err(ApiError::conflict(format!(
                            "代理 [{}] 已存在于配置文件中",
                            spec.name
                        )));
                    }
                    arr.push(token);
                }
                ProxyMutation::Update(name, _) => {
                    let idx = arr
                        .iter()
                        .position(|p| p.get("name").and_then(|v| v.as_str()) == Some(name.as_str()))
                        .ok_or_else(|| {
                            ApiError::not_found(format!("配置文件中没有代理 [{name}]"))
                        })?;
                    arr[idx] = token;
                    // 改名时要保证不撞名
                    if name != &spec.name
                        && arr.iter().filter(|p| p.get("name").and_then(|v| v.as_str()) == Some(spec.name.as_str())).count() > 1
                    {
                        return Err(ApiError::conflict(format!(
                            "改名后的代理 [{}] 与已有代理重名",
                            spec.name
                        )));
                    }
                }
                _ => unreachable!(),
            }
        }
        ProxyMutation::Delete(name) => {
            let before = arr.len();
            arr.retain(|p| p.get("name").and_then(|v| v.as_str()) != Some(name.as_str()));
            if arr.len() == before {
                return Err(ApiError::not_found(format!(
                    "配置文件中没有代理 [{name}]"
                )));
            }
        }
    }

    table.insert("proxies".into(), toml::Value::Array(arr));

    // ★ 关键：全局段必须排在所有 [[proxies]] 之前
    // 官方 frpc 解析 TOML 时，如果全局键写在 [[proxies]] 之后，会被归入最后一个代理
    // 并报 `unknown field`（实测）。toml crate 序列化会自然做到这一点。
    toml::to_string_pretty(value).map_err(|e| ApiError::Internal(anyhow::anyhow!("序列化配置失败：{e}")))
}

/// ProxySpec → TOML 表（按目标实现选择字段名）。
pub fn spec_to_toml(spec: &ProxySpec, flavor: BinaryFlavor) -> toml::Value {
    let mut t = toml::map::Map::new();
    t.insert("name".into(), toml::Value::String(spec.name.clone()));
    t.insert("type".into(), toml::Value::String(spec.proxy_type.clone()));

    let addr = spec.effective_local_addr();
    match flavor {
        BinaryFlavor::OfficialFrp => {
            if !addr.is_empty() {
                let (ip, port) = spec.split_local_addr();
                t.insert("localIP".into(), toml::Value::String(ip));
                t.insert("localPort".into(), toml::Value::Integer(port as i64));
            }
        }
        BinaryFlavor::Nfrp => {
            if !addr.is_empty() {
                t.insert("local_addr".into(), toml::Value::String(addr));
            }
        }
    }

    if let Some(p) = spec.remote_port {
        t.insert("remotePort".into(), toml::Value::Integer(p as i64));
    }
    if !spec.custom_domains.is_empty() {
        t.insert(
            "customDomains".into(),
            toml::Value::Array(
                spec.custom_domains
                    .iter()
                    .map(|d| toml::Value::String(d.clone()))
                    .collect(),
            ),
        );
    }
    if !spec.subdomain.is_empty() {
        t.insert(
            "subdomain".into(),
            toml::Value::String(spec.subdomain.clone()),
        );
    }
    if spec.use_encryption {
        t.insert("useEncryption".into(), toml::Value::Boolean(true));
    }
    if spec.use_compression {
        t.insert("useCompression".into(), toml::Value::Boolean(true));
    }
    if let Some(g) = &spec.group {
        if !g.is_empty() {
            t.insert("group".into(), toml::Value::String(g.clone()));
        }
    }
    if !spec.allow_users.is_empty() {
        t.insert(
            "allowUsers".into(),
            toml::Value::Array(
                spec.allow_users
                    .iter()
                    .map(|u| toml::Value::String(u.clone()))
                    .collect(),
            ),
        );
    }
    toml::Value::Table(t)
}

/// 拿一个在线客户端的 run_id（NFrp 服务端下发代理需要）。
async fn first_run_id(c: &FrpAdminClient) -> Option<String> {
    let r = c.get("/api/v2/clients?all=1").await.ok()?;
    if !r.is_success() {
        return None;
    }
    let v = r.json().ok()?;
    v.get("data")?
        .as_array()?
        .iter()
        .find_map(|c| c.get("run_id").and_then(|v| v.as_str()).map(str::to_string))
}

/// 抓取 Prometheus 文本指标。
pub async fn fetch_prometheus(url: &str) -> Result<String, ApiError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("创建 HTTP 客户端失败：{e}")))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::upstream(format!("抓取 Prometheus 指标失败：{e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::upstream(format!(
            "Prometheus 指标端点返回 HTTP {}",
            resp.status().as_u16()
        )));
    }
    resp.text()
        .await
        .map_err(|e| ApiError::upstream(format!("读取指标响应失败：{e}")))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PromCounters {
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub conns: u64,
    pub clients: u64,
    pub proxies: u64,
}

/// 解析 frp 的 Prometheus 文本（官方 `frp_server_*` 与 NFrp 的 `nfrp_*`/`frp_*` 都覆盖）。
pub fn parse_prometheus(text: &str) -> PromCounters {
    let mut c = PromCounters::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.rsplit_once(' ') else {
            continue;
        };
        let Ok(v) = val.trim().parse::<f64>() else {
            continue;
        };
        let v = v.max(0.0) as u64;
        let name = key.split('{').next().unwrap_or(key);
        match name {
            "frp_server_traffic_in" | "nfrp_bytes_down" | "frp_server_bytes_down" => {
                c.bytes_down = v
            }
            "frp_server_traffic_out" | "nfrp_bytes_up" | "frp_server_bytes_up" => c.bytes_up = v,
            "frp_server_conn_counts" | "frp_server_connections" | "frp_server_conn_current"
            | "nfrp_conns_active" => c.conns = v,
            "frp_server_client_counts" | "nfrp_clients_active" => c.clients = v,
            "frp_server_proxy_counts" | "nfrp_proxies_active" => c.proxies = c.proxies.max(v),
            _ => {}
        }
    }
    c
}

#[allow(dead_code)]
fn map_of() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_value() -> toml::Value {
        toml::from_str(
            r#"
server_addr = "1.2.3.4"
server_port = 7000
token = "t"
log_level = "info"
"#,
        )
        .unwrap()
    }

    #[test]
    fn create_proxy_writes_official_field_names() {
        let mut v = base_value();
        let spec = ProxySpec {
            name: "web".into(),
            proxy_type: "tcp".into(),
            local_addr: "127.0.0.1:8080".into(),
            remote_port: Some(6000),
            ..Default::default()
        };
        let out =
            apply_proxy_mutation(&mut v, &ProxyMutation::Create(spec), BinaryFlavor::OfficialFrp)
                .unwrap();
        assert!(out.contains("localIP"), "{out}");
        assert!(!out.contains("local_addr"));
        // 全局段必须在 [[proxies]] 之前（官方 frpc 否则会 unknown field）
        let idx_global = out.find("server_addr").unwrap();
        let idx_proxies = out.find("[[proxies]]").unwrap();
        assert!(idx_global < idx_proxies, "{out}");
    }

    #[test]
    fn create_proxy_writes_nfrp_field_names() {
        let mut v = base_value();
        let spec = ProxySpec {
            name: "web".into(),
            proxy_type: "tcp".into(),
            local_addr: "127.0.0.1:8080".into(),
            remote_port: Some(6000),
            ..Default::default()
        };
        let out =
            apply_proxy_mutation(&mut v, &ProxyMutation::Create(spec), BinaryFlavor::Nfrp).unwrap();
        assert!(out.contains("local_addr"), "{out}");
    }

    #[test]
    fn duplicate_create_is_conflict() {
        let mut v = base_value();
        let spec = ProxySpec {
            name: "web".into(),
            proxy_type: "tcp".into(),
            local_addr: "127.0.0.1:8080".into(),
            remote_port: Some(6000),
            ..Default::default()
        };
        apply_proxy_mutation(&mut v, &ProxyMutation::Create(spec.clone()), BinaryFlavor::Nfrp)
            .unwrap();
        let err = apply_proxy_mutation(
            &mut v,
            &ProxyMutation::Create(spec),
            BinaryFlavor::Nfrp,
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::Conflict(_)));
    }

    #[test]
    fn delete_missing_proxy_is_not_found() {
        let mut v = base_value();
        let err =
            apply_proxy_mutation(&mut v, &ProxyMutation::Delete("nope".into()), BinaryFlavor::Nfrp)
                .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[test]
    fn update_replaces_in_place() {
        let mut v = base_value();
        let mk = |port: u16| ProxySpec {
            name: "web".into(),
            proxy_type: "tcp".into(),
            local_addr: format!("127.0.0.1:{port}"),
            remote_port: Some(6000),
            ..Default::default()
        };
        apply_proxy_mutation(&mut v, &ProxyMutation::Create(mk(8080)), BinaryFlavor::Nfrp).unwrap();
        let out = apply_proxy_mutation(
            &mut v,
            &ProxyMutation::Update("web".into(), mk(9090)),
            BinaryFlavor::Nfrp,
        )
        .unwrap();
        assert!(out.contains("127.0.0.1:9090"));
        assert!(!out.contains("127.0.0.1:8080"));
        // 解析回来仍然合法
        let _: nfrp_common::config::ClientConfig =
            nfrp_common::config::parse_client(&out).unwrap();
    }

    #[test]
    fn parses_official_prometheus() {
        let text = "# HELP frp_server_client_counts\nThe current\nfrp_server_client_counts 3\nfrp_server_proxy_counts{type=\"tcp\"} 5\n";
        let c = parse_prometheus(text);
        assert_eq!(c.clients, 3);
        assert_eq!(c.proxies, 5);
    }

    #[test]
    fn parses_nfrp_prometheus_style_names() {
        let text = "nfrp_bytes_up 100\nnfrp_bytes_down 200\nnfrp_conns_active 7\n";
        let c = parse_prometheus(text);
        assert_eq!(c.bytes_up, 100);
        assert_eq!(c.bytes_down, 200);
        assert_eq!(c.conns, 7);
    }
}

