# SSH 远程部署

> 本文对应 `SshBinaryDriver`、`deploy/` 与 `ssh/` 三个模块。
> 每一步都能在面板的「部署」页签看到状态、日志与失败原因。

---

## 0. 前置条件

| 项 | 要求 | 检查方式 |
|---|---|---|
| 目标系统 | Linux（systemd 或 nohup） | 预检第 1 项 |
| SSH 服务 | 端口可达，允许密码或公钥登录 | 向导第 2 步「测试连接」 |
| sudo | **免密**（`sudo -n`）或 root | 预检「sudo 权限」 |
| 本机 sftp | 面板所在主机有 `sftp` 命令 | 预检「上传二进制」 |
| 磁盘 | ≥ 200 MB | 预检「磁盘空间」 |

> **sudo 需要密码时面板会明确失败**：面板不做交互式提权，请给对应用户
> `NOPASSWD`，或改用 root 登录，或选择 `sudo -n` 模式。

---

## 1. 首次连接与主机指纹

面板**默认拒绝未知主机**，没有"跳过校验"的开关。流程：

1. 向导第 2 步点击「测试连接」；
2. 若该主机未受信任，后端只完成 SSH 握手、**不发送任何凭据**，返回指纹
   （`SHA256:...`）；
3. 前端展示指纹，要求人工比对；
4. 点击「我已核对，信任该指纹」→ 写入 `host_keys` 表（同时同步到面板的
   known_hosts 供 `sftp` 子进程二次校验）；
5. 指纹与已保存记录不一致时**拒绝连接**——要用新指纹必须先移除旧记录。

比对方法（在目标机上执行）：

```bash
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
# 输出形如：256 SHA256:xxxxxxxx... root@host (ED25519)
```

指纹算法：`SHA256:base64`（与 `ssh-keygen -lf` 一致，去掉末尾 `=`）。

---

## 2. 环境探测

向导第 3 步执行，全部是**只读**命令：

| 探测项 | 命令（白名单生成） |
|---|---|
| OS / 内核 / 架构 / 主机名 | `uname -s; uname -r; uname -m; hostname` |
| systemd | `command -v systemctl && echo yes \|\| echo no` |
| 用户与 sudo | `id -un; sudo -n true && echo sudo:nopasswd \|\| echo sudo:password` |
| 磁盘可用 | `df -Pm <dir> \| awk 'NR==2{print $4}'` |
| 已装 frp | 遍历 `/usr/local/bin/{frps,frpc}`、`/usr/bin/{frps,frpc}` |
| 端口占用 | `ss -Hltn` 或 `netstat -ltn` 匹配端口 |

探测结果决定后续默认值：有 systemd 就用 systemd，没有就自动切到 nohup 并在界面标注。

---

## 3. 安装来源

三种来源，最终都会先落到 `/tmp/frppanel-<name>-<时间戳>` 再安装：

1. **使用远端已有** — 只下发配置，不替换二进制（适合接管已有安装）。
2. **上传本地文件** — 浏览器读取文件 → base64 → 后端解码 → SFTP 上传。
3. **从 URL 下载** — 后端下载（限 128 MB），可带 SHA256 校验。

**校验规则**：

- SHA256 必须一致（上传后远端再算一次 `sha256sum` 比对）；
- 读 ELF 头的 `e_machine` 判断架构（`amd64` / `arm64` / `arm` / `386`），
  与目标机 `uname -m` 不一致直接失败；
- PE 文件（Windows）会被明确拒绝。

---

## 4. 部署执行顺序

```
[probe]           探测环境（只读）
[upload_binary]   SFTP 上传到 /tmp
[verify_binary]   远端 sha256sum + ELF 架构校验
[install_binary]  备份旧二进制 → mv 原子替换 → chmod 0755
[write_config]    备份旧配置 → 写临时文件 → mv 原子替换
[write_unit]      写 /etc/systemd/system/<unit>.service（无 systemd 则跳过）
[daemon_reload]   systemctl daemon-reload
[enable_service]  systemctl enable
[write_pid_dir]   nohup 模式：准备日志目录
[start_service]   systemctl restart 或 nohup 启动
[wait_admin_api]  轮询 /api/healthz → /api/v2/status → /api/status（最多 25 秒）
[verify_status]   读取二进制版本，落库部署状态
```

