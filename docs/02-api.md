# API 契约

- Base：`/api`
- 认证：`Authorization: Bearer <access_token>`（登录与健康检查除外）
- 内容：`application/json; charset=utf-8`
- 时间：RFC3339 UTC（`2026-11-03T13:47:04Z`）
- 分页：`?page=1&page_size=20` → `{ items, pagination: { page, page_size, total, total_pages } }`

## 统一错误信封

```json
{ "error": { "code": "driver_unsupported", "message": "该驱动不拥有配置文件", "details": { "driver": "admin_api" } } }
```

| code | HTTP | 含义 |
|---|---|---|
| `bad_request` | 400 | 参数错误 |
| `unauthorized` | 401 | 未登录 / token 过期 |
| `forbidden` | 403 | 角色不足 |
| `not_found` | 404 | 资源不存在 |
| `conflict` | 409 | 状态冲突（如已在运行、配置已被他人修改） |
| `driver_unsupported` | 409 | 当前驱动不支持该操作（前端据此灰化按钮） |
| `upstream_failed` | 502 | frp Admin API / SSH 返回失败 |
| `validation_failed` | 422 | 配置校验不通过（`details.errors[]`） |
| `internal` | 500 | 服务端内部错误 |

---

## 1. 认证

### `POST /api/auth/login`
```json
{ "username": "admin", "password": "..." }
```
→ `{ "access_token": "...", "refresh_token": "...", "expires_in": 7200, "user": { "id": "...", "username": "admin", "role": "admin" } }`

### `POST /api/auth/refresh` → 同上（不含 refresh_token 轮换时的旧值）
### `POST /api/auth/logout` → 204
### `GET  /api/auth/me` → `{ id, username, role, created_at }`
### `GET  /api/health` → `{ "status": "ok", "version": "0.1.0" }`（免鉴权）

---

## 2. 实例

### `GET /api/instances`
→ `{ items: InstanceSummary[], pagination }`

```ts
interface InstanceSummary {
  id: string
  name: string
  role: 'server' | 'client'
  driver: 'binary' | 'admin_api' | 'ssh_binary' | 'embedded'
  flavor: 'nfrp' | 'official_frp'
  connection_kind: 'local' | 'ssh'
  host: string            // local → 主机名；ssh → host:port
  remote_ident: string    // 远程标识（SSH 主机 / 本机路径）
  binary_version: string | null
  deployment_status: 'not_deployed' | 'deployed' | 'failed' | 'rolled_back'
  service_mode: 'process' | 'systemd' | 'nohup'
  enabled: boolean
  live: { state: 'running' | 'stopped' | 'unknown' | 'error', pid: number | null, since: string | null, healthy: boolean }
  capabilities: Capabilities
  last_deployed_at: string | null
  updated_at: string
}
```

### `POST /api/instances`
```ts
interface InstanceCreate {
  name: string
  role: 'server' | 'client'
  driver: DriverKind
  flavor: BinaryFlavor
  connection: { kind: 'local', host?: string } | { kind: 'ssh', host: string, port: number, username: string, auth_kind: 'password'|'private_key'|'private_key_passphrase'|'agent', credential_id?: string, sudo: 'none'|'sudo'|'sudo_n'|'custom', sudo_cmd?: string }
  paths: { binary: string, config: string, log: string, work_dir: string }
  service_mode: 'process' | 'systemd' | 'nohup'
  service_unit?: string
  admin_api: { base_url: string, user: string, password_ref?: string }   // password_ref = 已存凭据 id
}
```
→ `201 InstanceDetail`

### `GET /api/instances/:id` → `InstanceDetail`

```ts
interface InstanceDetail extends InstanceSummary {
  paths: { binary: string, config: string, log: string, work_dir: string }
  admin_api: { base_url: string, user: string, password_set: boolean, password_masked: string } // 永不回显明文
  connection: { kind: string, host?: string, port?: number, username?: string, auth_kind?: string, credential_id?: string, sudo?: string, host_key_fingerprint?: string }
  capabilities: Capabilities
  deployment: DeploymentInfo
}
```

### `PATCH /api/instances/:id` → `InstanceDetail`（部分字段）
### `DELETE /api/instances/:id` → 204（`?force=true` 时即使运行中也删除）

### 生命周期
- `POST /api/instances/:id/start` → `{ state, pid }`
- `POST /api/instances/:id/stop` → `{ state }`（body 可带 `{ force: boolean }`）
- `POST /api/instances/:id/restart` → `{ state, pid }`
- `GET  /api/instances/:id/status` → `InstanceStatus`

```ts
interface InstanceStatus {
  state: 'running' | 'stopped' | 'unknown' | 'error'
  healthy: boolean
  pid: number | null
  uptime_secs: number | null
  binary_version: string | null
  api_reachable: boolean
  last_error: string | null
  detail: { source: 'process' | 'admin_api' | 'ssh', notes: string[] }
  counters?: { clients: number, proxies: number, conns: number, bytes_in: number, bytes_out: number }
}
```

