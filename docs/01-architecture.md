# FrpPanel 架构设计

> 面向 Rust-Frp（NFrp）与官方 frp（frps/frpc）的统一 Web 管理面板。
> 本文是**第一份交付物**：目录结构、driver trait、配置模型、API 设计。
> 所有结论均来自对 `E:\项目\nfrp`（NFrp 0.7.4 源码）与官方 frp 0.71.1
> 的**实测探测**，不是推测（探测方法与原始响应见 [`04-verified-behavior.md`](./04-verified-behavior.md)）。

---

## 0. 设计结论（先看这里）

| 决策 | 结论 | 依据 |
|---|---|---|
| 后端语言 | Rust（Axum 0.8 + tokio + sqlx/SQLite） | 需求硬约束：不用 Go、不引黑盒中间层 |
| 驱动模型 | `Driver` trait，4 实现 + 1 预留 | 需求第三节 |
| 默认驱动 | `BinaryDriver`（本机进程 + 配置文件 + Admin API） | 需求第二节 |
| 前端感知 | 前端**只认一套统一 DTO**，driver 差异由后端适配层吸收 | 需求「同一套前端页面适配所有驱动」 |
| 能力探测 | 启动/连接时探测，结果缓存并落库，前端展示 | 需求「启动时做能力探测」 |
| Admin API 认证 | Basic Auth，**服务端注入**，密钥永不进浏览器 | 需求「不暴露 frp 管理密钥」 |
| 敏感信息 | AES-256-GCM 信封加密 + 脱敏展示 + 审计脱敏 | 需求第七节 |
| 配置写入 | 校验 → 时间戳备份 → 临时文件 → fsync → rename | 需求第三节 |

---

## 1. 实测得到的关键事实（决定了整个架构）

完整原始数据见 `04-verified-behavior.md`，这里只列**会改变设计**的几条：

1. **服务端没有「reload」端点，也没有可重载的信号。**
   NFrp 服务端与官方 frps 的热重载都靠**轮询配置文件 mtime**（`server/src/reload.rs::watch`
   每秒轮询）。Windows 上 `taskkill`（优雅终止）对官方 frpc **直接退出**，不支持 SIGHUP 式 reload。
   ⇒ `reload()` 必须按实现分流：**写文件 + 等 mtime 生效 + 用 status 校验**，或**重启进程**。
2. **两套 Admin API 的响应结构完全不同，必须走适配层。**

   | 能力 | NFrp 0.7.4 | 官方 frp 0.71.1 |
   |---|---|---|
   | 服务端状态 | `GET /api/status`（或 `/api/v2/status`，**无鉴权**） | `GET /api/v2/clients` `/api/v2/proxies`（**Basic Auth**） |
   | 客户端状态 | `GET /api/status` → `{data:{proxies:[...]}}` | `GET /api/status` → `{"tcp":[{"name","status","local_addr","remote_addr"}]}` |
   | 客户端读配置 | 无此端点 | `GET /api/config`（返回 TOML 原文） |
   | 动态加代理（服务端推） | `POST /api/proxies/add` / `/api/v2/proxies/add` | 无（v2 只有 `clients/kick`） |
   | 动态加/删代理（客户端） | `/api/proxies/add` `/api/proxies/remove`（**需 `X-Nfrp-Client: 1`**） | `PUT /api/config`（本版本实测**静默无效**，见下） |
   | 版本 | `GET /api/v2/version` | 无（从 `-v` 或 `clients.version` 拿） |
   | 健康检查 | `GET /api/healthz`（免鉴权） | 无（404） |
   | Prometheus | `GET /metrics`（需 `enablePrometheus`）+ **** | `GET /metrics`（需 `enablePrometheus`） |

3. **官方 frp 0.71.1 的 `PUT /api/config` 返回 200 但代理并未生效。**
   实测 `PUT /api/config`（扁平 / `{name,type,params}` 两种体）与 `PUT /api/config/{name}`
   分别 200 / 404，且 `/api/status` 始终只有配置文件里的代理，日志无新增。
   ⇒ 官方 frpc 的代理动态管理要**以配置文件改写 + reload 为准**，Admin API 仅作为「列表来源」。
4. **NFrp 服务端推送的代理带 `dynamic` 语义、且 0.5.5 起「远程下发不能删/覆盖配置文件里定义的代理」。**
   ⇒ 面板必须区分「配置内代理」与「面板动态代理」，删除时给出明确能力提示，不能假装统一。
