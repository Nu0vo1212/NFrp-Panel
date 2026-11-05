//! 部署任务运行态与事件广播（SSE 源）。
//!
//! 生命周期：`pending → running → succeeded | failed | rolled_back`。
//! 每一步有 `pending/running/done/failed/skipped` 状态；
//! 失败时带 `remediation`，前端据此展示修复建议。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStepState {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

impl DeployStepState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployState {
    Pending,
    Running,
    Succeeded,
    Failed,
    RolledBack,
}

impl DeployState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "rolled_back" => Self::RolledBack,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStep {
    pub key: String,
    pub title: String,
    pub state: DeployStepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 失败时的修复建议（前端高亮展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl DeployStep {
    pub fn new(key: &str, title: &str) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            state: DeployStepState::Pending,
            started_at: None,
            finished_at: None,
            message: None,
            remediation: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployTask {
    pub id: String,
    pub instance_id: String,
    pub kind: String,
    pub state: DeployState,
    pub steps: Vec<DeployStep>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_of: Option<String>,
    /// 持久化前的日志（最多保留 5000 行）
    pub logs: Vec<String>,
}

/// SSE 事件。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DeployEvent {
    Step {
        key: String,
        state: DeployStepState,
        message: Option<String>,
        remediation: Option<String>,
    },
    Log {
        ts: String,
        stream: String,
        line: String,
    },
    Done {
        state: DeployState,
        error: Option<String>,
    },
}

impl DeployEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Step { .. } => "step",
            Self::Log { .. } => "log",
            Self::Done { .. } => "done",
        }
    }
}

/// 每个任务的运行态。
struct RunningTask {
    task: DeployTask,
    tx: broadcast::Sender<DeployEvent>,
}

/// 部署任务中心。
pub struct DeployHub {
    tasks: Mutex<HashMap<String, RunningTask>>,
}

impl DeployHub {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// 注册一个新任务，返回订阅端。
    pub async fn create(
        &self,
        id: &str,
        instance_id: uuid::Uuid,
        kind: &str,
        steps: Vec<DeployStep>,
        rollback_of: Option<&str>,
    ) -> (broadcast::Sender<DeployEvent>, broadcast::Receiver<DeployEvent>) {
        let (tx, rx) = broadcast::channel(1024);
        let task = DeployTask {
            id: id.to_string(),
            instance_id: instance_id.to_string(),
            kind: kind.to_string(),
            state: DeployState::Pending,
            steps,
            started_at: now_str(),
            finished_at: None,
            error: None,
            rollback_of: rollback_of.map(str::to_string),
            logs: Vec::new(),
        };
        let mut g = self.tasks.lock().await;
        g.insert(
            id.to_string(),
            RunningTask {
                task,
                tx: tx.clone(),
            },
        );
        (tx, rx)
    }

    /// 订阅已存在的任务（断线重连/刷新后继续看）。
    pub async fn subscribe(&self, id: &str) -> Option<broadcast::Receiver<DeployEvent>> {
        let g = self.tasks.lock().await;
        g.get(id).map(|t| t.tx.subscribe())
    }

    pub async fn get(&self, id: &str) -> Option<DeployTask> {
        let g = self.tasks.lock().await;
        g.get(id).map(|t| t.task.clone())
    }

    pub async fn latest_for(&self, instance_id: uuid::Uuid) -> Option<DeployTask> {
        let g = self.tasks.lock().await;
        g.values()
            .filter(|t| t.task.instance_id == instance_id.to_string())
            .max_by_key(|t| t.task.started_at.clone())
            .map(|t| t.task.clone())
    }

    pub async fn set_state(&self, id: &str, state: DeployState) {
        let mut g = self.tasks.lock().await;
        if let Some(t) = g.get_mut(id) {
            t.task.state = state;
            if matches!(
                state,
                DeployState::Succeeded | DeployState::Failed | DeployState::RolledBack
            ) {
                t.task.finished_at = Some(now_str());
            }
            let _ = t.tx.send(DeployEvent::Done {
                state,
                error: t.task.error.clone(),
            });
        }
    }

    pub async fn start_step(&self, id: &str, key: &str) {
        let mut g = self.tasks.lock().await;
        if let Some(t) = g.get_mut(id) {
            if let Some(s) = t.task.steps.iter_mut().find(|s| s.key == key) {
                s.state = DeployStepState::Running;
                s.started_at = Some(now_str());
            }
            let _ = t.tx.send(DeployEvent::Step {
                key: key.into(),
                state: DeployStepState::Running,
                message: None,
                remediation: None,
            });
        }
    }

    pub async fn finish_step(
        &self,
        id: &str,
        key: &str,
        state: DeployStepState,
        message: Option<&str>,
    ) {
        let mut g = self.tasks.lock().await;
        if let Some(t) = g.get_mut(id) {
            if let Some(s) = t.task.steps.iter_mut().find(|s| s.key == key) {
                s.state = state;
                s.finished_at = Some(now_str());
                s.message = message.map(str::to_string);
                if state == DeployStepState::Failed {
                    s.remediation = remediation_for(&s.key).map(str::to_string);
                }
            }
            let remediation = t
                .task
                .steps
                .iter()
                .find(|s| s.key == key)
                .and_then(|s| s.remediation.clone());
            let _ = t.tx.send(DeployEvent::Step {
                key: key.into(),
                state,
                message: message.map(str::to_string),
                remediation,
            });
        }
    }

