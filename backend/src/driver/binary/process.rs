//! 本机进程管理：启动、优雅停止、状态查询、日志跟随。
//!
//! 跨平台要点（实测见 `docs/04-verified-behavior.md`）：
//! - Windows 上 `taskkill /PID`（= 发送 WM_CLOSE）对 frps/frpc 表现为**优雅退出**（实测 103 ms 内退出），
//!   但**不会**触发 reload → 因此"保存配置"在 Windows 上按"重启生效"处理；
//! - Unix 上先 `SIGTERM`，等待宽限期，仍未退出则 `SIGKILL`；
//! - 面板自身重启后要能"接管"已在运行的实例（PID 文件 + 启动时间校验），
//!   否则用户会觉得"面板一重启实例就丢了"。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::error::ApiError;

/// 面板自己拉起的子进程句柄（重启面板后丢失，靠 PID 文件恢复）。
pub struct ManagedChild {
    pub pid: u32,
    child: Mutex<Option<Child>>,
}

impl ManagedChild {
    pub fn new(pid: u32, child: Child) -> Self {
        Self {
            pid,
            child: Mutex::new(Some(child)),
        }
    }

    /// 尝试非阻塞回收（进程已退出时返回退出码）。
    pub async fn try_wait(&self) -> Option<Option<i32>> {
        let mut guard = self.child.lock().await;
        if let Some(c) = guard.as_mut() {
            match c.try_wait() {
                Ok(Some(status)) => Some(status.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            }
        } else {
            Some(None)
        }
    }
}

/// 进程状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Running,
    Stopped,
}

/// PID 文件内容：用于面板重启后接管。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    /// 进程启动时间（Unix 秒），用于避免 PID 复用导致的误判。
    pub started_at: i64,
    pub binary: String,
    pub config: String,
}

fn pid_file_for(work_dir: &Path, name: &str) -> PathBuf {
    work_dir.join(format!("{name}.pid.json"))
}

pub fn read_pid_record(work_dir: &Path, name: &str) -> Option<PidRecord> {
    let p = pid_file_for(work_dir, name);
    let text = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_pid_record(work_dir: &Path, name: &str, rec: &PidRecord) -> Result<(), ApiError> {
    std::fs::create_dir_all(work_dir)?;
    let p = pid_file_for(work_dir, name);
    let text = serde_json::to_string_pretty(rec)?;
    crate::configfile::atomic_write(&p, &text)?;
    Ok(())
}

#[allow(dead_code)]
fn remove_pid_record(work_dir: &Path, name: &str) {
    let _ = std::fs::remove_file(pid_file_for(work_dir, name));
}

/// 启动参数。
pub struct SpawnOptions {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
    pub work_dir: PathBuf,
    /// 实例名（PID 文件名）
    pub name: String,
    pub env: Vec<(String, String)>,
}

/// 启动进程（stdout/stderr 追加写入日志文件）。
pub async fn spawn(opts: &SpawnOptions) -> Result<Arc<ManagedChild>, ApiError> {
    if !opts.binary.exists() {
        return Err(ApiError::not_found(format!(
            "二进制不存在：{}（请在实例设置里修正路径，或先执行部署）",
            opts.binary.display()
        )));
    }
    std::fs::create_dir_all(&opts.work_dir)?;
    if let Some(parent) = opts.log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&opts.log_path)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("打开日志文件失败：{e}")))?;
    let log_err = log
        .try_clone()
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("复制日志句柄失败：{e}")))?;

    let mut cmd = Command::new(&opts.binary);
    cmd.args(&opts.args)
        .current_dir(&opts.work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }

    #[cfg(windows)]
    {
        // 隐藏控制台窗口，避免面板启动时弹黑框
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd
        .spawn()
        .map_err(|e| ApiError::upstream(format!("启动进程失败（{}）：{e}", opts.binary.display())))?;
    let pid = child
        .id()
        .ok_or_else(|| ApiError::upstream("进程启动后拿不到 PID".to_string()))?;

    let rec = PidRecord {
        pid,
        started_at: chrono::Utc::now().timestamp(),
        binary: opts.binary.display().to_string(),
        config: opts.config_path.display().to_string(),
    };
    write_pid_record(&opts.work_dir, &opts.name, &rec)?;

    Ok(Arc::new(ManagedChild::new(pid, child)))
}

/// 判断某个 PID 是否仍在运行（含启动时间校验）。
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        // 用 tasklist 太慢；直接尝试打开进程句柄
        use std::process::Command as StdCommand;
        let out = StdCommand::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        // ★ tasklist 的输出会随系统语言变化，不能匹配 "No tasks" 这类英文提示。
        //   逐行按 CSV 解析，只比较第二列（PID），避免把内存/会话数字误判成 PID。
        match out {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                let target = pid.to_string();
                text.lines().any(|line| {
                    let mut fields = line.split(',');
                    let _image = fields.next();
                    let pid_field = fields.next().unwrap_or("").trim().trim_matches('"');
                    pid_field == target.as_str()
                })
            }
            Err(_) => false,
        }
    }
    #[cfg(unix)]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
            && std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|s| !s.contains(") Z"))
                .unwrap_or(true)
    }
}