5. **客户端 UI 写接口的 CSRF 防护头**：NFrp frpc 写操作要求 `X-Nfrp-Client: 1`。
   实测缺该头返回 `403 forbidden`。⇒ Admin API 客户端按实现自动附加该头。
6. **两边的端口/字段命名不同**：NFrp 配置是 snake_case（`bind_port`、`dashboard_port`、
   `webServer.user` 同时接受），官方是 camelCase（`bindPort`、`webServer.user`）。
   `nfrp-common` 的 `config` + `frp_config` 两层正好能双向吃下 ⇒ **复用 `nfrp-common` 做解析与校验**（仅二进制模式，见 §5 EmbeddedDriver）。
7. **服务端端口占用是硬失败**：给一个已被占用的 remote_port 动态加代理，NFrp 回
   `502 admin_failed: 监听 127.0.0.1:17081 失败`。⇒ 面板预检要先做端口占用检测。

---

## 2. 目录结构

```
Frp面板/
├── Cargo.toml                     # Rust workspace（后端）
├── README.md
├── docs/
│   ├── 01-architecture.md         # ← 本文
│   ├── 02-api.md                  # API 契约（含 TS 类型来源）
│   ├── 03-ssh-deploy.md           # SSH 部署与预检流程
│   └── 04-verified-behavior.md    # 实测行为记录（证据链）
├── backend/
│   ├── Cargo.toml
│   ├── migrations/                # sqlx migrations（SQLite）
│   │   └── 0001_init.sql
│   ├── systemd/                   # unit 模板（frps/frpc，含 nohup 回退）
│   │   ├── nfrp-server.service
│   │   ├── nfrp-client.service
│   │   └── frp-panel-fallback.sh
│   ├── tests/                     # 集成测试（含真实二进制端到端）
│   └── src/
│       ├── main.rs                # 启动、配置装载、路由挂载
│       ├── lib.rs                 # 便于集成测试复用
│       ├── config.rs              # 面板自身配置（panel.toml）+ 环境变量覆盖
│       ├── state.rs               # AppState（db/registry/crypto/metrics）
│       ├── error.rs               # ApiError → 统一错误信封
│       ├── api/                   # HTTP 层（薄）：路由 + DTO 映射 + 鉴权提取
│       │   ├── mod.rs
│       │   ├── auth.rs            # /api/auth/*
│       │   ├── instances.rs       # /api/instances/*
│       │   ├── proxies.rs         # /api/instances/:id/proxies*
│       │   ├── logs.rs            # /api/instances/:id/logs
│       │   ├── metrics.rs         # /api/instances/:id/metrics
│       │   ├── deploy.rs          # /api/instances/:id/deploy/* + SSE
│       │   ├── ssh.rs             # /api/ssh/*
│       │   ├── credentials.rs     # /api/credentials/*
│       │   ├── audit.rs           # /api/audit-logs
│       │   └── settings.rs        # /api/settings
│       ├── auth/                  # 认证与 RBAC
│       │   ├── mod.rs
│       │   ├── jwt.rs             # HS256（jsonwebtoken）
│       │   ├── password.rs        # Argon2id
│       │   └── rbac.rs            # admin / viewer + 资源级判定
│       ├── crypto.rs              # AES-256-GCM 信封加密（主密钥来自文件/环境变量）
│       ├── db/
│       │   ├── mod.rs             # sqlx SqlitePool
│       │   ├── models.rs          # Instance / Credential / AuditLog / User / MetricSample
│       │   └── repo/              # 每个聚合一个仓储
│       ├── domain/                # 与驱动无关的领域模型与 DTO
│       │   ├── mod.rs
│       │   ├── instance.rs        # InstanceRecord、InstanceSpec、Capabilities
│       │   ├── status.rs          # InstanceStatus、ProxyInfo、MetricsSnapshot
│       │   └── config_doc.rs      # ConfigDoc（原文 + 解析结果 + 兼容提示）
│       ├── driver/                # ★ 驱动抽象
│       │   ├── mod.rs             # Driver trait + DriverFactory
│       │   ├── kind.rs            # DriverKind / BinaryFlavor / ConnKind
│       │   ├── capability.rs      # 能力位与探测结果
│       │   ├── binary/            # BinaryDriver（本机）
│       │   │   ├── mod.rs
│       │   │   ├── process.rs     # tokio 进程组、优雅停止、日志跟随
│       │   │   └── service.rs     # Windows 服务/libsystemd 检测（可选）
│       │   ├── adminapi/          # AdminApiDriver（外部实例）
│       │   │   └── mod.rs
│       │   ├── ssh/               # SshBinaryDriver（远程，复用 binary 语义）
│       │   │   ├── mod.rs
│       │   │   ├── session.rs     # russh 连接池、超时、重试、阶段化错误
│       │   │   ├── exec.rs        # 白名单命令执行 + sudo 提权
│       │   │   ├── sftp.rs        # 上传/下载/原子替换
│       │   │   ├── hostkey.rs     # 指纹计算与 known_hosts
│       │   │   └── probe.rs       # 远程环境探测
│       │   └── embedded/          # EmbeddedDriver（预留）
│       │       └── mod.rs
│       ├── frpapi/                # ★ frp Admin API 适配层
│       │   ├── mod.rs             # FrpAdminClient（Basic Auth 注入、超时、重试）
│       │   ├── flavor.rs          # 实现识别（NFrp / official / unknown）
\u2502       \u2502   \u251c\u2500\u2500 nfrp.rs            # NFrp 端点与结构
│       │   ├── official.rs        # 官方 frps/frpc 端点与结构
│       │   └── probe.rs           # 能力探测（端点存在性 + 结构嗅探）
│       ├── configfile/            # 配置文档服务
│       │   ├── mod.rs             # 读/校验/备份/原子写
│       │   ├── toml_lint.rs       # TOML 语法 + 语义校验（复用 nfrp-common）
│       │   └── compat.rs          # NFRf ↔ 官方字段差异与提示
│       ├── deploy/                # 部署编排
│       │   ├── mod.rs             # 计划生成、步骤执行、SSE 事件
│       │   ├── plan.rs            # DeployPlan / DeployStep
│       │   ├── source.rs          # 本地上传 / URL 下载 / 内置清单
│       │   ├── preflight.rs       # 预检项
│       │   ├── rollback.rs        # 配置与二进制回滚
│       │   └── catalog.rs         # 版本清单（可配 JSON）
│       ├── logs/                  # 日志查看/搜索/下载（本机 + 远程统一）
│       ├── metrics/               # 采样落库 + 趋势查询
│       ├── audit/                 # 审计写入（含字段脱敏）
│       └── secret/                # 脱敏工具（mask / redact_lines）
└── frontend/
    ├── package.json
    ├── vite.config.ts             # 开发代理 /api → 127.0.0.1:8787
    ├── tsconfig.json
    └── src/
        ├── main.ts
        ├── App.vue
        ├── router/index.ts
        ├── styles/
        │   ├── tokens.css         # ★ 主题变量（唯一真源）
        │   ├── acrylic.css        # 亚克力卡片/降级
        │   └── base.css
        ├── api/
        │   ├── client.ts          # axios 实例、JWT 注入、错误归一
        │   ├── types.ts           # ★ 由 02-api.md 生成的 TS 类型
        │   └── endpoints/         # 每个资源一个模块
        ├── stores/                # Pinia: session / instances / theme / deploy
        ├── components/
        │   ├── glass/             # AcrylicCard / GlassPanel / StatCard
        │   ├── common/            # StateView(空/错/无权限) / Skeleton / ConfirmDanger
        │   ├── monitor/           # TrafficChart / ConnChart（ECharts 封装）
        │   └── deploy/            # StepTimeline / LogStream（等宽流式）
        ├── layouts/               # AppShell（侧栏 224 + 顶部工具栏 + 1280 居中）
        └── views/
            ├── LoginView.vue
            ├── InstancesView.vue
            ├── InstanceDetailView.vue   # 页签：概览/配置/代理/监控/日志/部署/能力
            ├── InstanceWizardView.vue   # 9 步向导
            ├── AuditView.vue
            └── SettingsView.vue
```

