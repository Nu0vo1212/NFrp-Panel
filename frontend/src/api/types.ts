/**
 * 与后端 DTO 一一对应的类型（来源：`docs/02-api.md`）。
 *
 * 约定：后端序列化为 snake_case；这里保持一致，避免运行时再做一次转换。
 */

export type DriverKind = 'binary' | 'admin_api' | 'ssh_binary' | 'embedded'
export type BinaryFlavor = 'nfrp' | 'official_frp'
export type InstanceRole = 'server' | 'client'
export type ServiceMode = 'process' | 'systemd' | 'nohup'
export type DeploymentStatus = 'not_deployed' | 'deployed' | 'failed' | 'rolled_back'
export type LiveState = 'running' | 'stopped' | 'unknown' | 'error'
export type ApiKind = 'nfrp' | 'official' | 'unknown' | 'none'
export type ProxyStatus =
  | 'running'
  | 'waiting'
  | 'start_error'
  | 'offline'
  | 'unknown'
export type AuthKind = 'password' | 'private_key' | 'private_key_passphrase' | 'agent'
export type SudoMode = 'none' | 'sudo' | 'sudo_n' | 'custom'
export type ReloadOutcome = 'hot' | 'restart_required' | 'unsupported' | 'restarted'
export type UserRole = 'admin' | 'viewer'

// ---------------------------------------------------------------------------
// 通用
// ---------------------------------------------------------------------------

export interface Pagination {
  page: number
  page_size: number
  total: number
  total_pages: number
}

export interface ListResponse<T> {
  items: T[]
  pagination: Pagination
}

export interface ApiErrorBody {
  error: {
    code: string
    message: string
    details?: unknown
  }
}

/** 后端错误码（前端据此决定展示方式）。 */
export type ErrorCode =
  | 'bad_request'
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'conflict'
  | 'driver_unsupported'
  | 'upstream_failed'
  | 'validation_failed'
  | 'internal'

// ---------------------------------------------------------------------------
// 认证
// ---------------------------------------------------------------------------

export interface UserInfo {
  id: string
  username: string
  role: UserRole
}

export interface Permissions {
  can_write: boolean
  can_manage_ssh: boolean
  can_deploy: boolean
  can_manage_credentials: boolean
  can_read_audit: boolean
}

export interface MeResponse extends UserInfo {
  permissions: Permissions
}

export interface TokenPair {
  access_token: string
  refresh_token: string
  expires_in: number
  user: UserInfo
}

// ---------------------------------------------------------------------------
// 能力
// ---------------------------------------------------------------------------

export interface CapabilityNote {
  level: 'info' | 'warning' | 'error'
  text: string
}

export interface Capabilities {
  flavor: BinaryFlavor
  flavor_version: string | null
  api_kind: ApiKind
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
  notes: CapabilityNote[]
  probed_at: string | null
}

// ---------------------------------------------------------------------------
// 实例
// ---------------------------------------------------------------------------

export interface LiveStatus {
  state: LiveState
  pid: number | null
  healthy: boolean
  since: string | null
  error: string | null
}

export interface InstancePaths {
  binary: string
  config: string
  log: string
  work_dir: string
}

export interface InstanceSummary {
  id: string
  name: string
  role: InstanceRole
  driver: DriverKind
  driver_label: string
  flavor: BinaryFlavor
  flavor_label: string
  connection_kind: 'local' | 'ssh'
  host: string
  remote_ident: string
  binary_version: string | null
  deployment_status: DeploymentStatus
  service_mode: ServiceMode
  enabled: boolean
  live: LiveStatus
  capabilities: Capabilities
  last_deployed_at: string | null
  updated_at: string
}

export interface AdminApiView {
  base_url: string
  user: string
  enabled: boolean
  password_set: boolean
  password_masked: string
  prometheus_url: string | null
}

export interface ConnectionView {
  kind: 'local' | 'ssh'
  host: string | null
  port: number | null
  username: string | null
  auth_kind: AuthKind | null
  credential_id: string | null
  sudo: SudoMode | null
  host_key_fingerprint: string | null
  host_key_trusted: boolean
}