---

## 3. 配置

- `GET  /api/instances/:id/config` → `ConfigDoc`
- `PUT  /api/instances/:id/config` → `{ apply: boolean }` → `ConfigWriteResult`
- `POST /api/instances/:id/config/validate` → `ValidationReport`
- `GET  /api/instances/:id/config/backups` → `BackupItem[]`
- `POST /api/instances/:id/config/restore` `{ backup_id }` → `ConfigWriteResult`
- `POST /api/instances/:id/reload` → `ReloadOutcome`

```ts
interface ConfigDoc {
  content: string                 // TOML 原文
  path: string
  sha256: string
  parsed: { ok: boolean, role: 'server'|'client', proxies: ProxySpec[], visitors: ProxySpec[] }
  diagnostics: Diagnostic[]       // 兼容性提示
  editable: boolean               // admin_api 驱动为 false
  editable_reason: string | null
}
interface Diagnostic { level: 'error'|'warning'|'info', code: string, message: string, hint?: string, line?: number }
interface ValidationReport { ok: boolean, errors: Diagnostic[], warnings: Diagnostic[], hints: Diagnostic[], checked_by: 'schema'|'binary_check'|'both' }
interface ConfigWriteResult { applied: ReloadOutcome, backup: { id: string, path: string, created_at: string } | null, warnings: Diagnostic[] }
type ReloadOutcome = 'hot' | 'restart_required' | 'unsupported' | 'restarted'
```

---

## 4. 代理 / Visitor

- `GET    /api/instances/:id/proxies` → `ProxyInfo[]`
- `POST   /api/instances/:id/proxies` → `ProxyOpResult`
- `PATCH  /api/instances/:id/proxies/:name` → `ProxyOpResult`
- `DELETE /api/instances/:id/proxies/:name` → `ProxyOpResult`
- `GET    /api/instances/:id/visitors` → `ProxyInfo[]`（stcp/xtcp/sudp）

```ts
interface ProxyInfo {
  name: string
  type: 'tcp'|'udp'|'http'|'https'|'tcpmux'|'stcp'|'xtcp'|'sudp'
  local_addr: string
  remote_port: number | null
  custom_domains: string[]
  subdomain: string
  status: 'running' | 'waiting' | 'start_error' | 'offline' | 'unknown'
  error: string | null
  cur_conns: number
  traffic_in: number | null
  traffic_out: number | null
  last_active_at: string | null
  last_start_at: string | null
  source: 'config' | 'dynamic'      // 配置文件里定义的 / 面板或 API 动态加的
  managed_by: 'panel' | 'server_push' | 'config_file' | 'external'
  client_id: string | null          // frps 视角
  run_id: string | null
}
interface ProxyOpResult {
  applied_via: 'admin_api' | 'config_reload' | 'config_restart'
  reload: ReloadOutcome
  proxy: ProxyInfo
  warnings: Diagnostic[]
}
```

---

## 5. 日志

- `GET /api/instances/:id/logs?offset=0&limit=200&q=&level=&tail=true`
- `GET /api/instances/:id/logs/download`
- `GET /api/instances/:id/logs/stream`（SSE）

```ts
interface LogPage { lines: LogLine[], total: number | null, next_offset: number | null, source: 'file'|'remote'|'journal', path: string }
interface LogLine { seq: number, ts: string | null, level: 'TRACE'|'DEBUG'|'INFO'|'WARN'|'ERROR'|null, raw: string }
```

---

## 6. 指标

- `GET /api/instances/:id/metrics?range=1h|6h|24h|7d` → `MetricsResponse`
- `GET /api/instances/:id/metrics/live` → `MetricsSnapshot`
- Prometheus 抓取配置在实例级：`prometheus: { enabled, url, user_ref? }`

```ts
interface MetricsSnapshot {
  ts: string
  api_reachable: boolean
  counters: { bytes_in: number, bytes_out: number, conns: number, clients: number, proxies: number, requests: number | null }
  availability: 'full' | 'aggregated' | 'none'   // 官方 frps 无流量字节 → aggregated
  source: 'admin_api' | 'prometheus' | 'both'
}
interface MetricsResponse { series: MetricsSeries[], availability: string, notes: string[] }
interface MetricsSeries { name: 'bytes_in'|'bytes_out'|'conns'|'clients'|'proxies', points: [number, number][] }
```

---

## 7. 部署 / SSH

- `POST /api/instances/:id/deploy/preflight` → `PreflightReport`
- `POST /api/instances/:id/deploy` → `{ task_id }`（`Idempotency-Key` 可选）
- `POST /api/instances/:id/deploy/rollback` → `{ task_id }`
- `GET  /api/instances/:id/deploy/status` → `DeployTask | null`
- `GET  /api/deploy/tasks/:task_id/stream`（SSE）

