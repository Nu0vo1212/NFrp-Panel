# FrpPanel

> 面向 **NFrp（Rust-Frp）** 与**官方 frp**（frps / frpc）的统一 Web 管理面板。
>
> Rust 后端（Axum + SQLite）+ Vue 3 + TypeScript 前端；**默认本机二进制模式**，
> 可选 SSH 远程部署与 Admin API 纳管；不引入 Go 后端、不做黑盒中间层。

---

## 一、它解决什么问题

NFrp 与官方 frp 都能跑，但两者的**管理面**完全不同：

| 能力 | NFRp 0.7.4（实测） | 官方 frp 0.71.1（实测） |
|---|---|---|
| 服务端状态 | `GET /api/status`、`/api/v2/status` | 只有 `/api/v2/clients`、`/api/v2/proxies` |
| 健康检查 | `GET /api/healthz` | **无** |
| 客户端读配置 | 无 | `GET /api/config`（返回 TOML 原文） |
| 代理动态增删 | 服务端推送 + 客户端 `/api/proxies/add\|remove` | **不可用**（`PUT /api/config` 返回 200 但不生效） |
| 热重载 | 轮询配置文件 mtime，无 HTTP 端点 | 服务端无热重载；frpc 需 `--enableReload` |
| 指标粒度 | 流量字节 + 连接数 + 客户端数 | 仅连接数 / 在线态 |
| 未知配置字段 | 宽容 | **零容忍**（`unknown field` 直接退出） |

面板把这些差异收敛成一套 **driver trait + 能力探测**，前端只认一份统一 DTO。
不支持的能力在界面上**灰化并说明原因**，而不是假装成功。

> 实测原始数据与复现步骤见 [`docs/04-verified-behavior.md`](docs/04-verified-behavior.md)。

---

## 二、架构

```
Vue 3 + TS (Naive UI 风格自绘 + ECharts)
   │  /api/*（JWT；只与本后端通信，永不直连 frp Admin API）
   ▼
Axum API 层（api/）── RBAC ── 审计
   │
   ▼
Driver trait（driver/）───────────────────────────────────────┐
 ├─ BinaryDriver    本机进程 + 配置文件 + Admin API           │
 ├─ SshBinaryDriver SSH 部署 + systemd/nohup + 远程配置        │
 ├─ AdminApiDriver  仅 Admin API（不接管进程、不拥有配置）     │
 └─ EmbeddedDriver  预留：进程内链接 nfrp-common              │
   │                                                          │
   ├── frpapi/      ★ NFRp / 官方 frp 双实现适配 + 能力探测   │
   ├── configfile/  校验（复用 nfrp-common）+ 备份 + 原子写    │
   ├── ssh/         russh 会话 + 主机指纹 + 白名单命令 + sftp  │
   ├── deploy/      预检 → 分步执行 → SSE 流 → 失败回滚        │
   ├── metrics/     采样落库 + 趋势查询
   └── db/          SQLite（实例、凭据密文、审计、备份、任务）
```

分层约束（代码里有对应的模块文档）：

- `api/` 只做「参数 → 领域调用 → DTO」，**不允许**直接发 frp 请求；
- `driver/` 只回答「这个实例能做什么」，能力位是前端唯一的驱动差异来源；
- `frpapi/` 是兼容性的唯一出口，NFRp / 官方的差异只在这里；
- `db/` 不出现任何驱动细节。

更多设计说明：[`docs/01-architecture.md`](docs/01-architecture.md) ·
API 契约：[`docs/02-api.md`](docs/02-api.md)。

---

## 三、快速开始

### 前置

| 依赖 | 版本 | 说明 |
|---|---|---|
| Rust | ≥ 1.80 | 后端 |
| Node.js | ≥ 20 | 前端；用 npm 即可（无需 pnpm） |
| SQLite | 内嵌 | 由 sqlx 自带，无需单独安装 |
| OpenSSH 客户端 | 任意 | **仅 SSH 部署**需要（`sftp` 命令）；Windows 10+ 自带 |
| nfrp-common | 源码 | 通过 `path` 依赖本地 NFRp 仓库，见下 |

### 1) 关联 nfrp-common

`Cargo.toml` 里以 path 方式复用 NFRp 的配置解析与校验：

```toml
nfrp-common = { path = "E:/项目/RustFrp/nfrp/common" }
```

改成你本机的 NFRp 源码路径即可（这是"与 NFRp 同源"的具体体现，不是中间层）。

### 2) 启动后端

```bash
cargo run -p frp-panel -- --bind 127.0.0.1:8787
```

首次启动会：

- 在 `data/` 下生成 `panel.db`、`master.key`、`jwt.key`（密钥文件权限收紧到 0600）；
- 创建管理员 `admin`，**随机口令写入 `data/admin.initial`** 并打印到控制台一次。

> 也可以用 `FRP_PANEL_ADMIN_PASSWORD` 环境变量指定初始口令（仅首次创建时生效）。

