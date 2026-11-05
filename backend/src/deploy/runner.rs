//! 部署执行器：按计划逐步推进，每步都广播状态与日志；失败自动回滚。
//!
//! 幂等性（需求第七节 2）：
//! - 重复部署**先备份**旧二进制与旧配置（带时间戳），再覆盖；
//! - 配置写入走「临时文件 + `mv` 原子替换」；
//! - 任一步失败 → 回滚到备份（若已有备份）。

use std::sync::Arc;

use crate::db::models::InstanceRow;
use crate::domain::DeploymentStatus;
use crate::db::repo::InstanceUpdate;
use crate::deploy::hub::{DeployHub, DeployState, DeployStepState};
use crate::deploy::plan;
use crate::domain::{BinaryFlavor, ServiceMode};
use crate::error::ApiError;
use crate::ssh::exec::{self, AllowedCommand, SudoPrefix};
use crate::ssh::session::SshSession;
use crate::ssh::sftp;
use crate::state::AppState;

/// 部署输入（由 API 层组装）。
pub struct DeployInput {
    pub instance: InstanceRow,
    /// 二进制内容（None = 只下发配置，不换二进制）
    pub binary: Option<Vec<u8>>,
    pub binary_name: String,
    pub config: String,
    pub unit_name: String,
    pub admin_url: String,
    pub admin_user: String,
    pub admin_password: String,
    /// SFTP 传输上下文（凭据已解密）
    pub transfer: sftp::TransferContext,
}

/// 执行一次部署，返回最终状态。
pub async fn run(
    st: Arc<AppState>,
    task_id: String,
    input: DeployInput,
) -> (DeployState, Option<String>) {
    let hub = st.deploy.clone();
    match run_inner(&st, &hub, &task_id, &input).await {
        Ok(()) => {
            hub.set_state(&task_id, DeployState::Succeeded).await;
            (DeployState::Succeeded, None)
        }
        Err(e) => {
            let msg = e.to_string();
            hub.set_error(&task_id, &msg).await;
            // 若内部已经回滚过，不要把它覆盖成 failed
            let already_rolled_back = hub
                .get(&task_id)
                .await
                .map(|t| t.state == DeployState::RolledBack)
                .unwrap_or(false);
            if !already_rolled_back {
                hub.set_state(&task_id, DeployState::Failed).await;
                let _ = crate::db::repo::update_instance(
                    &st.db,
                    input.instance.id,
                    InstanceUpdate {
                        deployment_status: Some(DeploymentStatus::Failed),
                        last_error: Some(Some(msg.clone())),
                        ..Default::default()
                    },
                )
                .await;
            }
            (hub.get(&task_id).await.map(|t| t.state).unwrap_or(DeployState::Failed), Some(msg))
        }
    }
}

/// 失败某步并返回错误。
async fn fail_step(hub: &DeployHub, task: &str, key: &str, msg: String) -> ApiError {
    hub.finish_step(task, key, DeployStepState::Failed, Some(&msg))
        .await;
    ApiError::upstream(msg)
}

