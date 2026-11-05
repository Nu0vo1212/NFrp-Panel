/** 实例列表状态：加载、搜索、快捷启停、轮询。 */

import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { instanceApi } from '@/api/endpoints'
import { ApiError, errorMessage } from '@/api/client'
import type { DriverKind, InstanceRole, InstanceSummary } from '@/api/types'

export const useInstancesStore = defineStore('instances', () => {
  const items = ref<InstanceSummary[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const query = ref('')
  const driverFilter = ref<DriverKind | ''>('')
  const roleFilter = ref<InstanceRole | ''>('')
  /** 正在执行启停的实例 ID（按钮 loading） */
  const busy = ref<Record<string, boolean>>({})
  /** 正在后台探测状态的实例 ID */
  const checking = ref<Record<string, boolean>>({})
  let checkingRound = false

  const filtered = computed(() => {
    const q = query.value.trim().toLowerCase()
    return items.value.filter((i) => {
      if (q && !i.name.toLowerCase().includes(q) && !i.host.toLowerCase().includes(q)) return false
      if (driverFilter.value && i.driver !== driverFilter.value) return false
      if (roleFilter.value && i.role !== roleFilter.value) return false
      return true
    })
  })

  const stats = computed(() => {
    const total = items.value.length
    const running = items.value.filter((i) => i.live.state === 'running').length
    const deployed = items.value.filter((i) => i.deployment_status === 'deployed').length
    const unhealthy = items.value.filter(
      (i) => i.live.state === 'running' && !i.live.healthy,
    ).length
    return { total, running, deployed, unhealthy }
  })

  async function fetchAll() {
    loading.value = true
    error.value = null
    try {
      const res = await instanceApi.list({ page: 1, page_size: 200 })
      // 列表只返回本地库数据，保留上一轮已知状态，避免刷新时状态闪回"未知"
      const prev = new Map(items.value.map((i) => [i.id, i]))
      items.value = res.items.map((i) => {
        const old = prev.get(i.id)
        if (old && i.live.state === 'unknown' && old.live.state !== 'unknown') {
          return {
            ...i,
            live: old.live,
            binary_version: i.binary_version ?? old.binary_version,
          }
        }
        return i
      })
      // 列表先渲染，连接测试在后台逐个补上
      void checkStatuses()
    } catch (e) {
      error.value = errorMessage(e)
    } finally {
      loading.value = false
    }
  }

  /** 逐个查询实例实时状态；最多 6 个并发，结果直接回填列表。 */
  async function checkStatuses() {
    if (checkingRound) return
    const queue = [...items.value]
    if (!queue.length) return
    checkingRound = true

    const checkingIds = queue
      .filter((i) => i.live.state === 'unknown')
      .map((i) => i.id)
    if (checkingIds.length) {
      checking.value = {
        ...checking.value,
        ...Object.fromEntries(checkingIds.map((id) => [id, true])),
      }
    }

    const worker = async () => {
      let cur = queue.shift()
      while (cur) {
        const item = cur
        try {
          const s = await instanceApi.status(item.id)
          const idx = items.value.findIndex((i) => i.id === item.id)
          if (idx >= 0) {
            const base = items.value[idx]
            items.value[idx] = {
              ...base,
              live: {
                state: s.state,
                pid: s.pid,
                healthy: s.healthy,
                since: null,
                error: s.last_error,
              },
              binary_version: s.binary_version ?? base.binary_version,
            }
          }
        } catch (e) {
          const idx = items.value.findIndex((i) => i.id === item.id)
          if (idx >= 0) {
            items.value[idx] = {
              ...items.value[idx],
              live: {
                ...items.value[idx].live,
                state: 'error',
                healthy: false,
                error: errorMessage(e),
              },
            }
          }
        } finally {
          const next = { ...checking.value }
          delete next[item.id]
          checking.value = next
        }
        cur = queue.shift()
      }
    }

    try {
      await Promise.all(
        Array.from({ length: Math.min(6, queue.length) }, () => worker()),
      )
    } finally {
      checkingRound = false
    }
  }

  function setBusy(id: string, v: boolean) {
    busy.value = { ...busy.value, [id]: v }
  }

  async function start(id: string) {
    setBusy(id, true)
    try {
      await instanceApi.start(id)
      await refreshOne(id)
    } finally {
      setBusy(id, false)
    }
  }

  async function stop(id: string) {
    setBusy(id, true)
    try {
      await instanceApi.stop(id)
      await refreshOne(id)
    } finally {
      setBusy(id, false)
    }
  }

  async function restart(id: string) {
    setBusy(id, true)
    try {
      await instanceApi.restart(id)
      await refreshOne(id)
    } finally {
      setBusy(id, false)
    }
  }

  async function remove(id: string, force = false) {
    await instanceApi.remove(id, force)
    items.value = items.value.filter((i) => i.id !== id)
  }

  /** 只刷新单个实例的状态，避免整表重载导致闪烁。 */
  async function refreshOne(id: string) {
    try {
      const detail = await instanceApi.get(id)
      const idx = items.value.findIndex((i) => i.id === id)
      const { paths, admin_api, connection, deployment, tags, ...summary } = detail
      void paths
      void admin_api
      void connection
      void deployment
      void tags
      if (idx >= 0) items.value[idx] = summary
      else await fetchAll()
    } catch (e) {
      if (e instanceof ApiError && e.code === 'not_found') {
        items.value = items.value.filter((i) => i.id !== id)
      }
    }
  }

  return {
    items,
    filtered,
    loading,
    error,
    query,
    driverFilter,
    roleFilter,
    busy,
    checking,
    stats,
    fetchAll,
    checkStatuses,
    start,
    stop,
    restart,
    remove,
    refreshOne,
  }
})