export interface DeploymentInfo {
  status: DeploymentStatus
  binary_version: string | null
  last_deployed_at: string | null
  service_mode: ServiceMode
  service_unit: string | null
  last_task: Record<string, unknown> | null
}

export interface InstanceDetail extends InstanceSummary {
  paths: InstancePaths
  admin_api: AdminApiView
  connection: ConnectionView
  deployment: DeploymentInfo
  tags: string[]
}

export interface InstanceCreateRequest {
  name: string
  role: InstanceRole
  driver: DriverKind
  flavor: BinaryFlavor
  tags?: string[]
  connection?:
    | { kind: 'local' }
    | {
        kind: 'ssh'
        host: string
        port: number
        username: string
        auth_kind: AuthKind
        credential_id?: string | null
        sudo?: SudoMode
        sudo_cmd?: string | null
      }
  paths: InstancePaths
  service_mode?: ServiceMode
  service_unit?: string | null
  admin_api?: {
    base_url: string
    user: string
    password?: string
    enabled?: boolean
    prometheus_url?: string | null
  }
}

export type InstanceUpdateRequest = Partial<InstanceCreateRequest> & {
  enabled?: boolean
}

export interface InstanceCounters {
  bytes_in: number
  bytes_out: number
  conns: number
  clients: number
  proxies: number
  requests?: number
}

export interface InstanceStatus {
  state: LiveState
  healthy: boolean
  pid: number | null
  uptime_secs: number | null
  binary_version: string | null
  api_reachable: boolean
  last_error: string | null
  detail: { source: string; notes: string[] }
  counters?: InstanceCounters
}

export interface HealthCheck {
  name: string
  ok: boolean
  detail: string
}

