/**
 * Axios 实例：统一鉴权注入、错误归一、401 自动刷新。
 *
 * 安全要点（对应 README「安全注意」）：
 * - token 只放 `sessionStorage`，不落 localStorage；
 * - 面板只与本后端通信，前端**从不**直接访问 frp Admin API。
 */

import axios, {
  type AxiosInstance,
  type AxiosRequestConfig,
  type AxiosError,
} from 'axios'
import type { ApiErrorBody, ErrorCode, TokenPair } from './types'

const TOKEN_KEY = 'frppanel.token'
const REFRESH_KEY = 'frppanel.refresh'

export class ApiError extends Error {
  code: ErrorCode | string
  status: number
  details?: unknown

  constructor(code: string, message: string, status: number, details?: unknown) {
    super(message)
    this.name = 'ApiError'
    this.code = code
    this.status = status
    this.details = details
  }

  /** 驱动不支持：UI 应灰化入口而不是弹错误。 */
  get isUnsupported() {
    return this.code === 'driver_unsupported'
  }
  get isUnauthorized() {
    return this.code === 'unauthorized' || this.status === 401
  }
  get isValidation() {
    return this.code === 'validation_failed'
  }
  get isUpstream() {
    return this.code === 'upstream_failed'
  }
  get isForbidden() {
    return this.code === 'forbidden' || this.status === 403
  }
}

export const tokens = {
  get access() {
    return sessionStorage.getItem(TOKEN_KEY) ?? ''
  },
  get refresh() {
    return sessionStorage.getItem(REFRESH_KEY) ?? ''
  },
  set(pair: Pick<TokenPair, 'access_token' | 'refresh_token'>) {
    sessionStorage.setItem(TOKEN_KEY, pair.access_token)
    sessionStorage.setItem(REFRESH_KEY, pair.refresh_token)
  },
  clear() {
    sessionStorage.removeItem(TOKEN_KEY)
    sessionStorage.removeItem(REFRESH_KEY)
  },
  get has() {
    return !!sessionStorage.getItem(TOKEN_KEY)
  },
}

export const http: AxiosInstance = axios.create({
  baseURL: '/api',
  timeout: 60_000,
  headers: { 'Content-Type': 'application/json' },
})

http.interceptors.request.use((config) => {
  if (tokens.access) {
    config.headers.Authorization = `Bearer ${tokens.access}`
  }
  return config
})

/** 401 时尝试用 refresh 换新 token（并发去重）。 */
let refreshing: Promise<boolean> | null = null

async function tryRefresh(): Promise<boolean> {
  if (!tokens.refresh) return false
  if (!refreshing) {
    refreshing = axios
      .post<TokenPair>('/api/auth/refresh', { refresh_token: tokens.refresh })
      .then((r) => {
        tokens.set(r.data)
        return true
      })
      .catch(() => {
        tokens.clear()
        return false
      })
      .finally(() => {
        refreshing = null
      })
  }
  return refreshing
}

http.interceptors.response.use(
  (r) => r,
  async (error: AxiosError<ApiErrorBody>) => {
    const original = error.config as AxiosRequestConfig & { _retried?: boolean }
    const status = error.response?.status ?? 0
    const body = error.response?.data

    // 未登录/过期：刷新一次后重放
    if (status === 401 && original && !original._retried && !original.url?.includes('/auth/login')) {
      original._retried = true
      if (await tryRefresh()) {
        return http.request(original)
      }
      // 刷新失败：交给上层跳登录
      window.dispatchEvent(new CustomEvent('frppanel:unauthorized'))
    }

    if (body && typeof body === 'object' && 'error' in body && body.error) {
      throw new ApiError(body.error.code, body.error.message, status, body.error.details)
    }

    // 非标准错误（网络、网关）
    const message =
      status === 0
        ? '无法连接面板后端：请确认服务已启动'
        : error.message || `请求失败（HTTP ${status}）`
    throw new ApiError(status === 0 ? 'network_error' : 'internal', message, status)
  },
)

/** 把任意异常转成可展示的文案。 */
export function errorMessage(e: unknown): string {
  if (e instanceof ApiError) return e.message
  if (e instanceof Error) return e.message
  return String(e)
}