**分层规则（后端）**：`api` 只做参数与 DTO；`driver` 只做能力；`frpapi`/`configfile`/`deploy`
是可复用服务；`db` 不出现驱动细节。`api` 层**不允许**直接 `reqwest` 调 frp —— 必须经 `frpapi`。

---

## 3. Driver trait 设计

设计目标：**同一套语义，四种实现**；调用方（API 层）不需要 `match driver_kind()`。
无法提供的可选能力用「能力位 + 明确的 `Unsupported` 错误」表达，而不是撒谎返回空数据。

```rust
// domain/instance.rs
pub enum DriverKind { Binary, AdminApi, SshBinary, Embedded }
pub enum BinaryFlavor { Nfrp, OfficialFrp }
pub enum InstanceRole { Server, Client }   // frps / frpc

// driver/mod.rs
#[async_trait]
pub trait Driver: Send + Sync {
    fn kind(&self) -> DriverKind;

    // —— 生命周期 ——
    async fn start(&self) -> Result<()>;
    async fn stop(&self, opts: StopOptions) -> Result<()>;
    async fn restart(&self) -> Result<()>;
    async fn health(&self) -> Result<HealthReport>;

    // —— 状态 ——
    async fn status(&self) -> Result<InstanceStatus>;
    async fn reload(&self) -> Result<ReloadOutcome>;   // 见 §3.2

    // —— 配置（ConfigDoc = 原文 + 解析 + 兼容提示） ——
    async fn read_config(&self) -> Result<ConfigDoc>;
    async fn validate_config(&self, doc: &ConfigDoc) -> Result<ValidationReport>;
    async fn write_config(&self, doc: &ConfigDoc, opts: WriteOptions) -> Result<BackupInfo>;

    // —— 日志 ——
    async fn logs(&self, query: LogQuery) -> Result<LogPage>;
    async fn follow_logs(&self) -> Result<BoxStream<'static, Result<LogLine>>>;

    // —— 代理 / Visitor ——
    async fn list_proxies(&self) -> Result<Vec<ProxyInfo>>;
    async fn create_proxy(&self, spec: ProxySpec) -> Result<ProxyOpResult>;
    async fn update_proxy(&self, name: &str, spec: ProxySpec) -> Result<ProxyOpResult>;
    async fn delete_proxy(&self, name: &str) -> Result<ProxyOpResult>;

    // —— 指标 ——
    async fn metrics(&self) -> Result<MetricsSnapshot>;

    // —— 能力 ——
    async fn capabilities(&self) -> Result<Capabilities>;
    async fn probe(&self) -> Result<Capabilities>;     // 强制重新探测
}
```

