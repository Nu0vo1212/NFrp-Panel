<script setup lang="ts">
/** 实例列表 / 概览：状态、类型、驱动、主机、版本、部署状态、快捷启停。 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useInstancesStore } from '@/stores/instances'
import { useSessionStore } from '@/stores/session'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateBadge from '@/components/common/StateBadge.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'
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
        <button class="btn btn-ghost" @click="store.fetchAll()" :disabled="store.loading">
          <Icon name="refresh" :size="15" />
          刷新
        </button>
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
          <tbody>
            <tr
              v-for="inst in store.filtered"
              :key="inst.id"
              class="row"
              @click="open(inst)"
            >
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

.row {
  cursor: pointer;
  transition: background-color var(--dur-fast) var(--ease);
}
.row:hover {
  background: var(--primary-soft);
}
.row:last-child td {
  border-bottom: none;
}

.col-name {
  width: 24%;
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
  z-index: 50;
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