**幂等**：重复部署会先备份 `<binary>.bak.<时间戳>` 与 `<config>.bak.<时间戳>`，
再覆盖；配置文件用「临时文件 + `mv`」替换，不会出现半个文件。

---

## 5. 失败回滚

以下情况自动回滚（把备份复制回去），并把任务标为 `rolled_back`：

- 启动命令非零退出；
- 25 秒内 Admin API 未就绪。

回滚只还原**二进制与配置**，不会尝试重启服务（避免掩盖问题），
前端会在失败步骤上显示 `remediation` 修复建议。

手动回滚入口：

- 详情页「部署」页签 →「回滚」（回滚配置到最近一次备份）；
- 配置页签 →「历史备份」→ 选择某份 →「恢复」。

---

## 6. systemd unit 模板

模板在 `backend/src/systemd.rs`，渲染结果形如：

```ini
[Unit]
Description=frp server (managed by FrpPanel)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/frps -c /etc/nfrp/frps.toml
WorkingDirectory=/var/lib/nfrp
Restart=on-failure
RestartSec=5s
LimitNOFILE=65535
StandardOutput=journal
StandardError=journal
SyslogIdentifier=nfrp-server
NoNewPrivileges=true
ProtectSystem=full
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

查看日志：`journalctl -u nfrp-server -f`。

### 无 systemd 的回退

同一模块还生成 nohup 守护脚本（`start|stop|restart|status`），使用
`<config>.pid` 记录 PID。**该模式不会开机自启**，界面会标注"非 systemd 模式"。

---

## 7. 安全约束（实现层面）

| 约束 | 实现位置 |
|---|---|
| 主机密钥必须校验，无可关闭开关 | `ssh/hostkey.rs`、`ssh/session.rs::check_server_key` |
| 只执行白名单模板命令 | `ssh/exec.rs::AllowedCommand` |
| 路径参数过字符白名单（拒绝 `; \| & $ ` < > ( ) { } * ? ! " ' \`） | `ssh/exec.rs::check_path` |
| 自定义提权命令默认关闭，且禁止管道/重定向/命令替换 | `ssh/exec.rs::SudoPrefix::build` |
| 凭据加密存储，AAD 绑定记录 ID | `crypto.rs::KeyRing::encrypt` |
| 凭据不落盘（`SSH_ASKPASS` 临时脚本 0600，用后即删） | `ssh/sftp.rs::TempSecret` |
| 审计记录主机/操作/结果/耗时，不含凭据 | `audit/mod.rs::redact_json` |

---

## 8. 手工验证清单（在一台干净 Linux 上）

```bash
# 1) 目标机准备（root 或有免密 sudo 的用户）
uname -m                      # 记下架构，决定上传哪个包
ls /etc/ssh/ssh_host_*_key.pub
df -h /usr/local/bin

# 2) 面板侧：新建实例 → SSH → 测试连接 → 信任指纹 → 探测
#    预期：探测结果显示 os/systemd/sudo/磁盘/已有安装

# 3) 上传对应架构的 frps → 预检
#    预期：全部 pass（除"已有安装"可能是 warn）

# 4) 执行部署
#    预期：步骤条依次变绿，日志流出现
#          "已上传 N 字节" / "sha256 一致" / "已安装到 /usr/local/bin/frps" / "Admin API 就绪"

# 5) 目标机上核对
systemctl status nfrp-server
/usr/local/bin/frps -v
ls -l /etc/nfrp/ /usr/local/bin/frps*.bak.*

# 6) 面板侧：状态页应显示 running + Admin API 可达
# 7) 故意写错配置（例如端口冲突）再部署 → 预期失败并回滚
```

---

## 9. 已知限制

1. **SFTP 依赖系统 OpenSSH 客户端**（`russh 0.54` 不含 SFTP 实现）。
   主机密钥仍由 russh 校验，`sftp` 另带 `StrictHostKeyChecking=yes`。
2. **sudo 需要密码的场景不支持**（面板不做交互式提权对话）。
3. **ssh-agent 认证未实现**，会返回明确错误。
4. **未在真实 Linux 主机上做端到端验证**：本次开发环境为 Windows，
   代码路径与单元测试完备，但完整部署链路需要你在目标环境验证（见第 8 节清单）。
5. **Admin API 必须可从面板主机直连**：若 frp 面板端口只监听 `127.0.0.1`，
   `wait_admin_api` 会失败并回滚；请把 `webServer.addr` 改为 `0.0.0.0`
   （务必配强口令）或自建隧道。