async fn run_inner(
    st: &Arc<AppState>,
    hub: &DeployHub,
    task_id: &str,
    input: &DeployInput,
) -> Result<(), ApiError> {
    let row = &input.instance;
    hub.set_state(task_id, DeployState::Running).await;

    // ---- 建立 SSH 会话（复用驱动的连接参数与主机密钥校验） ----
    let driver = st.driver(row.id).await?;
    let Some(ssh) = crate::driver::ssh::as_ssh_driver(&driver) else {
        return Err(ApiError::bad_request("该实例不是 SSH 远程实例，无法执行远程部署"));
    };
    let session = ssh.session().await?;
    let sudo = sudo_prefix(st, row)?;

    let paths = row.paths.clone();
    let install_root = paths.remote_root();
    let cfg_dir = dir_of(&paths.config, "/etc/nfrp");
    let log_dir = dir_of(&paths.log, "/var/log/nfrp");
    let _ = plan::build_plan(row.role, row.service_mode, row.flavor, input.binary.is_some());

    // ---- 1. 探测 ----
    hub.start_step(task_id, "probe").await;
    let env = match crate::ssh::probe::probe(
        &session,
        &crate::ssh::probe::ProbeOptions {
            config_dir: &cfg_dir,
            candidate_ports: &[],
        },
    )
    .await
    {
        Ok(e) => {
            hub.log(
                task_id,
                "stdout",
                &format!("目标：{} {} {}", e.os, e.kernel, e.arch),
            )
            .await;
            hub.log(
                task_id,
                "stdout",
                &format!(
                    "systemd={} sudo={} disk={}MB 用户={}",
                    e.systemd, e.sudo, e.disk_free_mb, e.current_user
                ),
            )
            .await;
            hub.finish_step(task_id, "probe", DeployStepState::Done, Some("环境探测完成"))
                .await;
            e
        }
        Err(e) => {
            let msg = e.to_string();
            return Err(fail_step(hub, task_id, "probe", msg).await);
        }
    };

    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let mut old_binary_backup: Option<String> = None;
    let mut old_config_backup: Option<String> = None;

    // ---- 2. 上传二进制 ----
    if let Some(bytes) = &input.binary {
        hub.start_step(task_id, "upload_binary").await;
        let remote_tmp = format!("/tmp/frppanel-{}-{stamp}", input.binary_name);
        if let Err(e) = sftp::upload_bytes(&input.transfer, &remote_tmp, bytes).await {
            let msg = e.to_string();
            return Err(fail_step(hub, task_id, "upload_binary", msg).await);
        }
        hub.log(
            task_id,
            "stdout",
            &format!("已上传 {} 字节 → {remote_tmp}", bytes.len()),
        )
        .await;
        hub.finish_step(task_id, "upload_binary", DeployStepState::Done, Some("上传完成"))
            .await;

        // ---- 3. 校验 ----
        hub.start_step(task_id, "verify_binary").await;
        let remote_sha = exec::run(
            &session,
            AllowedCommand::Sha256 {
                path: remote_tmp.clone(),
            },
            &sudo,
            None,
        )
        .await
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default();
        let local_sha = crate::crypto::sha256_hex(bytes);
        if remote_sha != local_sha {
            let _ = exec::run(
                &session,
                AllowedCommand::RemoveFile {
                    path: remote_tmp.clone(),
                },
                &sudo,
                None,
            )
            .await;
            let msg = format!("上传后哈希不一致（远端 {remote_sha}，本地 {local_sha}）");
            return Err(fail_step(hub, task_id, "verify_binary", msg).await);
        }
        hub.log(task_id, "stdout", &format!("sha256 一致：{remote_sha}"))
            .await;

        if let Some(arch) = crate::deploy::catalog::elf_machine(bytes) {
            let expected = crate::ssh::probe::normalize_arch(&env.arch);
            if arch != expected && arch != "unknown" {
                let msg = format!("二进制架构 {arch} 与目标机 {expected} 不匹配");
                return Err(fail_step(hub, task_id, "verify_binary", msg).await);
            }
        }
        hub.finish_step(
            task_id,
            "verify_binary",
            DeployStepState::Done,
            Some("哈希与架构校验通过"),
        )
        .await;

        // ---- 4. 安装 ----
        hub.start_step(task_id, "install_binary").await;
        let target = format!("{install_root}/{}", input.binary_name);
        if let Err(e) = exec::run_checked(
            &session,
            AllowedCommand::MkdirP {
                path: install_root.clone(),
            },
            &sudo,
            None,
            "创建安装目录",
        )
        .await
        {
            let msg = e.to_string();
            return Err(fail_step(hub, task_id, "install_binary", msg).await);
        }

        let exists = exec::run(
            &session,
            AllowedCommand::FileExists {
                path: target.clone(),
            },
            &sudo,
            None,
        )
        .await
        .map(|o| o.stdout.trim() == "yes")
        .unwrap_or(false);
        if exists {
            let bak = format!("{target}.bak.{stamp}");
            exec::run_checked(
                &session,
                AllowedCommand::Copy {
                    src: target.clone(),
                    dst: bak.clone(),
                },
                &sudo,
                None,
                "备份旧二进制",
            )
            .await?;
            hub.log(task_id, "stdout", &format!("已备份旧二进制 → {bak}"))
                .await;
            old_binary_backup = Some(bak);
        }

        for step in [
            exec::run_checked(
                &session,
                AllowedCommand::Move {
                    src: remote_tmp.clone(),
                    dst: target.clone(),
                },
                &sudo,
                None,
                "安装二进制",
            )
            .await,
            exec::run_checked(
                &session,
                AllowedCommand::Chmod {
                    path: target.clone(),
                    mode: "0755".into(),
                },
                &sudo,
                None,
                "设置可执行位",
            )
            .await,
        ] {
            if let Err(e) = step {
                let msg = e.to_string();
                return Err(fail_step(hub, task_id, "install_binary", msg).await);
            }
        }
        hub.finish_step(
            task_id,
            "install_binary",
            DeployStepState::Done,
            Some(&format!("已安装到 {target}")),
        )
        .await;
    }

    // ---- 5. 配置 ----
    hub.start_step(task_id, "write_config").await;
    for dir in [&cfg_dir, &log_dir] {
        let _ = exec::run(
            &session,
            AllowedCommand::MkdirP { path: dir.clone() },
            &sudo,
            None,
        )
        .await;
    }

    let cfg_exists = exec::run(
        &session,
        AllowedCommand::FileExists {
            path: paths.config.clone(),
        },
        &sudo,
        None,
    )
    .await
    .map(|o| o.stdout.trim() == "yes")
    .unwrap_or(false);
    if cfg_exists {
        let bak = format!("{}.bak.{stamp}", paths.config);
        exec::run_checked(
            &session,
            AllowedCommand::Copy {
                src: paths.config.clone(),
                dst: bak.clone(),
            },
            &sudo,
            None,
            "备份旧配置",
        )
        .await?;
        hub.log(task_id, "stdout", &format!("已备份旧配置 → {bak}"))
            .await;
        let _ = crate::db::repo::insert_backup(
            &st.db,
            row.id,
            &bak,
            &crate::crypto::sha256_hex(input.config.as_bytes()),
            input.config.len() as i64,
            "deploy",
        )
        .await;
        old_config_backup = Some(bak);
    }

    let tmp_cfg = format!("{}.frppanel.tmp", paths.config);
    if let Err(e) = exec::run_checked(
        &session,
        AllowedCommand::WriteFile {
            path: tmp_cfg.clone(),
        },
        &sudo,
        Some(&input.config),
        "写入临时配置",
    )
    .await
    {
        let msg = e.to_string();
        return Err(fail_step(hub, task_id, "write_config", msg).await);
    }
    if let Err(e) = exec::run_checked(
        &session,
        AllowedCommand::Move {
            src: tmp_cfg,
            dst: paths.config.clone(),
        },
        &sudo,
        None,
        "原子替换配置",
    )
    .await
    {
        let msg = e.to_string();
        return Err(fail_step(hub, task_id, "write_config", msg).await);
    }
    hub.finish_step(task_id, "write_config", DeployStepState::Done, Some("配置已下发"))
        .await;

    // ---- 6. 服务托管 ----
    let use_systemd = row.service_mode == ServiceMode::Systemd && env.systemd;
    if use_systemd {
        hub.start_step(task_id, "write_unit").await;
        let unit = crate::systemd::render_unit(
            &input.unit_name,
            row.role,
            &paths.binary,
            &paths.config,
            &log_dir,
            &paths.work_dir,
        );
        match exec::run(
            &session,
            AllowedCommand::WriteUnit {
                unit: input.unit_name.clone(),
            },
            &sudo,
            Some(&unit),
        )
        .await
        {
            Ok(o) if o.ok() => {
                hub.finish_step(task_id, "write_unit", DeployStepState::Done, Some("unit 已写入"))
                    .await
            }
            Ok(o) => {
                let msg = format!("写入 unit 失败：{}", o.combined());
                return Err(fail_step(hub, task_id, "write_unit", msg).await);
            }
            Err(e) => {
                let msg = e.to_string();
                return Err(fail_step(hub, task_id, "write_unit", msg).await);
            }
        }

        hub.start_step(task_id, "daemon_reload").await;
        if let Err(e) = exec::run_checked(
            &session,
            AllowedCommand::DaemonReload,
            &sudo,
            None,
            "systemctl daemon-reload",
        )
        .await
        {
            let msg = e.to_string();
            return Err(fail_step(hub, task_id, "daemon_reload", msg).await);
        }
        hub.finish_step(task_id, "daemon_reload", DeployStepState::Done, Some("已重载"))
            .await;

        hub.start_step(task_id, "enable_service").await;
        if let Err(e) = exec::run_checked(
            &session,
            AllowedCommand::Systemctl {
                action: "enable".into(),
                unit: input.unit_name.clone(),
            },
            &sudo,
            None,
            "启用开机自启",
        )
        .await
        {
            let msg = e.to_string();
            return Err(fail_step(hub, task_id, "enable_service", msg).await);
        }
        hub.finish_step(task_id, "enable_service", DeployStepState::Done, Some("已启用"))
            .await;
    } else {
        hub.start_step(task_id, "write_pid_dir").await;
        let _ = exec::run(
            &session,
            AllowedCommand::MkdirP {
                path: log_dir.clone(),
            },
            &sudo,
            None,
        )
        .await;
        hub.log(
            task_id,
            "stdout",
            "非 systemd 模式：使用 nohup 守护（不会开机自启）",
        )
        .await;
        hub.finish_step(task_id, "write_pid_dir", DeployStepState::Done, Some("已准备"))
            .await;
    }

    // ---- 7. 启动 ----
    hub.start_step(task_id, "start_service").await;
    let start = if use_systemd {
        exec::run(
            &session,
            AllowedCommand::Systemctl {
                action: "restart".into(),
                unit: input.unit_name.clone(),
            },
            &sudo,
            None,
        )
        .await
    } else {
        exec::run(
            &session,
            AllowedCommand::NohupStart {
                binary: paths.binary.clone(),
                config: paths.config.clone(),
                log: paths.log.clone(),
            },
            &sudo,
            None,
        )
        .await
    };

    let start_failed = match &start {
        Ok(o) if o.ok() => None,
        Ok(o) => Some(format!("启动失败：{}", o.combined())),
        Err(e) => Some(e.to_string()),
    };
    if let Some(msg) = start_failed {
        hub.finish_step(task_id, "start_service", DeployStepState::Failed, Some(&msg))
            .await;
        rollback(st, hub, &session, task_id, &sudo, &old_binary_backup, &old_config_backup, &input.binary_name, row)
            .await;
        return Err(ApiError::upstream(msg));
    }
    hub.finish_step(task_id, "start_service", DeployStepState::Done, Some("已启动"))
        .await;

    // ---- 8. 等待 Admin API ----
    hub.start_step(task_id, "wait_admin_api").await;
    if !wait_admin_api(&input.admin_url, &input.admin_user, &input.admin_password).await {
        let msg = format!(
            "启动后 {} 仍不可达（可能只监听 127.0.0.1，或配置里的面板端口不一致）",
            input.admin_url
        );
        hub.finish_step(task_id, "wait_admin_api", DeployStepState::Failed, Some(&msg))
            .await;
        rollback(st, hub, &session, task_id, &sudo, &old_binary_backup, &old_config_backup, &input.binary_name, row)
            .await;
        return Err(ApiError::upstream(msg));
    }
    hub.finish_step(
        task_id,
        "wait_admin_api",
        DeployStepState::Done,
        Some("Admin API 就绪"),
    )
    .await;

    // ---- 9. 校验 ----
    hub.start_step(task_id, "verify_status").await;
    let ver = exec::run(
        &session,
        AllowedCommand::BinaryVersion {
            binary: paths.binary.clone(),
        },
        &sudo,
        None,
    )
    .await
    .map(|o| o.stdout.trim().to_string())
    .ok()
    .filter(|v| !v.is_empty());
    hub.finish_step(
        task_id,
        "verify_status",
        DeployStepState::Done,
        Some(&format!(
            "部署完成，二进制版本 {}",
            ver.clone().unwrap_or_default()
        )),
    )
    .await;

    crate::db::repo::update_instance(
        &st.db,
        row.id,
        InstanceUpdate {
            deployment_status: Some(DeploymentStatus::Deployed),
            binary_version: Some(ver),
            last_deployed_at: Some(Some(crate::db::now_str())),
            last_error: Some(None),
            ..Default::default()
        },
    )
    .await?;

    let _ = session.disconnect().await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn rollback(
    st: &Arc<AppState>,
    hub: &DeployHub,
    session: &SshSession,
    task_id: &str,
    sudo: &SudoPrefix,
    old_binary: &Option<String>,
    old_config: &Option<String>,
    binary_name: &str,
    row: &InstanceRow,
) {
    hub.log(task_id, "stderr", "开始回滚…").await;
    let target_bin = format!("{}/{}", row.paths.remote_root(), binary_name);
    let mut did = false;
    if let Some(bak) = old_binary {
        if exec::run(
            session,
            AllowedCommand::Copy {
                src: bak.clone(),
                dst: target_bin,
            },
            sudo,
            None,
        )
        .await
        .is_ok()
        {
            hub.log(task_id, "stderr", &format!("已回滚二进制 → {bak}"))
                .await;
            did = true;
        }
    }
    if let Some(bak) = old_config {
        if exec::run(
            session,
            AllowedCommand::Copy {
                src: bak.clone(),
                dst: row.paths.config.clone(),
            },
            sudo,
            None,
        )
        .await
        .is_ok()
        {
            hub.log(task_id, "stderr", &format!("已回滚配置 → {bak}"))
                .await;
            did = true;
        }
    }
    if !did {
        hub.log(task_id, "stderr", "没有可用的备份（首次部署），已保留现场供排查")
            .await;
        return;
    }
    let _ = crate::db::repo::update_instance(
        &st.db,
        row.id,
        InstanceUpdate {
            deployment_status: Some(DeploymentStatus::RolledBack),
            ..Default::default()
        },
    )
    .await;
    hub.set_state(task_id, DeployState::RolledBack).await;
}

fn dir_of(path: &str, fallback: &str) -> String {
    path.rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn sudo_prefix(st: &Arc<AppState>, row: &InstanceRow) -> Result<SudoPrefix, ApiError> {
    match &row.connection {
        crate::domain::Connection::Ssh { sudo, sudo_cmd, .. } => SudoPrefix::build(
            *sudo,
            sudo_cmd.as_deref(),
            st.cfg.raw.ssh.allow_custom_command,
        ),
        _ => Ok(SudoPrefix::default()),
    }
}

/// 等待 Admin API 就绪（轮询多个已知端点）。
async fn wait_admin_api(url: &str, user: &str, password: &str) -> bool {
    if url.trim().is_empty() {
        return true; // 没配 Admin API 就跳过
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
    else {
        return false;
    };
    let base = url.trim().trim_end_matches('/');
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    while std::time::Instant::now() < deadline {
        for path in [
            "/api/healthz",
            "/api/v2/status",
            "/api/status",
            "/api/v2/clients?all=1",
        ] {
            let rb = client.get(format!("{base}{path}"));
            let rb = if user.is_empty() {
                rb
            } else {
                rb.basic_auth(user, Some(password))
            };
            if let Ok(resp) = rb.send().await {
                if resp.status().is_success() {
                    return true;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    false
}

/// 供 API 层复用：从实例构造 SFTP 传输上下文。
pub async fn transfer_context_for(
    st: &Arc<AppState>,
    row: &InstanceRow,
) -> Result<sftp::TransferContext, ApiError> {
    let driver = st.driver(row.id).await?;
    let Some(ssh) = crate::driver::ssh::as_ssh_driver(&driver) else {
        return Err(ApiError::bad_request("该实例不是 SSH 远程实例"));
    };
    let params = ssh.connect_params().await?;
    Ok(sftp::TransferContext::from_params(&params, None))
}

/// 判读二进制文件名（`frps` / `frpc`）。
pub fn binary_name_for(role: crate::domain::InstanceRole) -> &'static str {
    match role {
        crate::domain::InstanceRole::Server => "frps",
        crate::domain::InstanceRole::Client => "frpc",
    }
}

/// 默认 unit 名。
pub fn default_unit_name(role: crate::domain::InstanceRole, flavor: BinaryFlavor) -> String {
    let prefix = match flavor {
        BinaryFlavor::Nfrp => "nfrp",
        BinaryFlavor::OfficialFrp => "frp",
    };
    format!("{prefix}-{}", role.as_str())
}