### 3.1 各实现的语义

| 方法 | BinaryDriver | AdminApiDriver | SshBinaryDriver | EmbeddedDriver |
|---|---|---|---|---|
| start/stop/restart | 本机进程（进程组 + 优雅停止 → 强制） | `Unsupported`（不接管进程） | SSH 执行 `systemctl start/stop/restart` 或 nohup 脚本 | 预留：进程内启停 |
| health | PID 存活 + Admin API 可达 + 端口监听 | Admin API 可达 | 远程 PID/systemd active + Admin API | 预留 |
| status | Admin API（服务端/客户端端点分流） | Admin API | 远程 Admin API（经 SSH 隧道或直连） | 预留 |
| reload | 见 §3.2 | `Unsupported`/`RestartRequired` | 远程写文件后同 §3.2 | 预留 |
| read/write_config | 本机文件系统，原子写 + 备份 | `Unsupported`（无配置文件所有权） | SFTP 原子写 + 远程备份 | 预留：内存配置 |
| list_proxies | 配置 + Admin API 合并 | 仅 Admin API | 同 Binary（远程） | 预留 |
| create/delete_proxy | 首选 Admin API 动态；不行 → 配置改写 + reload | 仅 Admin API | 同 Binary（远程） | 预留 |
| metrics | Admin API + Prometheus 抓取 + 面板采样 | 同左 | 同左（远程） | 预留 |

### 3.2 `reload()` 的三种结果（这是本项目的核心正确性点）

```rust
pub enum ReloadOutcome {
    /// 已热生效（写文件后 mtime 轮询确认，或 Admin API 确认）
    Hot,
    /// 该字段无法热改，需要重启（返回不可热改字段列表）
    RestartRequired { fields: Vec<String> },
    /// 当前实现不支持热重载
    Unsupported,
}
```

决策树（依据实测 §1.1）：

