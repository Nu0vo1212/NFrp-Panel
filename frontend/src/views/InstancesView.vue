<script setup lang="ts">
/** 实例列表 / 概览：状态、类型、驱动、主机、版本、部署状态、快捷启停、分组与批量操作。 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useInstancesStore } from '@/stores/instances'
import { useSessionStore } from '@/stores/session'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateBadge from '@/components/common/StateBadge.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'
import { instanceApi } from '@/api/endpoints'
import { errorMessage } from '@/api/client'
import type { InstanceSummary } from '@/api/types'

const router = useRouter()
const store = useInstancesStore()
const session = useSessionStore()

const toast = ref<{ kind: 'ok' | 'err'; text: string } | null>(null)
let timer: number | undefined
let poll: number | undefined

function notify(kind: 'ok' | 'err', text: string) {
  toast.value = { kind, text }
  window.clearTimeout(timer)
  timer = window.setTimeout(() => (toast.value = null), 4000)
}

const canOperate = computed(() => session.canWrite)
const isAdmin = computed(() => session.user?.role === 'admin')

// ---- 选择与批量 ----
const selected = ref<Set<string>>(new Set())
const batchBusy = ref(false)
const confirmOpen = ref(false)
const confirmText = ref('')

function toggleOne(id: string) {
  const next = new Set(selected.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  selected.value = next
}

function toggleAll() {
  const visible = visibleIds.value
  const allChecked = visible.length > 0 && visible.every((id) => selected.value.has(id))
  const next = new Set(selected.value)
  if (allChecked) visible.forEach((id) => next.delete(id))
  else visible.forEach((id) => next.add(id))
  selected.value = next
}

function clearSelection() {
  selected.value = new Set()
}

const visibleIds = computed(() => store.filtered.map((i) => i.id))
const allChecked = computed(
  () => visibleIds.value.length > 0 && visibleIds.value.every((id) => selected.value.has(id)),
)
const selectedItems = computed(() => store.items.filter((i) => selected.value.has(i.id)))

async function runBatch(action: 'start' | 'stop' | 'restart') {
  const targets = selectedItems.value.filter((i) => i.capabilities.can_start_stop)
  if (!targets.length) {
    notify('err', '所选实例都不支持启停操作')
    return
  }
  batchBusy.value = true
  let ok = 0
  let fail = 0
  // 串行执行，避免同时打爆目标主机
  for (const inst of targets) {
    try {
      await store[action](inst.id)
      ok += 1
    } catch {
      fail += 1
    }
  }
  batchBusy.value = false
  clearSelection()
  notify(
    fail ? 'err' : 'ok',
    `${actionLabel(action)}完成：成功 ${ok} 个${fail ? `，失败 ${fail} 个` : ''}`,
  )
}

function actionLabel(a: 'start' | 'stop' | 'restart') {
  return a === 'start' ? '批量启动' : a === 'stop' ? '批量停止' : '批量重启'
}

function askBatchDelete() {
  confirmText.value = `确定删除所选的 ${selected.value.size} 个实例吗？仅移除面板记录，不会卸载目标主机上的二进制。`
  confirmOpen.value = true
}

async function doBatchDelete() {
  const ids = [...selected.value]
  confirmOpen.value = false
  batchBusy.value = true
  let ok = 0
  let fail = 0
  for (const id of ids) {
    try {
      await store.remove(id, true)
      ok += 1
    } catch {
      fail += 1
    }
  }
  batchBusy.value = false
  clearSelection()
  notify('ok', `已删除 ${ok} 个实例${fail ? `，${fail} 个失败` : ''}`)
  await store.fetchAll()
}

// ---- 分组 ----
const grouped = ref(false)
const groups = computed(() => {
  if (!grouped.value) return [{ key: '__all__', label: '', items: store.filtered }]
  const map = new Map<string, InstanceSummary[]>()
  for (const inst of store.filtered) {
    const key = inst.tags.length ? inst.tags[0] : '未分组'
    const list = map.get(key)
    if (list) list.push(inst)
    else map.set(key, [inst])
  }
  return [...map.entries()]
    .sort((a, b) => (a[0] === '未分组' ? 1 : b[0] === '未分组' ? -1 : a[0].localeCompare(b[0])))
    .map(([key, items]) => ({ key, label: key, items }))
})

// ---- 导入 / 导出 ----
const importInput = ref<HTMLInputElement | null>(null)
const importing = ref(false)

async function doExport() {
  try {
    const data = await instanceApi.exportConfig()
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    const stamp = new Date().toISOString().slice(0, 10)
    a.href = url
    a.download = `nfrp-instances-${stamp}.json`
    a.click()
    URL.revokeObjectURL(url)
    notify('ok', `已导出 ${data.items.length} 个实例配置（不含口令）`)
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

async function onImportFile(ev: Event) {
  const input = ev.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  importing.value = true
  try {
    const text = await file.text()
    const parsed = JSON.parse(text) as unknown
    const items = Array.isArray(parsed)
      ? parsed
      : ((parsed as { items?: unknown[] }).items ?? [])
    if (!Array.isArray(items) || !items.length) {
      throw new Error('文件里没有可导入的实例配置')
    }
    const report = await instanceApi.importConfig(items)
    const parts = [`新增 ${report.created.length} 个`]
    if (report.skipped.length) parts.push(`跳过同名 ${report.skipped.length} 个`)
    if (report.failed.length) parts.push(`失败 ${report.failed.length} 个`)
    notify(report.failed.length ? 'err' : 'ok', `导入完成：${parts.join('，')}`)
    if (report.failed.length) {
      notify('err', `导入失败：${report.failed[0].name} — ${report.failed[0].error}`)
    }
    await store.fetchAll()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    importing.value = false
  }
}

// ---- 单实例操作 ----
async function doStart(inst: InstanceSummary) {
  try {
    await store.start(inst.id)
    notify('ok', `${inst.name} 已启动`)
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

async function doStop(inst: InstanceSummary) {
  try {
    await store.stop(inst.id)
    notify('ok', `${inst.name} 已停止`)
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

function open(inst: InstanceSummary) {
  router.push({ name: 'instance', params: { id: inst.id } })
}

function fmtVersion(v: string | null, inst?: InstanceSummary) {
  if (v) return `v${v}`
  return inst?.live.state === 'running' ? '读取中' : '—'
}

/** 驱动列的补充信息：优先显示实现/API 类型，未探测时提示"待探测"。 */
function countsLabel(inst: InstanceSummary) {
  const c = inst.capabilities
  if (!c) return '待探测'
  if (c.api_kind === 'nfrp') return 'NFrp API'
  if (c.api_kind === 'official') return '官方 API'
  if (c.api_kind === 'none') return 'API 不可达'
  return inst.live.state === 'running' ? '待探测' : '未探测'
}