    /// 追加一行日志（同时广播）。
    pub async fn log(&self, id: &str, stream: &str, line: &str) {
        let safe = crate::secret::redact_line(line);
        let mut g = self.tasks.lock().await;
        if let Some(t) = g.get_mut(id) {
            if t.task.logs.len() < 5000 {
                t.task.logs.push(format!("[{stream}] {safe}"));
            }
            let _ = t.tx.send(DeployEvent::Log {
                ts: now_str(),
                stream: stream.to_string(),
                line: safe,
            });
        }
    }

    pub async fn set_error(&self, id: &str, err: &str) {
        let mut g = self.tasks.lock().await;
        if let Some(t) = g.get_mut(id) {
            t.task.error = Some(crate::secret::redact_line(err));
        }
    }

    pub async fn remove(&self, id: &str) {
        let mut g = self.tasks.lock().await;
        g.remove(id);
    }
}

impl Default for DeployHub {
    fn default() -> Self {
        Self::new()
    }
}

fn now_str() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 每个步骤的修复建议（与 `docs/03-ssh-deploy.md` 对应）。
pub fn remediation_for(step_key: &str) -> Option<&'static str> {
    Some(match step_key {
        "probe" => "确认 SSH 用户可执行 uname/hostname；确认目标为 Linux 系统",
        "host_key" => "在「主机指纹」页核对指纹后点击信任；若指纹变化请先确认为同一台主机",
        "upload_binary" => "检查上传目录的写权限与磁盘空间；确认本机有 sftp 命令",
        "verify_binary" => "二进制与远程架构不匹配，请选择对应平台（amd64/arm64）的包",
        "install_binary" => "确认 sudo 权限充足（建议配置 sudo -n 免密）",
        "write_config" => "检查 /etc 目录写权限；确认配置内容通过校验",
        "write_unit" => "确认有 systemd 且可用 sudo 写 /etc/systemd/system",
        "daemon_reload" => "执行 systemctl daemon-reload 需要 sudo；检查 unit 语法",
        "enable_service" => "确认服务名不冲突，必要时先 disable 旧服务",
        "start_service" => "查看远程日志（systemctl status）确认端口占用或配置错误",
        "wait_admin_api" => "确认 frps 配置了 dashboard_port/webServer.port 且与面板填写的地址一致；如仅监听 127.0.0.1 需改为 0.0.0.0 或使用隧道",
        "verify_status" => "通过 SSH 查看远程日志，或降低校验要求后重试",
        _ => return None,
    })
}

/// 把时间戳转成可读时间（测试辅助）。
pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn step_lifecycle_broadcasts_events() {
        let hub = DeployHub::new();
        let id = uuid::Uuid::new_v4();
        let (_tx, mut rx) = hub
            .create(
                "t1",
                id,
                "deploy",
                vec![DeployStep::new("probe", "探测环境")],
                None,
            )
            .await;

        hub.start_step("t1", "probe").await;
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.event_name(), "step");

        hub.finish_step("t1", "probe", DeployStepState::Done, Some("ok"))
            .await;
        let ev = rx.recv().await.unwrap();
        match ev {
            DeployEvent::Step { state, .. } => assert_eq!(state, DeployStepState::Done),
            _ => panic!("期望 step 事件"),
        }

        let task = hub.get("t1").await.unwrap();
        assert_eq!(task.steps[0].state, DeployStepState::Done);
    }

    #[tokio::test]
    async fn failed_step_gets_remediation() {
        let hub = DeployHub::new();
        let (_tx, _rx) = hub
            .create(
                "t2",
                uuid::Uuid::new_v4(),
                "deploy",
                vec![DeployStep::new("install_binary", "安装二进制")],
                None,
            )
            .await;
        hub.finish_step(
            "t2",
            "install_binary",
            DeployStepState::Failed,
            Some("permission denied"),
        )
        .await;
        let task = hub.get("t2").await.unwrap();
        assert!(task.steps[0].remediation.is_some());
    }

    #[tokio::test]
    async fn logs_are_redacted() {
        let hub = DeployHub::new();
        let (_tx, mut rx) = hub
            .create("t3", uuid::Uuid::new_v4(), "deploy", vec![], None)
            .await;
        hub.log("t3", "stdout", r#"token = "supersecret""#).await;
        let ev = rx.recv().await.unwrap();
        match ev {
            DeployEvent::Log { line, .. } => assert!(!line.contains("supersecret")),
            _ => panic!("期望 log 事件"),
        }
    }

    #[test]
    fn states_roundtrip() {
        for s in [
            DeployState::Pending,
            DeployState::Running,
            DeployState::Succeeded,
            DeployState::Failed,
            DeployState::RolledBack,
        ] {
            assert_eq!(DeployState::parse(s.as_str()), s);
        }
        for s in [
            DeployStepState::Pending,
            DeployStepState::Running,
            DeployStepState::Done,
            DeployStepState::Failed,
            DeployStepState::Skipped,
        ] {
            assert_eq!(DeployStepState::parse(s.as_str()), s);
        }
    }
}