### 3) 启动前端（开发模式）

```bash
cd frontend
npm install
npm run dev      # http://127.0.0.1:5173 ，/api 代理到 8787
```

### 4) 生产模式（单进程）

```bash
cd frontend && npm ci && npm run build
cd .. && FRP_PANEL_STATIC_DIR=frontend/dist cargo run --release -p frp-panel
# 访问 http://127.0.0.1:8787
```

后端会把 `static_dir` 作为静态目录挂载，未命中的路径回退到 `index.html`（SPA 路由）。

### 5) 桌面控制台（可选）

不想开两个终端时，双击 `panel_gui.bat`，或运行：

```bash
python panel_gui.py
```

它只用 Python 标准库（Tkinter），提供：

- 一键启动 / 停止 / 重启后端与前端，实时查看两边日志；
- 自动发现 `dev.bat` 已启动的实例，端口被占用时自动换端口；
- 打开面板、查看初始管理员口令；
- **直接重置登录密码**：不需要旧密码，直接写入本地 SQLite，后端运行中也能立即生效。

忘记管理员密码时，在窗口里填「新密码 / 确认新密码」，点「直接重置密码」即可。
等价的命令行方式（新密码从 stdin 读取，不会出现在进程列表里）：

```bash
echo 你的新密码 | cargo run -p frp-panel -- --reset-admin-password --admin-username admin
```

重置使用与后端相同的 Argon2 哈希；成功后旧的 `data/admin.initial` 会被删除。
此入口只适合本机管理员使用，不要把它包装成对公网开放的重置接口。

也可以用 `dev.bat` 启动开发环境：

```bat
dev.bat            :: 后端 + 前端，自动选空闲端口
dev.bat status
dev.bat stop
```

---

## 四、驱动模式

| 驱动 | 接管进程 | 拥有配置 | 能做什么 | 不能做什么 |
|---|---|---|---|---|
| **BinaryDriver**（默认） | ✅ 本机 | ✅ | 启停、读写配置、reload、代理管理、指标 | — |
| **SshBinaryDriver** | ✅ 远程 | ✅ | 同上 + 远程部署/预检/回滚 | 需要 SSH 凭据与 sudo |
| **AdminApiDriver** | ❌ | ❌ | 状态、代理（仅 NFRp）、指标 | 启停、编辑配置、日志 |
| **EmbeddedDriver** | — | — | 预留接口 | 第一版未实现，调用会返回明确错误 |

**怎么选**：默认用 BinaryDriver；要管远程服务器用 SshBinaryDriver；
只是想把一个**已经在跑**的实例纳管起来看一眼（不改配置、不重启），用 AdminApiDriver。

---

## 五、SSH 部署

完整流程（向导 9 步的前 8 步对应后端接口）：

1. **主机指纹确认** — 首次连接必须核对 `SHA256:…` 指纹，信任后才继续；指纹变更会被拒绝。
2. **环境探测** — OS/内核/架构、systemd、sudo 可用性、磁盘、已装 frp、端口占用。
3. **安装来源** — 使用远端已有 / 上传本地二进制 / 从 URL 下载（建议带 SHA256）。
4. **预检** — 架构匹配（读 ELF 头）、目录可写、端口空闲、sudo 权限等逐项给出 `pass/warn/fail` + 修复建议。
5. **部署执行** — 分步推进，SSE 实时推送步骤状态与日志。

部署顺序与安全细节：

```
探测 → 上传（SFTP，凭据不落盘）→ 校验（sha256 + ELF 架构）
     → 备份旧二进制 → 安装（mv 原子替换 + chmod 0755）
     → 备份旧配置 → 写临时文件 → mv 原子替换
     → 写 systemd unit（无 systemd 自动回退 nohup）→ daemon-reload → enable
     → 启动 → 轮询 Admin API 就绪（25s）→ 校验版本
```

**失败即回滚**：启动失败或 Admin API 未就绪时，自动把备份的二进制与配置复制回去，
并把任务状态标为 `rolled_back`（前端步骤条会显示回滚原因与修复建议）。

systemd 模板见 [`backend/src/systemd.rs`](backend/src/systemd.rs)（含 `Restart=on-failure`
与日志方向说明）；无 systemd 时的 nohup 回退脚本也在同一模块。

### 安全约束

- 凭据（SSH 密码/私钥、Admin API 密码）用 **AES-256-GCM** 加密，AAD 绑定
  `credential:<id>:secret`，密文搬移无法解密；
- 前端**永不**回显明文，列表只给 `••••••••` 与"是否已设置"；
- 审计与日志里统一走 `redact_line` 脱敏；
- SSH 只执行**白名单模板命令**（`systemctl`、`install`、`chmod`、`sha256sum`、`mv` 等），
  路径参数还要过字符白名单；自定义提权命令默认关闭；
- 没有"关闭主机密钥校验"的开关。

---

