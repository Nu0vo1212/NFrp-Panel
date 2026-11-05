/** 所有后端端点的类型化封装。页面只调这里，不直接碰 axios。 */

import { http } from './client'
import type {
  AuditLog,
  BackupInfo,
  ConfigDoc,
  ConfigWriteResult,
  CredentialSummary,
  DeployPreflightReport,
  DeployState,
  DeployTask,
  HealthReport,
  HostKeyRecord,
  InstanceCreateRequest,
  InstanceDetail,
  InstanceStatus,
  InstanceSummary,
  InstanceUpdateRequest,
  ListResponse,
  LogPage,
  MeResponse,
  MetricsResponse,
  MetricsSnapshot,
  PreflightItem,
  ProxyInfo,
  ProxyOpResult,
  ProxyPreflightResult,
  ProxySpec,
  RemoteEnvironment,
  Settings,
  SshTestResult,
  TaskCreated,
  TokenPair,
  ValidationReport,
} from './types'

// ---- 认证 ----
export const authApi = {
  login: (username: string, password: string) =>
    http
      .post<TokenPair>('/auth/login', { username, password })
      .then((r) => r.data),
  me: () => http.get<MeResponse>('/auth/me').then((r) => r.data),
  logout: () => http.post('/auth/logout').then(() => undefined),
  changePassword: (current_password: string, new_password: string) =>
    http
      .post('/auth/password', { current_password, new_password })
      .then(() => undefined),
  health: () => http.get<{ status: string; version: string }>('/health').then((r) => r.data),
}

// ---- 实例 ----
export const instanceApi = {
  list: (params?: { q?: string; driver?: string; role?: string; page?: number; page_size?: number }) =>
    http.get<ListResponse<InstanceSummary>>('/instances', { params }).then((r) => r.data),

  get: (id: string) => http.get<InstanceDetail>(`/instances/${id}`).then((r) => r.data),

  create: (body: InstanceCreateRequest) =>
    http.post<InstanceDetail>('/instances', body).then((r) => r.data),

  update: (id: string, body: InstanceUpdateRequest) =>
    http.patch<InstanceDetail>(`/instances/${id}`, body).then((r) => r.data),

  remove: (id: string, force = false) =>
    http.delete(`/instances/${id}`, { params: { force } }).then(() => undefined),

  start: (id: string) => http.post<InstanceStatus>(`/instances/${id}/start`).then((r) => r.data),
  stop: (id: string) => http.post<InstanceStatus>(`/instances/${id}/stop`).then((r) => r.data),
  restart: (id: string) =>
    http.post<InstanceStatus>(`/instances/${id}/restart`).then((r) => r.data),
  status: (id: string) => http.get<InstanceStatus>(`/instances/${id}/status`).then((r) => r.data),
  health: (id: string) => http.get<HealthReport>(`/instances/${id}/health`).then((r) => r.data),
  reload: (id: string) =>
    http.post<{ outcome: string }>(`/instances/${id}/reload`).then((r) => r.data),
  capabilities: (id: string) =>
    http.get(`/instances/${id}/capabilities`).then((r) => r.data),
  diagnose: (id: string) =>
    http.get(`/instances/${id}/capabilities/diagnose`).then((r) => r.data),
}

// ---- 配置 ----
export const configApi = {
  get: (id: string) => http.get<ConfigDoc>(`/instances/${id}/config`).then((r) => r.data),
  validate: (id: string, content: string) =>
    http.post<ValidationReport>(`/instances/${id}/config/validate`, { content }).then((r) => r.data),
  save: (id: string, content: string, apply = true, auto_restart?: boolean) =>
    http
      .put<ConfigWriteResult>(`/instances/${id}/config`, { content, apply, auto_restart })
      .then((r) => r.data),
  backups: (id: string) =>
    http.get<BackupInfo[]>(`/instances/${id}/config/backups`).then((r) => r.data),
  restore: (id: string, backup_id: string) =>
    http
      .post<ConfigWriteResult>(`/instances/${id}/config/restore`, { backup_id })
      .then((r) => r.data),
  template: (role: 'server' | 'client', flavor: 'nfrp' | 'official_frp') =>
    http
      .post<{ content: string; notes: string[] }>('/config/template', { role, flavor })
      .then((r) => r.data),
}

// ---- 代理 ----
export const proxyApi = {
  list: (id: string) => http.get<ProxyInfo[]>(`/instances/${id}/proxies`).then((r) => r.data),
  create: (id: string, spec: ProxySpec) =>
    http.post<ProxyOpResult>(`/instances/${id}/proxies`, spec).then((r) => r.data),
  update: (id: string, name: string, spec: ProxySpec) =>
    http.patch<ProxyOpResult>(`/instances/${id}/proxies/${name}`, spec).then((r) => r.data),
  remove: (id: string, name: string) =>
    http.delete<ProxyOpResult>(`/instances/${id}/proxies/${name}`).then((r) => r.data),
  preflight: (id: string, spec: ProxySpec) =>
    http
      .post<ProxyPreflightResult>(`/instances/${id}/proxies/preflight`, { spec })
      .then((r) => r.data),
  visitors: (id: string) =>
    http.get<ProxyInfo[]>(`/instances/${id}/visitors`).then((r) => r.data),
}

// ---- 日志 ----
export const logApi = {
  list: (id: string, params: { limit?: number; offset?: number; q?: string; level?: string }) =>
    http.get<LogPage>(`/instances/${id}/logs`, { params }).then((r) => r.data),
  downloadUrl: (id: string, limit = 2000) =>
    `/api/instances/${id}/logs/download?limit=${limit}`,
}