/** 部署状态：本机二进制无需远程部署，显示"—"而不是"未部署"。 */
function deployText(inst: InstanceSummary): string | null {
  if (inst.connection_kind === 'local' && inst.deployment_status === 'not_deployed') return null
  return inst.deployment_status
}

onMounted(async () => {
  await store.fetchAll()
  // 10 秒刷新一次列表状态（轻量轮询；详情页有自己的刷新）
  poll = window.setInterval(() => {
    if (document.visibilityState === 'visible') store.fetchAll()
  }, 10_000)
})

onUnmounted(() => {
  window.clearInterval(poll)
  window.clearTimeout(timer)
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div>
        <h2 class="page-title">实例</h2>
        <p class="page-subtitle">
          管理本机二进制、SSH 远程二进制与 Admin API 纳管的 frps / frpc
        </p>
      </div>
      <div class="head-actions">
        <input
          ref="importInput"
          type="file"
          accept="application/json,.json"
          class="hidden-file"
          @change="onImportFile"
        />
        <button class="btn btn-ghost" @click="store.fetchAll()" :disabled="store.loading">
          <Icon name="refresh" :size="15" />
          刷新
        </button>
        <template v-if="isAdmin">
          <button
            class="btn btn-ghost"
            :disabled="importing"
            title="从 JSON 导入实例配置（同名跳过）"
            @click="importInput?.click()"
          >
            <Icon name="file" :size="15" />
            {{ importing ? '导入中…' : '导入' }}
          </button>
          <button class="btn btn-ghost" title="导出全部实例配置（不含口令）" @click="doExport">
            <Icon name="download" :size="15" />
            导出
          </button>
        </template>
        <button
          v-if="canOperate"
          class="btn btn-primary"
          @click="router.push({ name: 'instance-new' })"
        >
          <Icon name="plus" :size="15" />
          新建实例
        </button>
      </div>
    </div>

    <!-- 概览数字 -->
    <div class="stats">
      <div class="acrylic stat">
        <div class="stat-head">
          <span class="stat-icon"><Icon name="layers" :size="16" /></span>
          <span class="stat-label">实例总数</span>
        </div>
        <span class="stat-value num">{{ store.stats.total }}</span>
      </div>
      <div class="acrylic stat">
        <div class="stat-head">
          <span class="stat-icon"><Icon name="activity" :size="16" /></span>
          <span class="stat-label">运行中</span>
        </div>
        <span class="stat-value num">{{ store.stats.running }}</span>
      </div>
      <div class="acrylic stat">
        <div class="stat-head">
          <span class="stat-icon"><Icon name="download" :size="16" /></span>
          <span class="stat-label">已部署</span>
        </div>
        <span class="stat-value num">{{ store.stats.deployed }}</span>
      </div>
      <div class="acrylic stat">
        <div class="stat-head">
          <span class="stat-icon"><Icon name="warning" :size="16" /></span>
          <span class="stat-label">运行但不健康</span>
        </div>
        <span
          class="stat-value num"
          :style="{ color: store.stats.unhealthy ? 'var(--warning)' : 'var(--text)' }"
        >
          {{ store.stats.unhealthy }}
        </span>
      </div>
    </div>

    <AcrylicCard padding="none" class="table-card">
      <div class="toolbar">
        <input
          v-model="store.query"
          class="input search"
          type="search"
          placeholder="搜索名称或主机…"
          aria-label="搜索实例"
        />
        <select v-model="store.driverFilter" class="input select" aria-label="按驱动筛选">
          <option value="">全部驱动</option>
          <option value="binary">本机二进制</option>
          <option value="ssh_binary">SSH 远程二进制</option>
          <option value="admin_api">Admin API</option>
          <option value="embedded">内嵌（预留）</option>
        </select>
        <select v-model="store.roleFilter" class="input select" aria-label="按角色筛选">
          <option value="">全部角色</option>
          <option value="server">frps</option>
          <option value="client">frpc</option>
        </select>
        <label class="group-toggle">
          <input v-model="grouped" type="checkbox" />
          <span>按标签分组</span>
        </label>
      </div>

      <!-- 批量操作条 -->
      <div v-if="selected.size" class="batch-bar">
        <span class="batch-count">已选 {{ selected.size }} 个</span>
        <button
          class="btn btn-sm btn-ghost"
          :disabled="batchBusy"
          @click="runBatch('start')"
        >
          <Icon name="play" :size="13" />
          启动
        </button>
        <button class="btn btn-sm btn-ghost" :disabled="batchBusy" @click="runBatch('stop')">
          <Icon name="stop" :size="13" />
          停止
        </button>
        <button class="btn btn-sm btn-ghost" :disabled="batchBusy" @click="runBatch('restart')">
          <Icon name="restart" :size="13" />
          重启
        </button>
        <button
          v-if="isAdmin"
          class="btn btn-sm btn-danger"
          :disabled="batchBusy"
          @click="askBatchDelete"
        >
          <Icon name="delete" :size="13" />
          删除
        </button>
        <button class="btn btn-sm btn-ghost" @click="clearSelection">取消选择</button>
      </div>

      <StateView v-if="store.loading && !store.items.length" kind="loading" :rows="5" />

      <StateView
        v-else-if="store.error"
        kind="error"
        :description="store.error"
        title="实例列表加载失败"
      >
        <button class="btn btn-ghost" @click="store.fetchAll()">重试</button>
      </StateView>

      <StateView
        v-else-if="!store.filtered.length"
        kind="empty"
        :title="store.items.length ? '没有匹配的实例' : '还没有实例哦'"
        :description="
          store.items.length
            ? '换个关键字或清除筛选条件'
            : '新建一个实例即可开始管理 frps / frpc'
        "
      >
        <button
          v-if="canOperate && !store.items.length"
          class="btn btn-primary"
          @click="router.push({ name: 'instance-new' })"
        >
          新建实例
        </button>
      </StateView>

      <div v-else class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th class="col-pick">
                <input
                  type="checkbox"
                  :checked="allChecked"
                  aria-label="全选"
                  @change="toggleAll"
                />
              </th>
              <th class="col-name">名称</th>
              <th class="col-role">角色</th>
              <th class="col-driver">驱动</th>
              <th class="col-host">主机</th>
              <th class="col-ver">版本</th>
              <th class="col-deploy">部署</th>
              <th class="col-state">状态</th>
              <th class="col-actions">操作</th>
            </tr>
          </thead>
          <tbody v-for="group in groups" :key="group.key">
            <tr v-if="grouped" class="group-row">
              <td :colspan="9">
                <span class="group-name">
                  <Icon name="layers" :size="13" />
                  {{ group.label }}
                </span>
                <span class="group-count">{{ group.items.length }}</span>
              </td>
            </tr>
            <tr
              v-for="inst in group.items"
              :key="inst.id"
              class="row"
              :class="{ 'row-selected': selected.has(inst.id) }"
              @click="open(inst)"
            >
              <td class="col-pick" @click.stop>
                <input
                  type="checkbox"
                  :checked="selected.has(inst.id)"
                  :aria-label="`选择 ${inst.name}`"
                  @change="toggleOne(inst.id)"
                />
              </td>
              <td class="col-name">
                <div class="name-cell">
                  <span class="name" :title="inst.name">{{ inst.name }}</span>
                  <span class="tag">{{ inst.flavor_label }}</span>
                </div>
                <div class="aux ident mono" :title="inst.remote_ident">{{ inst.remote_ident }}</div>
              </td>
              <td class="col-role">
                <span class="mono">{{ inst.role === 'server' ? 'frps' : 'frpc' }}</span>
              </td>
              <td class="col-driver">
                <span class="driver-chip" :class="`driver-${inst.driver}`">
                  {{ inst.driver_label }}
                </span>
                <div class="aux">{{ countsLabel(inst) }}</div>
              </td>
              <td class="col-host">
                <span class="mono ellipsis" :title="inst.host">{{ inst.host }}</span>
              </td>
              <td class="col-ver num">{{ fmtVersion(inst.binary_version, inst) }}</td>
              <td class="col-deploy">
                <StateBadge
                  v-if="deployText(inst)"
                  :state="deployText(inst)!"
                  size="sm"
                />
                <span v-else class="aux" title="本机二进制模式无需远程部署">—</span>
              </td>
              <td class="col-state">
                <StateBadge
                  :state="inst.live.state"
                  :label="store.checking[inst.id] ? '检测中' : undefined"
                  size="sm"
                  :pulse="inst.live.state === 'running'"
                />
                <div v-if="inst.live.pid" class="aux mono">PID {{ inst.live.pid }}</div>
                <div v-else-if="inst.live.error" class="aux err-text ellipsis" :title="inst.live.error">
                  {{ inst.live.error }}
                </div>
              </td>
              <td class="col-actions" @click.stop>
                <template v-if="inst.capabilities.can_start_stop && canOperate">
                  <button
                    v-if="inst.live.state !== 'running'"
                    class="btn btn-sm btn-ghost"
                    :disabled="store.busy[inst.id] || store.checking[inst.id]"
                    @click="doStart(inst)"
                  >
                    <Icon name="play" :size="13" />
                    启动
                  </button>
                  <button
                    v-else
                    class="btn btn-sm btn-ghost"
                    :disabled="store.busy[inst.id]"
                    @click="doStop(inst)"
                  >
                    <Icon name="stop" :size="13" />
                    停止
                  </button>
                </template>
                <span v-else class="aux" :title="inst.capabilities.managed_locally ? '' : '该驱动不接管进程'">
                  只读
                </span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </AcrylicCard>

    <!-- 删除确认 -->
    <Transition name="fade">
      <div v-if="confirmOpen" class="overlay" @click.self="confirmOpen = false">
        <div class="dialog">
          <h3 class="dialog-title">确认删除</h3>
          <p class="dialog-text">{{ confirmText }}</p>
          <div class="dialog-actions">
            <button class="btn btn-ghost" @click="confirmOpen = false">取消</button>
            <button class="btn btn-danger" @click="doBatchDelete">确定删除</button>
          </div>
        </div>
      </div>
    </Transition>

    <!-- 轻提示 -->
    <Transition name="toast">
      <div v-if="toast" class="toast" :class="`toast-${toast.kind}`" role="status">
        {{ toast.text }}
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.head-actions {
  display: flex;
  gap: var(--space-3);
  align-items: center;
}
.hidden-file {
  display: none;
}

.btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 38px;
  padding: 0 16px;
  border-radius: var(--radius-control);
  border: 1px solid transparent;
  font-size: var(--fs-body);
  font-family: inherit;
  cursor: pointer;
  transition: background-color var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease),
    transform var(--dur-fast) var(--ease);
  white-space: nowrap;
}
.btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.btn-primary {
  background: var(--primary);
  color: #fff;
  box-shadow: var(--primary-shadow);
}
.btn-primary:hover:not(:disabled) {
  background: var(--primary-hover);
  transform: translateY(-1px);
}
.btn-ghost {
  background: var(--surface-sunken);
  color: var(--text-secondary);
}
.btn-ghost:hover:not(:disabled) {
  background: var(--primary-soft);
  color: var(--primary);
}
.btn-danger {
  background: var(--danger-soft);
  color: var(--danger);
}
.btn-danger:hover:not(:disabled) {
  background: var(--danger);
  color: #fff;
}
.btn-sm {
  height: 30px;
  padding: 0 12px;
  font-size: var(--fs-aux);
}