```
BinaryDriver/NFrp  → 写文件 → 轮询 mtime 应用（≤3s）→ 抓 /api/v2/status 比对关键字段
                   → 成功 Hot；超时但有变化 → Hot（宽限）；字段无法热改 → RestartRequired
BinaryDriver/Official → 写文件 → 若进程以 `--enableReload` 运行 → 触发 reload；否则 RestartRequired
                       （面板可选 `auto_restart_on_save`：自动优雅重启完成）
Client 侧（两边）  → 重写 [[proxies]] 后重启或触发热加载；官方 frpc 官方支持 SIGHUP/配置 mtime
AdminApiDriver     → Unsupported（只有所有者能改配置）
```

### 3.3 能力位

```rust
pub struct Capabilities {
    pub flavor: BinaryFlavor,
    pub flavor_version: Option<String>,      // NFrp 0.7.4 / 0.71.1
    pub api_kind: ApiKind,                   // NFrp | Official | Unknown
    pub managed_locally: bool,               // 进程由本面板接管
    pub remote: bool,
    pub can_start_stop: bool,
    pub can_reload: bool,
    pub can_edit_config: bool,
    pub can_dynamic_proxy: bool,             // Admin API 直接增删
    pub can_manage_visitors: bool,
    pub has_v2_api: bool,                    // NFrp /api/v2/*
    pub has_healthz: bool,
    pub prometheus: PrometheusSupport,       // Off | Enabled | Unknown
    pub metrics_detail: MetricDetail,        // Full | Aggregated | None
    pub auth_guard_header: bool,             // NFrp 客户端写操作需 X-Nfrp-Client
    pub notes: Vec<CapabilityNote>,          // 面向用户的能力说明/兼容提示
}
```

探测算法（`frpapi/probe.rs`）：
1. 取 Admin API base 与凭据（凭据从面板加密存储中解密，**仅在服务端内存中出现**）。
2. 并发 `GET /api/v2/version`、`GET /api/v2/status`、`GET /api/status`、`GET /api/healthz`。
3. 按命中组合判定 `ApiKind`：`v2/version` 命中且含 `frp_wire_version` ⇒ 极可能是 NFrp；
   `/api/status` 顶层有 `clients[]` ⇒ NFrp 服务端；顶层有 `tcp[]/udp[]/http[]` 键 ⇒ 官方 frpc；
   `/api/v2/*` 返回 `{code,msg,data:{items,total}}` 信封 ⇒ 官方服务端。
4. 版本：`/api/v2/version.data.server` → 否则二进制 `-v`（本机/SSH）→ 否则 `clients.version`。
5. 把结果连同证据（哪个端点返回什么状态码）落库，前端「能力信息」页签原样展示。

> 判定必须**容错且可解释**：任何一项拿不到都不应让探测整体失败，只降低 `ApiKind` 置信度并写入 `notes`。

---

## 4. 配置模型

### 4.1 面板自身配置（`panel.toml`，可被环境变量覆盖）

```toml
[server]
bind = "127.0.0.1:8787"
public_url = ""                  # 可选，用于生成回跳链接
static_dir = ""                  # 为空则由后端内嵌前端产物

[database]
url = "sqlite://data/panel.db"   # 面板自身数据

[security]
jwt_secret_file = "data/jwt.key"         # 权限 0600，缺失自动生成
master_key_file  = "data/master.key"     # 敏感字段信封加密主密钥
access_token_ttl_minutes = 120
refresh_token_ttl_days = 14
initial_admin = { username = "admin", password_file = "data/admin.initial" }  # 首次启动生成随机口令并落 0600 文件

[defaults]
driver = "binary"                # binary | admin_api | ssh_binary | embedded
binary_flavor = "nfrp"           # NFrp | official_frp
binary_dir = "bin"               # 默认二进制/配置/日志根目录
server_binary = "frps"           # 不带扩展名，按平台补 .exe
client_binary = "frpc"
config_dir = "config"
log_dir = "logs"
work_dir = "work"

[frp]
admin_timeout_ms = 5000
admin_retry = 2
status_poll_ms = 10000           # 面板采样周期
metrics_history_days = 7

[ssh]
connect_timeout_ms = 10000
command_timeout_ms = 20000
upload_timeout_ms = 120000
retry = 2
known_hosts_file = "data/known_hosts"
default_port = 22
default_user = "root"
default_sudo = "sudo_n"          # none | sudo | sudo_n | custom
default_sudo_cmd = ""
default_install_dir = "/usr/local/bin"
default_config_dir = "/etc/nfrp"
default_log_dir = "/var/log/nfrp"
default_service = "systemd"      # systemd | nohup
allow_custom_command = false     # 显式开启才允许自定义提权命令

[audit]
enabled = true
retention_days = 90

[theme]
default = "light"
```