// ---- 监控 ----
export const metricsApi = {
  history: (id: string, range = '1h') =>
    http.get<MetricsResponse>(`/instances/${id}/metrics`, { params: { range } }).then((r) => r.data),
  live: (id: string) =>
    http.get<MetricsSnapshot>(`/instances/${id}/metrics/live`).then((r) => r.data),
}

// ---- 部署 ----
export const deployApi = {
  preflight: (id: string, body?: { binary_b64?: string; arch_expected?: string; config?: string }) =>
    http
      .post<DeployPreflightReport>(`/instances/${id}/deploy/preflight`, body ?? {})
      .then((r) => r.data),
  run: (id: string, body: { binary_b64?: string; binary_name?: string; config?: string; unit_name?: string }) =>
    http.post<TaskCreated>(`/instances/${id}/deploy`, body).then((r) => r.data),
  status: (id: string) =>
    http.get<DeployTask | null>(`/instances/${id}/deploy/status`).then((r) => r.data),
  rollback: (id: string, backup_id?: string) =>
    http.post(`/instances/${id}/deploy/rollback`, { backup_id }).then((r) => r.data),
  history: (id: string) => http.get(`/instances/${id}/deploy/history`).then((r) => r.data),
  fetchBinary: (url: string, sha256?: string) =>
    http
      .post<{ binary_b64: string; size: number; sha256: string; arch: string | null; notes: string[] }>(
        '/deploy/binary/fetch',
        { url, sha256 },
      )
      .then((r) => r.data),
  /** 订阅部署事件流（SSE）。返回取消函数。 */
  stream(
    taskId: string,
    handlers: {
      onStep?: (data: { key: string; state: string; message?: string; remediation?: string }) => void
      onLog?: (data: { ts: string; stream: string; line: string }) => void
      onDone?: (data: { state: string; error?: string }) => void
      onError?: (e: unknown) => void
    },
  ): () => void {
    const ctrl = new AbortController()
    const token = sessionStorage.getItem('frppanel.token') ?? ''
    fetch(`/api/deploy/tasks/${taskId}/stream`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      signal: ctrl.signal,
    })
      .then(async (resp) => {
        if (!resp.ok || !resp.body) throw new Error(`SSE 连接失败（HTTP ${resp.status}）`)
        const reader = resp.body.getReader()
        const decoder = new TextDecoder()
        let buffer = ''
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })
          // SSE 帧以空行分隔
          const frames = buffer.split('\n\n')
          buffer = frames.pop() ?? ''
          for (const frame of frames) {
            let event = 'message'
            const dataLines: string[] = []
            for (const line of frame.split('\n')) {
              if (line.startsWith('event:')) event = line.slice(6).trim()
              else if (line.startsWith('data:')) dataLines.push(line.slice(5).trim())
            }
            if (!dataLines.length) continue
            let payload: any
            try {
              payload = JSON.parse(dataLines.join('\n'))
            } catch {
              continue
            }
            if (event === 'step') handlers.onStep?.(payload)
            else if (event === 'log') handlers.onLog?.(payload)
            else if (event === 'done') handlers.onDone?.(payload)
          }
        }
      })
      .catch((e) => {
        if ((e as Error).name !== 'AbortError') handlers.onError?.(e)
      })
    return () => ctrl.abort()
  },
}

// ---- SSH ----
export const sshApi = {
  test: (body: {
    host: string
    port: number
    username: string
    auth_kind: string
    credential_id?: string
    secret_inline?: string
    passphrase_inline?: string
  }) => http.post<SshTestResult>('/ssh/test-connection', body).then((r) => r.data),

  probe: (body: {
    host: string
    port: number
    username: string
    auth_kind: string
    credential_id?: string
    secret_inline?: string
    config_dir?: string
    candidate_ports?: number[]
  }) => http.post<RemoteEnvironment>('/ssh/probe', body).then((r) => r.data),

  hostKeys: () => http.get<HostKeyRecord[]>('/ssh/host-keys').then((r) => r.data),
  trustHostKey: (body: {
    host: string
    port: number
    algorithm: string
    fingerprint: string
    public_key: string
  }) => http.post<HostKeyRecord>('/ssh/host-keys/trust', body).then((r) => r.data),
  deleteHostKey: (id: string) => http.delete(`/ssh/host-keys/${id}`).then(() => undefined),
}

// ---- 凭据 ----
export const credentialApi = {
  list: () => http.get<CredentialSummary[]>('/credentials').then((r) => r.data),
  create: (body: {
    name: string
    kind: 'ssh_password' | 'ssh_private_key'
    username?: string
    secret: string
    passphrase?: string
  }) => http.post<CredentialSummary>('/credentials', body).then((r) => r.data),
  remove: (id: string) => http.delete(`/credentials/${id}`).then(() => undefined),
}

// ---- 审计 ----
export const auditApi = {
  list: (params?: {
    page?: number
    page_size?: number
    action?: string
    result?: string
    actor?: string
    q?: string
    from?: string
    to?: string
  }) => http.get<ListResponse<AuditLog>>('/audit-logs', { params }).then((r) => r.data),
  exportUrl: '/api/audit-logs/export',
}

// ---- 设置 ----
export const settingsApi = {
  get: () => http.get<Settings>('/settings').then((r) => r.data),
  update: (body: Partial<Settings> & { theme?: 'light' | 'dark' }) =>
    http.patch<Settings>('/settings', body).then((r) => r.data),
  catalog: () =>
    http
      .get<{ entries: unknown[]; note: string }>('/settings/catalog')
      .then((r) => r.data),
}

export type { PreflightItem, DeployState }
