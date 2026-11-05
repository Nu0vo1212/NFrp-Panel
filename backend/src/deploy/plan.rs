//! 部署计划：把「部署」拆成可观测的步骤。
//!
//! 计划与驱动能力无关，只描述**要做什么**；执行由 `deploy::runner` 逐步推进，
//! 每一步的成败、消息与修复建议都经 `DeployHub` 广播到前端。

use serde::{Deserialize, Serialize};

use crate::deploy::hub::DeployStep;
use crate::domain::{BinaryFlavor, InstanceRole, ServiceMode};

/// 计划中的一步。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub key: String,
    pub title: String,
}

/// 依据实例配置生成计划（顺序即执行顺序）。
pub fn build_plan(
    role: InstanceRole,
    service: ServiceMode,
    _flavor: BinaryFlavor,
    with_binary: bool,
) -> Vec<PlanStep> {
    let mut steps = vec![PlanStep {
        key: "probe".into(),
        title: "探测远程环境".into(),
    }];

    if with_binary {
        steps.push(PlanStep {
            key: "upload_binary".into(),
            title: "上传二进制".into(),
        });
        steps.push(PlanStep {
            key: "verify_binary".into(),
            title: "校验二进制（架构 / 哈希 / 可执行位）".into(),
        });
        steps.push(PlanStep {
            key: "install_binary".into(),
            title: "安装到目标路径".into(),
        });
    }

    steps.push(PlanStep {
        key: "write_config".into(),
        title: "下发配置（备份 + 原子替换）".into(),
    });

    match service {
        ServiceMode::Systemd => {
            steps.push(PlanStep {
                key: "write_unit".into(),
                title: "写入 systemd unit".into(),
            });
            steps.push(PlanStep {
                key: "daemon_reload".into(),
                title: "systemctl daemon-reload".into(),
            });
            steps.push(PlanStep {
                key: "enable_service".into(),
                title: "启用开机自启".into(),
            });
        }
        _ => {
            steps.push(PlanStep {
                key: "write_pid_dir".into(),
                title: "准备 nohup 运行目录".into(),
            });
        }
    }

    steps.push(PlanStep {
        key: "start_service".into(),
        title: "启动服务".into(),
    });
    steps.push(PlanStep {
        key: "wait_admin_api".into(),
        title: "等待 Admin API 就绪".into(),
    });
    steps.push(PlanStep {
        key: "verify_status".into(),
        title: "校验运行状态".into(),
    });

    let _ = role;
    steps
}

/// 计划 → 初始步骤状态。
pub fn to_steps(plan: &[PlanStep]) -> Vec<DeployStep> {
    plan.iter()
        .map(|p| DeployStep::new(&p.key, &p.title))
        .collect()
}

/// 解析计划（前端可展示"将要执行哪些步骤"）。
pub fn parse_plan(steps: &[PlanStep]) -> Vec<(String, String)> {
    steps
        .iter()
        .map(|s| (s.key.clone(), s.title.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_plan_has_all_phases() {
        let plan = build_plan(
            InstanceRole::Server,
            ServiceMode::Systemd,
            BinaryFlavor::Nfrp,
            true,
        );
        let keys: Vec<&str> = plan.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys.first(), Some(&"probe"));
        assert_eq!(keys.last(), Some(&"verify_status"));
        assert!(keys.contains(&"upload_binary"));
        assert!(keys.contains(&"write_unit"));
        assert!(keys.contains(&"daemon_reload"));
    }

    #[test]
    fn nohup_plan_skips_systemd_steps() {
        let plan = build_plan(
            InstanceRole::Client,
            ServiceMode::Nohup,
            BinaryFlavor::OfficialFrp,
            true,
        );
        let keys: Vec<&str> = plan.iter().map(|p| p.key.as_str()).collect();
        assert!(!keys.contains(&"write_unit"));
        assert!(keys.contains(&"write_pid_dir"));
    }

    #[test]
    fn plan_without_binary_skips_install_steps() {
        let plan = build_plan(
            InstanceRole::Server,
            ServiceMode::Systemd,
            BinaryFlavor::Nfrp,
            false,
        );
        let keys: Vec<&str> = plan.iter().map(|p| p.key.as_str()).collect();
        assert!(!keys.contains(&"upload_binary"));
        assert!(keys.contains(&"write_config"));
    }

    #[test]
    fn steps_are_all_pending_initially() {
        let plan = build_plan(
            InstanceRole::Server,
            ServiceMode::Systemd,
            BinaryFlavor::Nfrp,
            true,
        );
        let steps = to_steps(&plan);
        assert!(steps
            .iter()
            .all(|s| s.state == crate::deploy::hub::DeployStepState::Pending));
    }
}