/* 统计卡 */
.stats {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: var(--space-6);
  margin-bottom: var(--space-8);
}

.stat {
  padding: var(--space-6);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.stat-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-height: 30px;
}
.stat-icon {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: var(--radius-control);
  background: var(--surface-sunken);
  color: var(--text-tertiary);
}
.stat-label {
  font-family: var(--font-mono);
  font-size: 12px;
  letter-spacing: 0.02em;
  color: var(--text-secondary);
  white-space: nowrap;
}
.stat-value {
  font-family: var(--font-mono);
  font-variant-numeric: tabular-nums;
  font-size: 30px;
  font-weight: 600;
  line-height: 1.1;
  text-align: left;
  letter-spacing: -0.02em;
}

/* 工具栏 */
.toolbar {
  display: flex;
  gap: var(--space-3);
  padding: var(--space-5) var(--space-6);
  border-bottom: 1px solid var(--divider);
  align-items: center;
}
.group-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-aux);
  color: var(--text-secondary);
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
}

.batch-bar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: 10px var(--space-6);
  background: var(--primary-soft);
  border-bottom: 1px solid var(--divider);
}
.batch-count {
  font-size: var(--fs-aux);
  font-weight: 600;
  color: var(--primary);
  margin-right: var(--space-2);
}

.input {
  height: 38px;
  padding: 0 14px;
  border-radius: var(--radius-control);
  border: 1px solid transparent;
  background: var(--surface-sunken);
  color: var(--text);
  font-size: var(--fs-body);
  font-family: inherit;
  transition: border-color var(--dur-fast) var(--ease),
    box-shadow var(--dur-fast) var(--ease);
}
.input:focus {
  outline: none;
  border-color: var(--primary-border);
  box-shadow: 0 0 0 3px var(--primary-soft);
}
.search {
  flex: 1 1 auto;
  max-width: 360px;
}
.select {
  width: 160px;
}