/// 优雅停止：先请求终止，等待宽限期，再强制。
pub async fn terminate(pid: u32, grace_ms: u64, force: bool) -> Result<(), ApiError> {
    if !pid_alive(pid) {
        return Ok(());
    }

    // ★ Windows 实测（关键）：面板用 CREATE_NO_WINDOW 拉起 frps/frpc 时，
    //   子进程**没有可见窗口**，`taskkill /PID`（= 发送 WM_CLOSE）会报
    //   "This process can only be terminated forcefully (with /F option)"。
    //   ⇒ 无窗口进程直接走强制终止；只有确实有窗口时才先尝试优雅关闭。
    let graceful_ok = if force {
        false
    } else {
        request_graceful(pid)
    };

    if graceful_ok {
        let deadline = std::time::Instant::now() + Duration::from_millis(grace_ms);
        while std::time::Instant::now() < deadline {
            if !pid_alive(pid) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }

    if pid_alive(pid) {
        force_kill(pid)?;
        // 给系统一点时间回收
        for _ in 0..30 {
            if !pid_alive(pid) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if pid_alive(pid) {
            return Err(ApiError::upstream(format!(
                "进程 {pid} 在强制终止后仍未退出，请手动检查（可能需要管理员权限）"
            )));
        }
    }
    Ok(())
}

/// 尝试优雅终止。返回 `true` 表示"已发出优雅请求，值得等宽限期"。
///
/// ★ 不解析 taskkill 的输出文本：中文 Windows 的提示是 GBK 编码，且措辞随系统语言变化。
///   在此之前，错误提示会以乱码透出，且 "强制终止" 的判定也会失效。现在任何失败
///   （包括无窗口进程只能强杀的提示）都只记录日志并回退到强制终止。
fn request_graceful(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::process::Command as StdCommand;
        let out = StdCommand::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .output();
        match out {
            Ok(o) if o.status.success() => true,
            Ok(o) => {
                tracing::debug!(
                    pid,
                    stdout = %String::from_utf8_lossy(&o.stdout),
                    stderr = %String::from_utf8_lossy(&o.stderr),
                    "taskkill 未能优雅终止，回退强制终止"
                );
                false
            }
            Err(e) => {
                tracing::debug!(pid, %e, "无法调用 taskkill，回退强制终止");
                false
            }
        }
    }
    #[cfg(unix)]
    {
        unsafe {
            if libc_kill(pid as i32, 15) != 0 {
                // ESRCH：进程不存在，视为已停止；其余情况一律回退强杀
                false
            } else {
                true
            }
        }
    }
}

fn force_kill(pid: u32) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        use std::process::Command as StdCommand;
        let _ = StdCommand::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
        Ok(())
    }
    #[cfg(unix)]
    {
        unsafe {
            libc_kill(pid as i32, 9);
        }
        Ok(())
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// 读日志文件：支持 tail、按偏移分页、关键字与级别过滤。
pub async fn read_log(
    path: &Path,
    offset: Option<usize>,
    limit: usize,
    q: Option<&str>,
    level: Option<&str>,
) -> Result<(Vec<String>, Option<usize>, usize, usize), ApiError> {
    if !path.exists() {
        return Ok((vec![], None, 0, 0));
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("打开日志失败：{e}")))?;
    // 日志过大会拖死面板：只读最后 8 MB
    const MAX_READ: u64 = 8 * 1024 * 1024;
    let len = file.metadata().await?.len();
    let start = len.saturating_sub(MAX_READ);
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // 第一行可能被截断
    }
    let total = lines.len();

    // 过滤
    let needle = q.map(|s| s.to_lowercase()).filter(|s| !s.is_empty());
    let lvl = level.filter(|s| !s.is_empty()).map(|s| s.to_uppercase());
    let filtered: Vec<&str> = lines
        .into_iter()
        .filter(|l| {
            if let Some(n) = &needle {
                if !l.to_lowercase().contains(n.as_str()) {
                    return false;
                }
            }
            if let Some(want) = &lvl {
                let parsed = crate::domain::parse_log_line(0, l);
                if parsed.level.as_deref() != Some(want.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect();

    let total_filtered = filtered.len();
    // offset 语义：从过滤后的第 offset 条开始；None = 取最后 limit 条
    let (slice, next) = match offset {
        Some(off) => {
            let end = (off + limit).min(total_filtered);
            (
                filtered.get(off..end).unwrap_or(&[]).to_vec(),
                if end < total_filtered { Some(end) } else { None },
            )
        }
        None => {
            let start = total_filtered.saturating_sub(limit);
            (filtered[start..].to_vec(), None)
        }
    };

    Ok((
        slice.into_iter().map(str::to_string).collect(),
        next,
        total_filtered,
        total,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_tail_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.log");
        let content = (0..100)
            .map(|i| {
                if i % 10 == 0 {
                    format!("2026-11-03T00:00:{:02}Z  ERROR something bad {i}\n", i % 60)
                } else {
                    format!("2026-11-03T00:00:{:02}Z  INFO normal line {i}\n", i % 60)
                }
            })
            .collect::<String>();
        std::fs::write(&f, content).unwrap();

        let (lines, next, total, _) = read_log(&f, None, 5, None, None).await.unwrap();
        assert_eq!(lines.len(), 5);
        assert!(lines.last().unwrap().contains("line 99"));
        assert!(next.is_none());
        assert_eq!(total, 100);

        let (errs, _, err_total, _) = read_log(&f, None, 100, None, Some("ERROR")).await.unwrap();
        assert_eq!(err_total, 10);
        assert!(errs.iter().all(|l| l.contains("ERROR")));

        let (hits, _, hit_total, _) = read_log(&f, None, 100, Some("line 42"), None)
            .await
            .unwrap();
        assert_eq!(hit_total, 1);
        assert!(hits[0].contains("line 42"));

        let (page1, next1, _, _) = read_log(&f, Some(0), 10, None, None).await.unwrap();
        assert_eq!(page1.len(), 10);
        assert_eq!(next1, Some(10));
    }

    #[tokio::test]
    async fn missing_log_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let (lines, _, total, _) = read_log(&dir.path().join("none.log"), None, 10, None, None)
            .await
            .unwrap();
        assert!(lines.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn pid_alive_for_self() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
    }
}