### 4.2 实例记录（面板数据库，非 frp 配置）

```rust
pub struct InstanceRecord {
    pub id: Uuid,
    pub name: String,
    pub role: InstanceRole,          // frps / frpc
    pub driver: DriverKind,
    pub flavor: BinaryFlavor,
    pub enabled: bool,
    pub tags: Vec<String>,
    // 连接方式
    pub connection: Connection,       // Local | Ssh { host, port, user, auth_kind, credential_id, sudo }
    // 本机/远程路径与二进制
    pub paths: InstancePaths { binary, config, log, work_dir },
    pub service: ServiceMode,         // Process | Systemd { unit } | Nohup
    // Admin API（密码存加密字段，单独表或加密列）
    pub admin_api: AdminApiConfig { base_url, user, password_secret_id, enabled },
    pub binary_version: Option<String>,
    pub deployment_status: DeploymentStatus,  // NotDeployed | Deployed | Failed | RolledBack
    pub last_deployed_at: Option<DateTime<Utc>>,
    pub last_capabilities: Option<Capabilities>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

> **关键**：`InstanceRecord` 里**只放引用**（`credential_id`、`password_secret_id`），
> 密文单独存 `secrets` 表，便于轮换主密钥与统一审计。

### 4.3 实例的 frp 配置（TOML）—— 复用 `nfrp-common`

`configfile` 服务用 `nfrp-common` 的 `ServerConfig` / `ClientConfig` 解析（它能同时吃
snake_case、camelCase、`localIP+localPort`、`includes`），再做**兼容性诊断**：

| 检查 | 说明 | 提示级别 |
|---|---|---|
| `local_addr` 合并式 vs `localIP+localPort` | NFrp 两处都认，官方只认两段式 | 官方二进制时 **error**，NFrp 时 **info** |
| `[webServer]` 与 `dashboard_*` 同时存在 | NFrp 显式 `dashboard_*` 优先 | warning（语义歧义） |
| `transport.protocol` / `transport_protocol` | 官方语义不同（QUIC 需 TCP 兼容），并与 NFrp 的 `quicBindPort` 区分 | warning |
| 面板绑定非回环但无凭据 | NFrp ≥0.5.2 **拒绝启动**；官方允许但危险 | error |
| `udpPacketSize` 默认值 | NFrp 65507 / 官方 1500，跨端混用会截断 | info |
| `[[proxies]]` 重名 | includes 合并后 NFrp 直接报错 | error |
| 官方二进制 + NFrp 专有字段（`nlink`、`vnet`） | 官方会 `unknown field` 直接退出（实测） | error |
| `token` 为空且显式绑外网 | NFrp 拒绝启动 | error |

校验入口：
- 语义：`nfrp_common::config::{ServerConfig, ClientConfig}` 反序列化（有 schema 级别的严格性）。
- 语法且**针对目标二进制**：调用 `frps/frpc --check`（NFrp 有 `--check`；官方 frp 0.71 用 `frps verify` / `frpc verify`，需探测可用性）。
- 结果写回 `ValidationReport { ok, errors[], warnings[], hints[] }`，前端逐条展示。

### 4.4 写入流程（本机与远程一致）

```
1. 校验（§4.3）          失败 → 不写
2. 读取旧文件 → 备份到 <file>.bak.<UTC 时间戳>（远程：cp -p，本机：fs::copy）
3. 写临时文件 <file>.tmp.<rand>（权限递归自旧文件；新文件默认 0640）
4. fsync → rename 覆盖（POSIX 原子；Windows 先删除目标占位再 rename 或 ReplaceFile）
5. 校验目标文件内容哈希一致
6. 触发 reload（§3.2），失败且策略为 auto_restart → 重启
7. 记审计（含备份路径、文件 sha256 前后值，不含密钥内容）
```

---

## 5. 四种驱动的边界（明确「不做什么」）

| Driver | 接管进程 | 拥有配置 | 依赖 | 主要用途 |
|---|---|---|---|---|
| BinaryDriver | ✅ | ✅ | 本机文件系统 | 默认；单机部署 |
| AdminApiDriver | ❌ | ❌ | 网络可达 Admin API | 纳管已有实例、只读/半托管 |
| SshBinaryDriver | ✅（远程） | ✅（远程） | russh + sudo | 远程服务器一键部署 |
| EmbeddedDriver | ❌（预留） | ❌ | `nfrp-common` 库 | 后续：进程内嵌入 frps/frpc |

**AdminApiDriver 的诚实约束**：没有配置文件所有权 ⇒ 不能编辑配置、不能改代理的持久定义。
`PUT /api/instances/:id/config` 与 `PATCH .../proxies/:name`（持久化语义）返回
`409 driver_immutable`，前端**灰化并给出原因**，而不是假装成功。

---

## 6. API 设计（详见 `02-api.md`）

- 统一错误信封：`{ "error": { "code": "...", "message": "...", "details": {...} } }`，
  错误码沿用 NFrp v2 风格（`bad_request` / `not_found` / `forbidden` / `conflict` /
  `driver_unsupported` / `upstream_failed` / `internal`）。
- 认证：`Authorization: Bearer <JWT>`；登录返回 access + refresh。
- RBAC：`admin` 全权；`viewer` 仅 GET。SSH 凭据与部署仅 admin。
- 分页：`?page=1&page_size=20`，回 `pagination`。
- 流：SSE（`text/event-stream`），事件 `step` / `log` / `done` / `error`，带 `id` 便于断线续传。
- 部署类接口一律「先建任务 → 返回 task_id → SSE 订阅」，幂等键 `Idempotency-Key`。

`GET /api/instances/:id` 返回的 `capabilities` 是前端**唯一**的驱动差异来源。

---

## 7. 安全模型（要点，完整见 README）

1. **frp 管理密钥不出后端**：浏览器只拿到 `admin_api: { base_url, user, password_set: true }`。
2. **SSH 凭据**：AES-256-GCM（每次加密随机 96-bit nonce，AAD 绑定 `记录id:字段名`），
   主密钥来自 `master.key`（0600）或环境变量；前端永不回显，仅 `has_password/has_private_key`。
3. **主机指纹**：首次连接返回 `untrusted` + SHA256 指纹，必须显式 `trust` 才继续；
   校验用 `russh` 的 `check_server_key` 回调，**不提供关闭开关**。
4. **命令白名单**：SSH 只允许模板化命令（`systemctl`、`install`、`chmod`、`mkdir`、
   `sha256sum`、`mv`、`tar` 等），参数做严格转义；`allow_custom_command` 默认 false。
5. **审计**：登录、配置变更（含 diff 统计）、启停、代理增删、凭据增删、部署与回滚；
   `secret` 类型字段在写入前统一走 `secret::redact`。
6. **前端**：JWT 仅存 `sessionStorage`，不落 localStorage；无任何 frp Admin API 直连。

---

## 8. 前端架构（要点）

- 单一 `tokens.css` 定义主题变量（浅/深两套），组件只引用变量；
- `AcrylicCard` 封装 `backdrop-filter` + `@supports not` ❌ 降级为不透明背景（`#FFFFFF` / `#1B1E23`）；
- 所有页面共用 `InstanceStatusBadge` / `CapabilityChips` / `StateView`，驱动差异只体现在数据；
- ECharts 封装为 `TrafficChart`（双轴流量）与 `ConnChart`（连接数），主题色取自 CSS 变量；
- 部署向导 9 步与后端 `DeployPlan` 一一对应，步骤失败展示 `remediation` 文案。

---

## 9. 里程碑与验收（对应交付顺序）

| # | 里程碑 | 验收方式 |
|---|---|---|
| 1 | 骨架 | `cargo run` + `pnpm dev` → 登录 → 实例列表（空态） |
| 2 | BinaryDriver | 添加 NFrp/官方实例 → 启停 → 读配置 → 状态 → reload |
| 3 | 代理 + 监控 | 代理增删改；流量/连接数图表 |
| 4 | AdminApiDriver + 能力探测 | 同一前端切换驱动，页面不变 |
| 5 | SshBinaryDriver + 向导 | 干净 Linux 主机完成 探测→部署→启动→校验；失败回滚 |
| 6 | 主题与组件 | 浅/深主题、backdrop-filter 降级、1280 栅格对齐 |
| 7 | 审计/RBAC/测试/文档 | `cargo test`、集成测试、systemd 模板、README |
