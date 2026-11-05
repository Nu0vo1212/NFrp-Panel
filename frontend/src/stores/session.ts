/** 会话状态：登录、当前用户、权限、登出。 */

import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { authApi } from '@/api/endpoints'
import { ApiError, tokens } from '@/api/client'
import type { MeResponse, Permissions, UserInfo } from '@/api/types'

export const useSessionStore = defineStore('session', () => {
  const user = ref<UserInfo | null>(null)
  const permissions = ref<Permissions | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)

  const isAuthenticated = computed(() => !!tokens.access)
  const isAdmin = computed(() => user.value?.role === 'admin')
  /** 写操作是否可用（驱动不支持时另说，由各页面按 capabilities 判断）。 */
  const canWrite = computed(() => permissions.value?.can_write ?? false)
  const canDeploy = computed(() => permissions.value?.can_deploy ?? false)
  const canManageSsh = computed(() => permissions.value?.can_manage_ssh ?? false)

  async function login(username: string, password: string) {
    loading.value = true
    error.value = null
    try {
      const pair = await authApi.login(username, password)
      tokens.set(pair)
      user.value = pair.user
      await fetchMe()
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : String(e)
      throw e
    } finally {
      loading.value = false
    }
  }

  async function fetchMe() {
    try {
      const me: MeResponse = await authApi.me()
      user.value = { id: me.id, username: me.username, role: me.role }
      permissions.value = me.permissions
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) {
        tokens.clear()
        user.value = null
        permissions.value = null
      }
      throw e
    }
  }

  async function logout() {
    try {
      await authApi.logout()
    } catch {
      // 登出失败不影响本地清理
    }
    tokens.clear()
    user.value = null
    permissions.value = null
  }

  /** 启动时恢复会话（有 token 则拉一次 /me）。 */
  async function restore() {
    if (!tokens.access) return false
    try {
      await fetchMe()
      return true
    } catch {
      return false
    }
  }

  return {
    user,
    permissions,
    loading,
    error,
    isAuthenticated,
    isAdmin,
    canWrite,
    canDeploy,
    canManageSsh,
    login,
    fetchMe,
    logout,
    restore,
  }
})