```ts
interface PreflightItem { key: string, label: string, status: 'pass'|'warn'|'fail'|'skip', message: string, remediation?: string, data?: Record<string, unknown> }
interface PreflightReport { ok: boolean, items: PreflightItem[], environment: RemoteEnvironment }
interface RemoteEnvironment {
  os: string, kernel: string, arch: string, hostname: string
  systemd: boolean, sudo: 'available'|'passwordless'|'unavailable', disk_free_mb: number
  existing: { frps: string|null, frpc: string|null, config: string|null }
  listening_ports: number[]
}
type DeployStepState = 'pending'|'running'|'done'|'failed'|'skipped'
interface DeployStep { key: string, title: string, state: DeployStepState, started_at?: string, finished_at?: string, message?: string, remediation?: string }
interface DeployTask {
  id: string, instance_id: string, kind: 'deploy'|'rollback',
  state: 'pending'|'running'|'succeeded'|'failed'|'rolled_back',
  steps: DeployStep[], started_at: string, finished_at?: string, error?: string, rollback_of?: string
}
```

SSE 帧：
```
event: step
id: 3
data: {"key":"upload_binary","state":"running"}

event: log
id: 3-17
data: {"ts":"...","stream":"stdout","line":"==> 上传 frps 到 /usr/local/bin/frps (5.1 MB)"}

event: done
data: {"task_id":"...","state":"succeeded"}
```

---

## 8. SSH 相关

- `POST /api/ssh/test-connection` `{ host, port, username, auth_kind, credential_id?, credential_inline? }` → `SshTestResult`
- `GET  /api/ssh/host-keys` → `HostKeyRecord[]`
- `POST /api/ssh/host-keys/trust` `{ host, port, fingerprint, algorithm }` → `HostKeyRecord`
- `DELETE /api/ssh/host-keys/:id` → 204
- `POST /api/ssh/probe` `{ ...connection, paths }` → `RemoteEnvironment`（向导第 3 步）

```ts
interface SshTestResult {
  stage: 'connect'|'auth'|'host_key'|'exec'|'done'
  ok: boolean
  message: string
  host_key?: { fingerprint: string, algorithm: string, trusted: boolean }
  duration_ms: number
  remediation?: string
}
```

---

## 9. 凭据

- `GET /api/credentials` → `CredentialSummary[]`（**永不返回明文**）
- `POST /api/credentials` → `CredentialSummary`
- `DELETE /api/credentials/:id` → 204

```ts
interface CredentialSummary {
  id: string, name: string, kind: 'ssh_password'|'ssh_private_key'|'admin_password'|'token',
  username: string | null, has_secret: true, secret_preview: '••••••••',
  algorithm_hint: string | null,   // 私钥类型：ssh-ed25519 / RSA-4096
  created_at: string, last_used_at: string | null, used_by: string[]
}
```

---

## 10. 审计日志

- `GET /api/audit-logs?page=&page_size=&action=&result=&actor=&instance_id=&from=&to=`
- `GET /api/audit-logs/export`（CSV）

```ts
interface AuditLog {
  id: string, ts: string, actor: string, actor_ip: string,
  action: string,             // auth.login / instance.start / config.write / proxy.create / deploy.run / credential.create ...
  target: string,             // 实例名 / 凭据名 / 主机
  result: 'success'|'failure'|'denied',
  duration_ms: number,
  detail: Record<string, unknown>   // 已脱敏
}
```

---

## 11. 设置

- `GET /api/settings` → `Settings`
- `PATCH /api/settings` → `Settings`

```ts
interface Settings {
  defaults: { driver: DriverKind, flavor: BinaryFlavor, binary_dir: string, server_binary: string, client_binary: string, config_dir: string, log_dir: string, work_dir: string }
  frp: { admin_timeout_ms: number, status_poll_ms: number, metrics_history_days: number, auto_restart_on_save: boolean }
  ssh: { connect_timeout_ms: number, command_timeout_ms: number, retry: number, default_port: number, default_user: string, default_install_dir: string, default_config_dir: string, default_log_dir: string, default_service: 'systemd'|'nohup', allow_custom_command: boolean }
  security: { access_token_ttl_minutes: number, audit_retention_days: number, master_key_fingerprint: string }
  catalog: { entries: CatalogEntry[] }   // 内置版本清单
  theme: { default: 'light'|'dark' }
}
```

---

## 12. 能力对象（前端唯一驱动差异来源）

```ts
interface Capabilities {
  flavor: 'nfrp' | 'official_frp'
  flavor_version: string | null
  api_kind: 'nfrp' | 'official' | 'unknown' | 'none'
  managed_locally: boolean
  remote: boolean
  can_start_stop: boolean
  can_reload: boolean
  can_edit_config: boolean
  can_dynamic_proxy: boolean
  can_manage_visitors: boolean
  has_v2_api: boolean
  has_healthz: boolean
  prometheus: 'off' | 'enabled' | 'unknown'
  metrics_detail: 'full' | 'aggregated' | 'none'
  auth_guard_header: boolean
  notes: { level: 'info'|'warning'|'error', text: string }[]
  probed_at: string
}
```