/* 表格 */
.table-wrap {
  overflow-x: auto;
}

.table {
  width: 100%;
  border-collapse: collapse;
  table-layout: fixed;
  font-size: var(--fs-body);
}

.table th {
  position: sticky;
  top: 0;
  z-index: 1;
  padding: 14px var(--space-6);
  text-align: left;
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.06em;
  color: var(--text-tertiary);
  background: var(--surface);
  border-bottom: 1px solid var(--divider);
  white-space: nowrap;
}

.table th.col-ver,
.table th.col-actions {
  text-align: right;
}

.table td {
  padding: var(--space-4) var(--space-6);
  vertical-align: middle;
  border-bottom: 1px solid var(--divider);
  font-variant-numeric: tabular-nums;
}

.group-row td {
  padding: 8px var(--space-6);
  background: var(--surface-sunken);
  border-bottom: 1px solid var(--divider);
}
.group-name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-aux);
  font-weight: 600;
  color: var(--text-secondary);
}
.group-count {
  margin-left: 8px;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-tertiary);
}

.row {
  cursor: pointer;
  transition: background-color var(--dur-fast) var(--ease);
}
.row:hover {
  background: var(--primary-soft);
}
.row-selected {
  background: var(--primary-soft);
}
.row:last-child td {
  border-bottom: none;
}