## 六、安全注意事项

1. **面板默认只监听回环地址**。要暴露到公网请用 Nginx/Caddy 反代 + HTTPS，
   并保持 `bind = 127.0.0.1:8787`。
2. **首次登录后立刻改密码**，并删除 `data/admin.initial`。
3. **三个密钥文件不能丢也不能泄露**：`data/master.key`（加密凭据）、
   `data/jwt.key`（签发 token）。丢失 `master.key` 后已存凭据无法解密，需重新录入。
4. **frp 自身的安全配置**面板不代劳：`token` 强度、`transport.tls.force`、
   面板端口凭据这些仍需在 frp 配置里设好；配置诊断会给出提醒。
5. **RBAC 只有两级**（admin / viewer）。viewer 只读；写接口在提取器层就拒绝。
   面板暂无用户管理界面，创建 viewer 需直接写库（见"已知限制"）。

---

## 七、测试

```bash
# 单元测试（88 项：加密、脱敏、配置诊断、命令白名单、探测、指标等）
cargo test -p frp-panel --lib

# 端到端集成测试：用真实二进制跑完整链路
set NFRP_FRPS=E:\项目\RustFrp\dist\bin_test\frps.exe
set NFRP_FRPC=E:\项目\RustFrp\dist\bin_test\frpc.exe
set FRP_FRPS=D:\...\frp_0.71.1_windows_amd64\frps.exe
cargo test -p frp-panel --test e2e_binary -- --nocapture
```

集成测试覆盖：登录/RBAC、NFRp 全生命周期（启动→状态→能力→配置→备份→停止→审计）、
官方 frp 语义（能力位/`restart_required`/拒绝 NFRp 专有段）、AdminApiDriver 边界
（启停返回 409 `driver_unsupported`）、客户端动态代理路由（`admin_api` vs `config_reload`）。

未找到对应二进制时用例会打印"跳过"而不是失败。

---

## 八、已知限制（如实列出）

1. **EmbeddedDriver 未实现** — 预留 trait 与错误路径，调用返回明确提示。
2. **官方 frp 的代理动态增删不可用** — 面板改写配置文件后重启；这是官方 API 的现状，
   不是面板的取舍（0.71.1 实测 `PUT /api/config` 返回 200 但不生效）。
3. **官方 frps 没有热重载** — 保存配置后需重启；可开启"保存后自动重启"。
4. **官方 frps 不暴露流量字节** — 监控只画连接数/在线态，能力位标为 `aggregated`。
5. **SSH 的 SFTP 依赖系统 OpenSSH** — 因为 `russh 0.54` 不含 SFTP 实现，
   面板用 `sftp` 子进程作为传输通道（凭据经 `SSH_ASKPASS` 临时传入、用后即删）。
   主机密钥仍由 russh 校验，`sftp` 另带 `StrictHostKeyChecking=yes` 二次校验。
6. **SSH 模式尚未在真实 Linux 主机上做过端到端验证** — 代码路径、单元测试与预检逻辑完备，
   但本次开发环境只有 Windows，**没有**一台干净 Linux 机器做完整部署实测。
   这一项需要在你的环境里验证（流程见第五节）。
7. **ssh-agent 认证未实现** — 会返回明确错误并建议改用密码/私钥。
8. **没有用户管理界面** — 只有"至少一个管理员"的初始化；创建 viewer 需直接插入 `users` 表。
9. **审计日志不可删除** — 只按 `retention_days` 由数据库保留策略控制；没有清空接口。
10. **远程 Admin API 需要可从面板主机直连** — 若 frp 面板端口只监听 `127.0.0.1`，
    面板访问不到；预检与提示会明确指出，需要改监听地址或自建隧道。
11. **配置编辑会丢失注释** — 保存时按 TOML 值序列化，注释不保留；请以备份回滚兜底。

---

## 九、目录结构

```
backend/
  migrations/0001_init.sql        数据库 schema
  src/api/                        路由与 DTO（薄层）
  src/driver/{binary,ssh,adminapi,embedded}   四种驱动
  src/frpapi/{nfrp,official,probe}           双实现适配与能力探测
  src/configfile/                 校验、备份、原子写、兼容诊断
  src/ssh/{session,exec,sftp,probe,hostkey}  SSH 全套
  src/deploy/{plan,preflight,runner,hub}     部署编排与 SSE
  src/auth, src/audit, src/crypto, src/db    认证、审计、加密、存储
  systemd/                        systemd unit 模板
  tests/e2e_binary.rs             真实二进制端到端测试
frontend/
  src/styles/{tokens,base}.css    设计令牌（浅/深 + backdrop-filter 降级）
  src/components/glass|common|monitor
  src/views/{Login,Instances,InstanceDetail,InstanceWizard,Audit,Settings}
docs/
  01-architecture.md  02-api.md  04-verified-behavior.md
```

## 十、License

Apache-2.0（与 NFRp 一致）。
