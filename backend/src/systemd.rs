//! systemd unit 模板生成。
//!
//! 目标：给出**可直接用**的 unit，包含重启策略与日志方向；
//! 同时在注释里写清"非 systemd 时的回退"，避免用户以为只能这样。

use crate::domain::InstanceRole;

/// 渲染 unit 文件内容。
pub fn render_unit(
    name: &str,
    role: InstanceRole,
    binary: &str,
    config: &str,
    log_dir: &str,
    work_dir: &str,
) -> String {
    let desc = match role {
        InstanceRole::Server => "frp server (managed by FrpPanel)",
        InstanceRole::Client => "frp client (managed by FrpPanel)",
    };
    let wd = if work_dir.trim().is_empty() {
        "/var/lib/frp"
    } else {
        work_dir.trim()
    };
    format!(
        r#"[Unit]
# 由 FrpPanel 生成；修改后请执行 systemctl daemon-reload
Description={desc}
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={binary} -c {config}
WorkingDirectory={wd}
Restart=on-failure
RestartSec=5s
LimitNOFILE=65535

# 日志：进程直接写 stdout/stderr，由 journald 收集；
# 如需落文件，可在 frp 配置里设 log.to = "{log_dir}/frp.log"
StandardOutput=journal
StandardError=journal
SyslogIdentifier={name}

# 安全加固：frp 通常不需要写系统目录
NoNewPrivileges=true
ProtectSystem=full
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
"#
    )
}

/// 非 systemd 环境的回退脚本（nohup 守护）。
pub fn fallback_script(name: &str, binary: &str, config: &str, log: &str) -> String {
    format!(
        r#"#!/bin/sh
# FrpPanel 生成：无 systemd 时的 nohup 守护
# 注意：该方式不会开机自启；建议改用 systemd 或外部进程守护（如 supervisord）
BIN={binary}
CFG={config}
LOG={log}
PID={config}.pid

case "$1" in
  start)
    if [ -f "$PID" ] && kill -0 "$(cat $PID)" 2>/dev/null; then
      echo "{name} 已在运行 (PID $(cat $PID))"; exit 0
    fi
    nohup "$BIN" -c "$CFG" >> "$LOG" 2>&1 &
    echo $! > "$PID"
    echo "{name} 已启动 (PID $(cat $PID))"
    ;;
  stop)
    if [ -f "$PID" ]; then
      kill "$(cat $PID)" 2>/dev/null
      sleep 1
      kill -9 "$(cat $PID)" 2>/dev/null
      rm -f "$PID"
      echo "{name} 已停止"
    else
      echo "$PID 不存在，可能未运行"
    fi
    ;;
  restart)
    "$0" stop; sleep 1; "$0" start
    ;;
  status)
    if [ -f "$PID" ] && kill -0 "$(cat $PID)" 2>/dev/null; then
      echo "{name} 运行中 (PID $(cat $PID))"
    else
      echo "{name} 未运行"
    fi
    ;;
  *)
    echo "用法：$0 {{start|stop|restart|status}}"; exit 1
    ;;
esac
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_contains_required_directives() {
        let u = render_unit(
            "nfrp-server",
            InstanceRole::Server,
            "/usr/local/bin/frps",
            "/etc/nfrp/frps.toml",
            "/var/log/nfrp",
            "/var/lib/nfrp",
        );
        assert!(u.contains("ExecStart=/usr/local/bin/frps -c /etc/nfrp/frps.toml"));
        assert!(u.contains("Restart=on-failure"));
        assert!(u.contains("WantedBy=multi-user.target"));
        assert!(u.contains("SyslogIdentifier=nfrp-server"));
        assert!(u.contains("[Service]"));
    }

    #[test]
    fn unit_for_client_uses_correct_description() {
        let u = render_unit(
            "nfrp-client",
            InstanceRole::Client,
            "/usr/local/bin/frpc",
            "/etc/nfrp/frpc.toml",
            "/var/log/nfrp",
            "",
        );
        assert!(u.contains("frp client"));
        assert!(u.contains("/var/lib/frp") || u.contains("WorkingDirectory="));
    }

    #[test]
    fn fallback_script_has_all_commands() {
        let s = fallback_script(
            "nfrp-server",
            "/usr/local/bin/frps",
            "/etc/nfrp/frps.toml",
            "/var/log/nfrp/frps.log",
        );
        for cmd in ["start)", "stop)", "restart)", "status)"] {
            assert!(s.contains(cmd), "缺少 {cmd}");
        }
        assert!(s.contains("nohup"));
        assert!(s.contains("不会开机自启"));
    }
}