.col-pick {
  width: 3%;
  padding-right: 0 !important;
}
.col-name {
  width: 22%;
}
.col-role {
  width: 7%;
}
.col-driver {
  width: 14%;
}
.col-host {
  width: 17%;
}
.col-ver {
  width: 9%;
}
.col-deploy {
  width: 9%;
}
.col-state {
  width: 12%;
}
.col-actions {
  width: 8%;
  text-align: right;
}

.name-cell {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}
.name {
  font-weight: 600;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tag {
  flex: 0 0 auto;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  background: var(--primary-soft);
  color: var(--primary);
}
.ident {
  margin-top: 2px;
  font-size: 11px;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ellipsis {
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.err-text {
  color: var(--danger);
  max-width: 100%;
}

.driver-chip {
  display: inline-block;
  padding: 1px 8px;
  border-radius: var(--radius-pill);
  font-size: 11.5px;
  border: 1px solid var(--border-strong);
  color: var(--text-secondary);
  white-space: nowrap;
}
.driver-binary {
  color: var(--primary);
  border-color: var(--primary-border);
  background: var(--primary-soft);
}
.driver-ssh_binary {
  color: var(--info);
  border-color: transparent;
  background: var(--info-soft);
}
.driver-admin_api {
  color: var(--text-secondary);
  background: var(--surface-sunken);
  border-color: transparent;
}

/* 确认对话框 */
.overlay {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.35);
  backdrop-filter: blur(2px);
  display: grid;
  place-items: center;
  z-index: 60;
}
.dialog {
  width: min(420px, calc(100vw - 48px));
  padding: var(--space-6);
  border-radius: var(--radius-card);
  background: var(--surface);
  box-shadow: var(--shadow-pop);
  border: 1px solid var(--divider);
}
.dialog-title {
  margin: 0 0 var(--space-3);
  font-size: var(--fs-title-sm, 16px);
  color: var(--text);
}
.dialog-text {
  margin: 0 0 var(--space-6);
  font-size: var(--fs-body);
  line-height: 1.6;
  color: var(--text-secondary);
}
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-3);
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity var(--dur-base) var(--ease);
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

/* toast */
.toast {
  position: fixed;
  right: var(--space-6);
  bottom: var(--space-6);
  padding: 10px 16px;
  border-radius: var(--radius-control);
  font-size: var(--fs-body);
  box-shadow: var(--shadow-pop);
  backdrop-filter: var(--acrylic-blur);
  z-index: 70;
  max-width: 420px;
}
.toast-ok {
  background: var(--success-soft);
  color: var(--success);
  border: 1px solid var(--success-soft);
}
.toast-err {
  background: var(--danger-soft);
  color: var(--danger);
  border: 1px solid var(--danger-soft);
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity var(--dur-base) var(--ease), transform var(--dur-base) var(--ease);
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(6px);
}

@media (max-width: 1100px) {
  .stats {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