export interface HealthReport {
  healthy: boolean
  message: string
  checks: HealthCheck[]
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

export interface Diagnostic {
  level: 'error' | 'warning' | 'info'
  code: string
  message: string
  hint?: string
  line?: number
}

export interface ProxySpec {
  name: string
  type: string
  local_addr?: string
  local_ip?: string | null
  local_port?: number | null
  remote_port?: number | null
  custom_domains?: string[]
  subdomain?: string
  secret_key?: string | null
  allow_users?: string[]
  group?: string | null
  use_encryption?: boolean
  use_compression?: boolean
}

export interface ParsedConfig {
  ok: boolean
  role: InstanceRole
  proxies: ProxySpec[]
  visitors: ProxySpec[]
}

export interface ConfigDoc {
  content: string
  path: string
  sha256: string
  parsed: ParsedConfig
  diagnostics: Diagnostic[]
  editable: boolean
  editable_reason: string | null
}

export interface ValidationReport {
  ok: boolean
  errors: Diagnostic[]
  warnings: Diagnostic[]
  hints: Diagnostic[]
  checked_by: string
}

export interface BackupInfo {
  id: string
  path: string
  created_at: string
  size: number
}

export interface ConfigWriteResult {
  applied: ReloadOutcome
  backup: BackupInfo | null
  warnings: Diagnostic[]
}

// ---------------------------------------------------------------------------
// 代理
// ---------------------------------------------------------------------------

export interface ProxyInfo {
  name: string
  type: string
  local_addr: string
  remote_port: number | null
  custom_domains: string[]
  subdomain: string
  status: ProxyStatus
  error: string | null
  cur_conns: number
  traffic_in: number | null
  traffic_out: number | null
  last_active_at: string | null
  last_start_at: string | null
  source: 'config' | 'dynamic'
  managed_by: 'panel' | 'server_push' | 'config_file' | 'external'
  client_id: string | null
  run_id: string | null
}

export interface ProxyOpResult {
  applied_via: 'admin_api' | 'config_reload' | 'config_restart'
  reload: ReloadOutcome
  proxy: ProxyInfo
  warnings: Diagnostic[]
}

export interface PreflightItem {
  key: string
  label: string
  status: 'pass' | 'warn' | 'fail' | 'skip'
  message: string
  remediation?: string
}

export interface ProxyPreflightResult {
  ok: boolean
  items: PreflightItem[]
}

// ---------------------------------------------------------------------------
// 日志
// ---------------------------------------------------------------------------

export interface LogLine {
  seq: number
  ts: string | null
  level: string | null
  raw: string
}

export interface LogPage {
  lines: LogLine[]
  total: number | null
  next_offset: number | null
  source: string
  path: string
}

// ---------------------------------------------------------------------------
// 监控
// ---------------------------------------------------------------------------

export interface MetricsSnapshot {
  ts: string
  api_reachable: boolean
  counters: InstanceCounters
  availability: 'full' | 'aggregated' | 'none'
  source: string
}

export interface MetricsSeries {
  name: 'bytes_in' | 'bytes_out' | 'conns' | 'clients' | 'proxies'
  points: [number, number][]
}

export interface MetricsResponse {
  series: MetricsSeries[]
  availability: 'full' | 'aggregated' | 'none'
  notes: string[]
  range_secs: number
}

// ---------------------------------------------------------------------------
// 部署
// ---------------------------------------------------------------------------

export type DeployStepState = 'pending' | 'running' | 'done' | 'failed' | 'skipped'
export type DeployState =
  | 'pending'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'rolled_back'

export interface DeployStep {
  key: string
  title: string
  state: DeployStepState
  started_at?: string
  finished_at?: string
  message?: string
  remediation?: string
}

export interface DeployTask {
  id: string
  instance_id: string
  kind: string
  state: DeployState
  steps: DeployStep[]
  started_at: string
  finished_at?: string
  error?: string
  rollback_of?: string
  logs?: string[]
}

export interface RemoteEnvironment {
  os: string
  kernel: string
  arch: string
  hostname: string
  systemd: boolean
  sudo: 'available' | 'passwordless' | 'unavailable' | 'password'
  current_user: string
  disk_free_mb: number
  existing: { frps: string | null; frpc: string | null; config: string | null }
  listening_ports: number[]
  notes: string[]
}

export interface DeployPreflightReport {
  ok: boolean
  items: PreflightItem[]
  environment: RemoteEnvironment
}

export interface TaskCreated {
  task_id: string
  stream: string
}

// ---------------------------------------------------------------------------
// SSH
// ---------------------------------------------------------------------------

export interface HostKeyInfo {
  algorithm: string
  fingerprint: string
  trusted: boolean
}

export interface SshTestResult {
  stage: 'connect' | 'host_key' | 'auth' | 'exec' | 'done'
  ok: boolean
  message: string
  duration_ms: number
  host_key?: HostKeyInfo
  remediation?: string
}

export interface HostKeyRecord {
  id: string
  host: string
  port: number
  algorithm: string
  fingerprint: string
  trusted_at: string
}

// ---------------------------------------------------------------------------
// 凭据
// ---------------------------------------------------------------------------

export interface CredentialSummary {
  id: string
  name: string
  kind: 'ssh_password' | 'ssh_private_key' | 'admin_password' | 'token'
  username: string | null
  has_secret: boolean
  secret_preview: string
  algorithm_hint: string | null
  created_at: string
  last_used_at: string | null
  used_by: string[]
}

// ---------------------------------------------------------------------------
// 审计
// ---------------------------------------------------------------------------

export interface AuditLog {
  id: string
  ts: string
  actor: string
  actor_ip: string
  action: string
  target: string
  result: 'success' | 'failure' | 'denied'
  duration_ms: number
  detail: Record<string, unknown>
}

// ---------------------------------------------------------------------------
// 设置
// ---------------------------------------------------------------------------

export interface Settings {
  defaults: {
    driver: DriverKind
    flavor: BinaryFlavor
    binary_dir: string
    server_binary: string
    client_binary: string
    config_dir: string
    log_dir: string
    work_dir: string
  }
  frp: {
    admin_timeout_ms: number
    status_poll_ms: number
    metrics_history_days: number
    auto_restart_on_save: boolean
  }
  ssh: {
    connect_timeout_ms: number
    command_timeout_ms: number
    retry: number
    default_port: number
    default_user: string
    default_install_dir: string
    default_config_dir: string
    default_log_dir: string
    default_service: 'systemd' | 'nohup'
    default_sudo: SudoMode
    allow_custom_command: boolean
  }
  security: {
    access_token_ttl_minutes: number
    audit_retention_days: number
    master_key_fingerprint: string
    audit_enabled: boolean
  }
  theme: 'light' | 'dark'
}
