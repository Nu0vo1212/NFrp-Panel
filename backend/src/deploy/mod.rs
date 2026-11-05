//! 部署编排：预检、分步执行、流式日志、回滚。
//!
//! 任务运行态放在 `DeployHub`：每个任务一个广播通道，SSE 端订阅；
//! 同时把步骤与日志持久化到 `deploy_tasks` 表，断线后可续看。

pub mod catalog;
pub mod hub;
pub mod plan;
pub mod preflight;
pub mod runner;

pub use hub::{DeployEvent, DeployHub, DeployState, DeployStep, DeployStepState, DeployTask};
pub use plan::{parse_plan, PlanStep};
