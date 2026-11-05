<script setup lang="ts">
/**
 * 实例详情：概览 / 配置 / 代理 / 监控 / 日志 / 部署 / 能力。
 *
 * 设计要点：
 * - 所有驱动共用同一套页面，差异只来自 `capabilities`（灰化不支持的入口并说明原因）；
 * - 配置编辑有本地校验 + 保存确认；危险操作二次确认；
 * - 敏感字段（Admin API 密码）只显示掩码。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  configApi,
  deployApi,
  instanceApi,
  logApi,
  metricsApi,
  proxyApi,
} from '@/api/endpoints'
import { ApiError, errorMessage, tokens } from '@/api/client'
import type {
  BackupInfo,
  Capabilities,
  ConfigDoc,
  DeployTask,
  Diagnostic,
  HealthReport,
  InstanceDetail,
  InstanceStatus,
  LogLine,
  MetricsResponse,
  ProxyInfo,
  ProxySpec,
  ValidationReport,
} from '@/api/types'
import { useSessionStore } from '@/stores/session'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateBadge from '@/components/common/StateBadge.vue'
import StateView from '@/components/common/StateView.vue'
import TrendChart from '@/components/monitor/TrendChart.vue'
import Icon from '@/components/icons/Icon.vue'

const route = useRoute()
const router = useRouter()
const session = useSessionStore()
const id = computed(() => route.params.id as string)

type Tab = 'overview' | 'config' | 'proxies' | 'metrics' | 'logs' | 'deploy' | 'capabilities'
const tab = ref<Tab>('overview')

const detail = ref<InstanceDetail | null>(null)
const status = ref<InstanceStatus | null>(null)
const health = ref<HealthReport | null>(null)
const caps = computed<Capabilities | null>(() => detail.value?.capabilities ?? null)
const loading = ref(true)
const error = ref<string | null>(null)
const busy = ref(false)
const toast = ref<{ kind: 'ok' | 'err'; text: string } | null>(null)

let timer: number | undefined
let poll: number | undefined

function notify(kind: 'ok' | 'err', text: string) {
  toast.value = { kind, text }
  window.clearTimeout(timer)
  timer = window.setTimeout(() => (toast.value = null), 5000)
}

const canWrite = computed(() => session.canWrite)

// ---------------------------------------------------------------------------
// 基础加载
// ---------------------------------------------------------------------------

async function load() {
  loading.value = true
  error.value = null
  status.value = null
  try {
    detail.value = await instanceApi.get(id.value)
    // ★ 记录数据到了就先渲染表格；status 要走 frp Admin API，放到后台补齐，
    //   否则每次打开实例详情都要串行等两次往返（detail → status）。
    loading.value = false
    void refreshStatus()
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

async function refreshStatus() {
  try {
    status.value = await instanceApi.status(id.value)
  } catch {
    /* 轮询失败静默 */
  }
}

async function checkHealth() {
  busy.value = true
  try {
    health.value = await instanceApi.health(id.value)
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    busy.value = false
  }
}

async function doAction(kind: 'start' | 'stop' | 'restart') {
  busy.value = true
  try {
    if (kind === 'start') await instanceApi.start(id.value)
    else if (kind === 'stop') await instanceApi.stop(id.value)
    else await instanceApi.restart(id.value)
    notify('ok', kind === 'start' ? '已启动' : kind === 'stop' ? '已停止' : '已重启')
    await refreshStatus()
    await load()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    busy.value = false
  }
}

const confirmStop = ref(false)

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

const doc = ref<ConfigDoc | null>(null)
const draft = ref('')
const configDirty = computed(() => !!doc.value && draft.value !== doc.value.content)
const configLoading = ref(false)
const validation = ref<ValidationReport | null>(null)
const saving = ref(false)
const backups = ref<BackupInfo[]>([])
const diagnostics = computed<Diagnostic[]>(() => {
  if (!doc.value) return []
  return validation.value
    ? [...validation.value.errors, ...validation.value.warnings, ...validation.value.hints]
    : doc.value.diagnostics
})

async function loadConfig() {
  configLoading.value = true
  try {
    doc.value = await configApi.get(id.value)
    draft.value = doc.value.content
    validation.value = null
    backups.value = await configApi.backups(id.value).catch(() => [])
  } catch (e) {
    notify('err', errorMessage(e))
    doc.value = null
  } finally {
    configLoading.value = false
  }
}

async function validateDraft() {
  try {
    validation.value = await configApi.validate(id.value, draft.value)
    if (validation.value.ok) notify('ok', '校验通过')
    else notify('err', `校验未通过：${validation.value.errors.length} 个错误`)
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

const confirmSave = ref(false)

async function saveConfig() {
  saving.value = true
  try {
    const res = await configApi.save(id.value, draft.value, true)
    const label =
      res.applied === 'hot'
        ? '已热加载生效'
        : res.applied === 'restarted'
          ? '已重启生效'
          : res.applied === 'restart_required'
            ? '已写入，需要重启生效'
            : '已写入'
    notify('ok', `${label}${res.backup ? '（已备份旧配置）' : ''}`)
    confirmSave.value = false
    await loadConfig()
    await refreshStatus()
    await load()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    saving.value = false
  }
}

async function useTemplate() {
  if (!detail.value) return
  try {
    const t = await configApi.template(detail.value.role, detail.value.flavor)
    draft.value = t.content
    notify('ok', '已填入模板，请按需修改后保存')
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

const confirmRestore = ref<{ visible: boolean; backup: BackupInfo | null }>({
  visible: false,
  backup: null,
})

async function restoreBackup() {
  const b = confirmRestore.value.backup
  if (!b) return
  try {
    const res = await configApi.restore(id.value, b.id)
    notify('ok', `已从备份恢复（${res.applied}）`)
    confirmRestore.value.visible = false
    await loadConfig()
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

// ---------------------------------------------------------------------------
// 代理
// ---------------------------------------------------------------------------

const proxies = ref<ProxyInfo[]>([])
const proxyLoading = ref(false)
const proxyForm = ref<{ visible: boolean; edit: boolean; spec: ProxySpec }>({
  visible: false,
  edit: false,
  spec: { name: '', type: 'tcp', local_addr: '', remote_port: null },
})

const canDynamic = computed(() => caps.value?.can_dynamic_proxy ?? false)

async function loadProxies() {
  proxyLoading.value = true
  try {
    proxies.value = await proxyApi.list(id.value)
  } catch (e) {
    notify('err', errorMessage(e))
    proxies.value = []
  } finally {
    proxyLoading.value = false
  }
}

function openCreateProxy() {
  proxyForm.value = {
    visible: true,
    edit: false,
    spec: { name: '', type: 'tcp', local_addr: '', remote_port: null },
  }
}

function openEditProxy(p: ProxyInfo) {
  proxyForm.value = {
    visible: true,
    edit: true,
    spec: {
      name: p.name,
      type: p.type,
      local_addr: p.local_addr,
      remote_port: p.remote_port,
      custom_domains: p.custom_domains,
      subdomain: p.subdomain,
    },
  }
}

const proxySubmitting = ref(false)

async function submitProxy() {
  const spec = proxyForm.value.spec
  proxySubmitting.value = true
  try {
    const res = proxyForm.value.edit
      ? await proxyApi.update(id.value, spec.name, spec)
      : await proxyApi.create(id.value, spec)
    notify(
      'ok',
      `已${proxyForm.value.edit ? '更新' : '创建'}（经 ${res.applied_via === 'admin_api' ? 'Admin API 动态生效' : '配置改写'}）`,
    )
    proxyForm.value.visible = false
    await loadProxies()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    proxySubmitting.value = false
  }
}

const confirmDeleteProxy = ref<ProxyInfo | null>(null)

async function deleteProxy() {
  const p = confirmDeleteProxy.value
  if (!p) return
  try {
    const res = await proxyApi.remove(id.value, p.name)
    notify('ok', `代理 ${p.name} 已删除（经 ${res.applied_via === 'admin_api' ? 'Admin API' : '配置改写'}）`)
    confirmDeleteProxy.value = null
    await loadProxies()
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

// ---------------------------------------------------------------------------
// 监控
// ---------------------------------------------------------------------------

const metrics = ref<MetricsResponse | null>(null)
const metricsRange = ref('1h')
const metricsLoading = ref(false)

async function loadMetrics() {
  metricsLoading.value = true
  try {
    metrics.value = await metricsApi.history(id.value, metricsRange.value)
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    metricsLoading.value = false
  }
}

function seriesByName(name: string): [number, number][] {
  return metrics.value?.series.find((s) => s.name === name)?.points ?? []
}

const trafficSeries = computed(() => [
  { name: '入站', points: seriesByName('bytes_in'), color: '#3b82f6', area: true, format: 'bytes' as const },
  { name: '出站', points: seriesByName('bytes_out'), color: '#0ea5a4', area: true, format: 'bytes' as const },
])

const connSeries = computed(() => [
  { name: '活跃连接', points: seriesByName('conns'), color: '#3b82f6', area: true, format: 'number' as const },
  { name: '在线客户端', points: seriesByName('clients'), color: '#16a34a', format: 'number' as const },
  { name: '代理数', points: seriesByName('proxies'), color: '#d97706', format: 'number' as const },
])

// ---------------------------------------------------------------------------
// 日志
// ---------------------------------------------------------------------------

const logs = ref<LogLine[]>([])
const logQuery = ref('')
const logLevel = ref('')
const logLoading = ref(false)
const logInfo = ref<{ source: string; path: string; total: number | null } | null>(null)

async function loadLogs() {
  logLoading.value = true
  try {
    const page = await logApi.list(id.value, {
      limit: 300,
      q: logQuery.value || undefined,
      level: logLevel.value || undefined,
    })
    logs.value = page.lines
    logInfo.value = { source: page.source, path: page.path, total: page.total ?? null }
  } catch (e) {
    notify('err', errorMessage(e))
    logs.value = []
  } finally {
    logLoading.value = false
  }
}

function downloadLogs() {
  // 直接带 token 下载会污染 URL，这里用 fetch + blob
  fetch(logApi.downloadUrl(id.value), {
    headers: { Authorization: `Bearer ${tokens.access}` },
  })
    .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`HTTP ${r.status}`))))
    .then((blob) => {
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `${detail.value?.name ?? 'instance'}-logs.txt`
      a.click()
      URL.revokeObjectURL(url)
    })
    .catch((e) => notify('err', `下载失败：${errorMessage(e)}`))
}

// ---------------------------------------------------------------------------
// 部署
// ---------------------------------------------------------------------------

const deployTask = ref<DeployTask | null>(null)
const deployStreaming = ref(false)
const deployLogs = ref<string[]>([])
let stopStream: (() => void) | null = null

async function loadDeployStatus() {
  try {
    deployTask.value = await deployApi.status(id.value)
    deployLogs.value = deployTask.value?.logs ?? []
  } catch {
    /* 无任务 */
  }
}

async function startDeploy() {
  if (!detail.value) return
  try {
    const task = await deployApi.run(id.value, {})
    notify('ok', '部署任务已创建')
    deployStreaming.value = true
    deployLogs.value = []
    stopStream?.()
    stopStream = deployApi.stream(task.task_id, {
      onStep: (d) => {
        const steps = deployTask.value?.steps ?? []
        const found = steps.find((s) => s.key === d.key)
        if (found) {
          found.state = d.state as never
          found.message = d.message
          found.remediation = d.remediation
        }
      },
      onLog: (d) => {
        deployLogs.value.push(`[${d.stream}] ${d.line}`)
        if (deployLogs.value.length > 2000) deployLogs.value.shift()
      },
      onDone: (d) => {
        deployStreaming.value = false
        notify(d.state === 'succeeded' ? 'ok' : 'err', `部署${d.state === 'succeeded' ? '成功' : '失败'}${d.error ? '：' + d.error : ''}`)
        loadDeployStatus()
        load()
      },
    })
    await loadDeployStatus()
  } catch (e) {
    notify('err', errorMessage(e))
    deployStreaming.value = false
  }
}

const confirmRollback = ref(false)

async function rollback() {
  try {
    await deployApi.rollback(id.value)
    notify('ok', '已回滚到上一个配置备份')
    confirmRollback.value = false
    await loadConfig()
    await loadDeployStatus()
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

// ---------------------------------------------------------------------------
// 页签切换时按需加载
// ---------------------------------------------------------------------------

watch(tab, async (t) => {
  if (t === 'config' && !doc.value) await loadConfig()
  if (t === 'proxies' && !proxies.value.length) await loadProxies()
  if (t === 'metrics' && !metrics.value) await loadMetrics()
  if (t === 'logs') await loadLogs()
  if (t === 'deploy') await loadDeployStatus()
})

watch(metricsRange, () => {
  if (tab.value === 'metrics') loadMetrics()
})

onMounted(async () => {
  await load()
  poll = window.setInterval(() => {
    if (document.visibilityState === 'visible') refreshStatus()
  }, 8000)
})

onUnmounted(() => {
  window.clearInterval(poll)
  window.clearTimeout(timer)
  stopStream?.()
})

function fmtBytes(v: number | null | undefined): string {
  if (v == null) return '—'
  if (v < 1024) return `${v} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let n = v / 1024
  let i = 0
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024
    i++
  }
  return `${n.toFixed(n >= 10 ? 0 : 1)} ${units[i]}`
}

function fmtTime(s: string | null | undefined): string {
  if (!s) return '—'
  const d = new Date(s)
  if (Number.isNaN(d.getTime())) return s
  return d.toLocaleString()
}

const tabs: { key: Tab; label: string }[] = [
  { key: 'overview', label: '概览' },
  { key: 'config', label: '配置' },
  { key: 'proxies', label: '代理' },
  { key: 'metrics', label: '监控' },
  { key: 'logs', label: '日志' },
  { key: 'deploy', label: '部署' },
  { key: 'capabilities', label: '能力' },
]
</script>

<template>
  <div class="page">
    <StateView v-if="loading" kind="loading" :rows="6" />

    <StateView
      v-else-if="error"
      kind="error"
      title="实例加载失败"
      :description="error"
    >
      <button class="btn btn-ghost" @click="load()">重试</button>
      <button class="btn btn-ghost" @click="router.push({ name: 'instances' })">返回列表</button>
    </StateView>

    <template v-else-if="detail">
      <!-- 头部 -->
      <div class="detail-head">
        <div class="head-left">
          <button class="back" @click="router.push({ name: 'instances' })" aria-label="返回">
            <Icon name="back" :size="16" />
          </button>
          <div>
            <div class="title-row">
              <h2 class="page-title">{{ detail.name }}</h2>
              <StateBadge :state="detail.live.state" :pulse="detail.live.state === 'running'" />
              <span class="chip">{{ detail.role === 'server' ? 'frps' : 'frpc' }}</span>
              <span class="chip">{{ detail.flavor_label }}</span>
              <span class="chip chip-accent">{{ detail.driver_label }}</span>
            </div>
            <p class="page-subtitle mono">
              {{ detail.host }} · {{ detail.paths.config || '（无配置文件）' }}
            </p>
          </div>
        </div>

        <div class="head-actions">
          <template v-if="detail.capabilities.can_start_stop && canWrite">
            <button
              class="btn btn-ghost"
              :disabled="busy || detail.live.state === 'running'"
              @click="doAction('start')"
            >
              <Icon name="play" :size="15" />
              启动
            </button>
            <button
              class="btn btn-ghost"
              :disabled="busy || detail.live.state !== 'running'"
              @click="confirmStop = true"
            >
              <Icon name="stop" :size="15" />
              停止
            </button>
            <button class="btn btn-ghost" :disabled="busy" @click="doAction('restart')">
              <Icon name="restart" :size="15" />
              重启
            </button>
          </template>
          <button class="btn btn-ghost" :disabled="busy" @click="checkHealth">
            <Icon name="shield" :size="15" />
            健康检查
          </button>
        </div>
      </div>

      <!-- 页签 -->
      <div class="tabs" role="tablist">
        <button
          v-for="t in tabs"
          :key="t.key"
          class="tab"
          role="tab"
          :aria-selected="tab === t.key"
          :class="{ active: tab === t.key }"
          @click="tab = t.key"
        >
          {{ t.label }}
        </button>
      </div>

      <!-- ============ 概览 ============ -->
      <div v-if="tab === 'overview'" class="grid-2">
        <AcrylicCard title="运行状态">
          <dl class="kv">
            <dt>状态</dt>
            <dd><StateBadge :state="status?.state ?? detail.live.state" size="sm" /></dd>
            <dt>健康</dt>
            <dd>
              <StateBadge
                :state="status?.healthy ? 'healthy' : 'unhealthy'"
                size="sm"
              />
            </dd>
            <dt>PID</dt>
            <dd class="mono">{{ status?.pid ?? '—' }}</dd>
            <dt>运行时长</dt>
            <dd class="mono">
              {{ status?.uptime_secs ? Math.floor(status.uptime_secs / 60) + ' 分钟' : '—' }}
            </dd>
            <dt>二进制版本</dt>
            <dd class="mono">{{ status?.binary_version ?? detail.binary_version ?? '—' }}</dd>
            <dt>Admin API</dt>
            <dd>
              <StateBadge
                :state="status?.api_reachable ? 'ok' : 'offline'"
                :label="status?.api_reachable ? '可达' : '不可达'"
                size="sm"
              />
            </dd>
          </dl>
          <p v-if="status?.last_error" class="err-line">{{ status.last_error }}</p>
          <ul v-if="status?.detail.notes.length" class="notes">
            <li v-for="(n, i) in status.detail.notes" :key="i">{{ n }}</li>
          </ul>
        </AcrylicCard>

        <AcrylicCard title="实时计数">
          <div v-if="status?.counters" class="counters">
            <div class="counter">
              <span class="c-label">入站流量</span>
              <span class="c-value num">{{ fmtBytes(status.counters.bytes_in) }}</span>
            </div>
            <div class="counter">
              <span class="c-label">出站流量</span>
              <span class="c-value num">{{ fmtBytes(status.counters.bytes_out) }}</span>
            </div>
            <div class="counter">
              <span class="c-label">活跃连接</span>
              <span class="c-value num">{{ status.counters.conns }}</span>
            </div>
            <div class="counter">
              <span class="c-label">在线客户端</span>
              <span class="c-value num">{{ status.counters.clients }}</span>
            </div>
            <div class="counter">
              <span class="c-label">代理数</span>
              <span class="c-value num">{{ status.counters.proxies }}</span>
            </div>
            <div class="counter">
              <span class="c-label">HTTP 请求</span>
              <span class="c-value num">{{ status.counters.requests ?? '—' }}</span>
            </div>
          </div>
          <StateView v-else kind="empty" title="暂无计数" description="该实例未配置或无法访问 Admin API" />
        </AcrylicCard>

        <AcrylicCard title="健康检查明细" class="span-2">
          <StateView
            v-if="!health"
            kind="empty"
            title="尚未执行"
            description="点击右上角「健康检查」查看进程、配置与 Admin API 的逐项结果"
          />
          <div v-else>
            <div class="health-head">
              <StateBadge :state="health.healthy ? 'healthy' : 'unhealthy'" />
              <span class="aux">{{ health.message }}</span>
            </div>
            <ul class="checks">
              <li v-for="c in health.checks" :key="c.name" class="check">
                <span class="dot" :class="c.ok ? 'dot--success' : 'dot--danger'" />
                <span class="check-name">{{ c.name }}</span>
                <span class="aux mono check-detail">{{ c.detail }}</span>
              </li>
            </ul>
          </div>
        </AcrylicCard>

        <AcrylicCard title="实例信息" class="span-2">
          <dl class="kv kv-wide">
            <dt>驱动</dt>
            <dd>{{ detail.driver_label }}</dd>
            <dt>连接方式</dt>
            <dd>{{ detail.connection_kind === 'ssh' ? 'SSH 远程' : '本机' }}</dd>
            <dt>二进制</dt>
            <dd class="mono">{{ detail.paths.binary || '—' }}</dd>
            <dt>配置</dt>
            <dd class="mono">{{ detail.paths.config || '—' }}</dd>
            <dt>日志</dt>
            <dd class="mono">{{ detail.paths.log || '—' }}</dd>
            <dt>服务托管</dt>
            <dd>{{ detail.service_mode }}</dd>
            <dt>Admin API 地址</dt>
            <dd class="mono">{{ detail.admin_api.base_url || '—' }}</dd>
            <dt>Admin API 密码</dt>
            <dd class="mono">
              {{ detail.admin_api.password_set ? detail.admin_api.password_masked : '未设置' }}
            </dd>
            <template v-if="detail.connection.kind === 'ssh'">
              <dt>SSH</dt>
              <dd class="mono">
                {{ detail.connection.username }}@{{ detail.connection.host }}:{{
                  detail.connection.port
                }}
              </dd>
              <dt>主机指纹</dt>
              <dd>
                <StateBadge
                  :state="detail.connection.host_key_trusted ? 'ok' : 'warning'"
                  :label="detail.connection.host_key_trusted ? '已信任' : '未信任'"
                  size="sm"
                />
              </dd>
            </template>
          </dl>
        </AcrylicCard>
      </div>

      <!-- ============ 配置 ============ -->
      <div v-if="tab === 'config'">
        <AcrylicCard v-if="doc && !doc.editable" title="配置" subtitle="当前驱动不支持编辑配置文件">
          <StateView
            kind="forbidden"
            :description="doc.editable_reason ?? '该驱动不拥有配置文件'"
          />
        </AcrylicCard>

        <div v-else class="config-layout">
          <AcrylicCard padding="none" class="config-editor">
            <template #actions>
              <button class="btn btn-ghost btn-sm" @click="useTemplate">
                <Icon name="file" :size="13" />
                填入模板
              </button>
              <button class="btn btn-ghost btn-sm" :disabled="configLoading" @click="validateDraft">
                <Icon name="check" :size="13" />
                校验
              </button>
              <button
                class="btn btn-primary btn-sm"
                :disabled="!configDirty || saving || !canWrite"
                @click="confirmSave = true"
              >
                <Icon name="save" :size="13" />
                保存
              </button>
            </template>
            <div class="editor-head">
              <span class="mono aux">{{ doc?.path }}</span>
              <span v-if="configDirty" class="dirty">已修改</span>
            </div>
            <StateView v-if="configLoading" kind="loading" :rows="8" />
            <textarea
              v-else
              v-model="draft"
              class="editor mono"
              spellcheck="false"
              aria-label="TOML 配置"
            />
          </AcrylicCard>

          <AcrylicCard title="诊断与备份" class="config-side">
            <ul v-if="diagnostics.length" class="diags">
              <li
                v-for="(d, i) in diagnostics"
                :key="i"
                class="diag"
                :class="`diag-${d.level}`"
              >
                <span class="diag-msg">{{ d.message }}</span>
                <span v-if="d.hint" class="diag-hint">{{ d.hint }}</span>
              </li>
            </ul>
            <StateView v-else kind="empty" title="没有发现问题" description="配置通过校验" />

            <template #footer>
              <div class="backup-head">
                <span class="section-title">历史备份</span>
                <span class="aux">{{ backups.length }} 份</span>
              </div>
              <ul v-if="backups.length" class="backups">
                <li v-for="b in backups.slice(0, 8)" :key="b.id" class="backup">
                  <span class="mono aux">{{ fmtTime(b.created_at) }}</span>
                  <span class="aux num">{{ fmtBytes(b.size) }}</span>
                  <button
                    class="btn btn-ghost btn-sm"
                    :disabled="!canWrite"
                    @click="
                      confirmRestore = { visible: true, backup: b }
                    "
                  >
                    恢复
                  </button>
                </li>
              </ul>
              <p v-else class="aux">暂无备份（保存配置时会自动备份）</p>
            </template>
          </AcrylicCard>
        </div>
      </div>

      <!-- ============ 代理 ============ -->
      <div v-if="tab === 'proxies'">
        <AcrylicCard padding="none">
          <template #actions>
            <span v-if="!canDynamic" class="aux">
              该实现不支持动态增删，变更将改写配置并重启
            </span>
            <button class="btn btn-ghost btn-sm" @click="loadProxies">
              <Icon name="refresh" :size="13" />
              刷新
            </button>
            <button class="btn btn-primary btn-sm" :disabled="!canWrite" @click="openCreateProxy">
              <Icon name="plus" :size="13" />
              新增代理
            </button>
          </template>
          <div class="card-inner-head">
            <h3 class="section-title">代理列表</h3>
          </div>
          <StateView v-if="proxyLoading" kind="loading" :rows="5" />
          <StateView
            v-else-if="!proxies.length"
            kind="empty"
            title="还没有代理"
            description="新建一个 tcp/http 代理即可开始转发"
          >
            <button class="btn btn-primary btn-sm" :disabled="!canWrite" @click="openCreateProxy">
              新增代理
            </button>
          </StateView>
          <div v-else class="table-wrap">
            <table class="table">
              <thead>
                <tr>
                  <th>名称</th>
                  <th>类型</th>
                  <th>本地地址</th>
                  <th class="num">远端端口</th>
                  <th class="num">连接</th>
                  <th>来源</th>
                  <th>状态</th>
                  <th class="right">操作</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="p in proxies" :key="p.name">
                  <td class="mono">{{ p.name }}</td>
                  <td><span class="chip">{{ p.type }}</span></td>
                  <td class="mono">{{ p.local_addr || '—' }}</td>
                  <td class="num mono">{{ p.remote_port ?? '—' }}</td>
                  <td class="num mono">{{ p.cur_conns }}</td>
                  <td>
                    <span class="chip" :class="p.source === 'dynamic' ? 'chip-accent' : ''">
                      {{ p.source === 'dynamic' ? '动态' : '配置' }}
                    </span>
                  </td>
                  <td><StateBadge :state="p.status" size="sm" /></td>
                  <td class="right">
                    <button class="btn btn-ghost btn-sm" :disabled="!canWrite" @click="openEditProxy(p)">
                      <Icon name="edit" :size="13" />
                      编辑
                    </button>
                    <button
                      class="btn btn-danger-ghost btn-sm"
                      :disabled="!canWrite"
                      @click="confirmDeleteProxy = p"
                    >
                      <Icon name="delete" :size="13" />
                      删除
                    </button>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </AcrylicCard>
      </div>

      <!-- ============ 监控 ============ -->
      <div v-if="tab === 'metrics'" class="grid-2">
        <AcrylicCard title="流量趋势" class="span-2">
          <template #actions>
            <select v-model="metricsRange" class="input select" aria-label="时间范围">
              <option value="1h">近 1 小时</option>
              <option value="6h">近 6 小时</option>
              <option value="24h">近 24 小时</option>
              <option value="7d">近 7 天</option>
            </select>
            <button class="btn btn-ghost btn-sm" @click="loadMetrics">刷新</button>
          </template>
          <StateView v-if="metricsLoading" kind="loading" :rows="4" />
          <template v-else>
            <TrendChart
              :series="trafficSeries"
              :height="260"
              y-format="bytes"
              :empty-text="
                metrics?.availability === 'aggregated'
                  ? '该实现不提供流量字节（官方 frps 仅暴露连接数）'
                  : '暂无采样数据（每分钟采样一次）'
              "
            />
            <p v-if="metrics?.notes.length" class="aux notes-inline">
              {{ metrics.notes.join('；') }}
            </p>
          </template>
        </AcrylicCard>

        <AcrylicCard title="连接与代理" class="span-2">
          <TrendChart
            :series="connSeries"
            :height="220"
            y-format="number"
            empty-text="暂无采样数据"
          />
        </AcrylicCard>
      </div>

      <!-- ============ 日志 ============ -->
      <div v-if="tab === 'logs'">
        <AcrylicCard padding="none">
          <template #actions>
            <select v-model="logLevel" class="input select" aria-label="级别筛选" @change="loadLogs">
              <option value="">全部级别</option>
              <option value="ERROR">ERROR</option>
              <option value="WARN">WARN</option>
              <option value="INFO">INFO</option>
              <option value="DEBUG">DEBUG</option>
            </select>
            <input
              v-model="logQuery"
              class="input search"
              type="search"
              placeholder="关键字…"
              @keyup.enter="loadLogs"
            />
            <button class="btn btn-ghost btn-sm" @click="loadLogs">
              <Icon name="activity" :size="13" />
              查询
            </button>
            <button class="btn btn-ghost btn-sm" @click="downloadLogs">
              <Icon name="download" :size="13" />
              下载
            </button>
          </template>
          <div class="log-meta">
            <span class="aux mono">
              {{ logInfo ? `${logInfo.source} · ${logInfo.path} · 共 ${logInfo.total ?? '?'} 行` : '—' }}
            </span>
          </div>
          <StateView v-if="logLoading" kind="loading" :rows="8" />
          <StateView v-else-if="!logs.length" kind="empty" title="没有日志" description="可能日志文件为空或路径不正确" />
          <pre v-else class="log-view mono" aria-label="日志内容"><code>{{ logs.map((l) => l.raw).join('\n') }}</code></pre>
        </AcrylicCard>
      </div>

      <!-- ============ 部署 ============ -->
      <div v-if="tab === 'deploy'" class="grid-2">
        <AcrylicCard title="部署信息">
          <dl class="kv">
            <dt>部署状态</dt>
            <dd><StateBadge :state="detail.deployment.status" size="sm" /></dd>
            <dt>二进制版本</dt>
            <dd class="mono">{{ detail.deployment.binary_version ?? '—' }}</dd>
            <dt>上次部署</dt>
            <dd class="mono">{{ fmtTime(detail.deployment.last_deployed_at) }}</dd>
            <dt>服务托管</dt>
            <dd>{{ detail.deployment.service_mode }}</dd>
            <dt>unit 名称</dt>
            <dd class="mono">{{ detail.deployment.service_unit ?? '—' }}</dd>
            <dt>主机</dt>
            <dd class="mono">{{ detail.host }}</dd>
            <dt>安装路径</dt>
            <dd class="mono">{{ detail.paths.binary || '—' }}</dd>
          </dl>
          <template #footer>
            <div class="deploy-actions">
              <button
                class="btn btn-primary btn-sm"
                :disabled="!session.canDeploy || deployStreaming || detail.connection_kind !== 'ssh'"
                @click="startDeploy"
              >
                <Icon :name="deployStreaming ? 'loading' : 'play'" :size="13" :spin="deployStreaming" />
                {{ deployStreaming ? '部署中…' : '执行部署' }}
              </button>
              <button
                class="btn btn-danger-ghost btn-sm"
                :disabled="!session.canDeploy || detail.connection_kind !== 'ssh'"
                @click="confirmRollback = true"
              >
                <Icon name="restart" :size="13" />
                回滚
              </button>
              <span
                v-if="detail.connection_kind !== 'ssh'"
                class="aux"
              >
                部署仅适用于 SSH 远程实例
              </span>
            </div>
          </template>
        </AcrylicCard>

        <AcrylicCard title="部署进度">
          <StateView
            v-if="!deployTask"
            kind="empty"
            title="暂无部署任务"
            description="执行部署后可在此查看分步进度与实时日志"
          />
          <template v-else>
            <ul class="steps">
              <li v-for="s in deployTask.steps" :key="s.key" class="step" :class="`step-${s.state}`">
                <span class="step-dot" />
                <div class="step-body">
                  <span class="step-title">{{ s.title }}</span>
                  <span v-if="s.message" class="aux step-msg">{{ s.message }}</span>
                  <span v-if="s.remediation" class="step-fix">修复建议：{{ s.remediation }}</span>
                </div>
              </li>
            </ul>
          </template>
        </AcrylicCard>

        <AcrylicCard title="部署日志（流式）" class="span-2">
          <pre
            v-if="deployLogs.length"
            ref="deployLogEl"
            class="log-view mono"
          ><code>{{ deployLogs.join('\n') }}</code></pre>
          <StateView v-else kind="empty" title="暂无日志" />
        </AcrylicCard>
      </div>

      <!-- ============ 能力 ============ -->
      <div v-if="tab === 'capabilities'" class="grid-2">
        <AcrylicCard title="能力探测结果" subtitle="右侧为探测依据；不可用的能力在界面上会被灰化">
          <dl class="kv kv-wide">
            <dt>实现</dt>
            <dd class="mono">{{ caps?.api_kind }}</dd>
            <dt>版本</dt>
            <dd class="mono">{{ caps?.flavor_version ?? '—' }}</dd>
            <dt>接管进程</dt>
            <dd><StateBadge :state="caps?.managed_locally ? 'ok' : 'info'" :label="caps?.managed_locally ? '是' : '否'" size="sm" /></dd>
            <dt>启停</dt>
            <dd><StateBadge :state="caps?.can_start_stop ? 'ok' : 'info'" :label="caps?.can_start_stop ? '支持' : '不支持'" size="sm" /></dd>
            <dt>编辑配置</dt>
            <dd><StateBadge :state="caps?.can_edit_config ? 'ok' : 'info'" :label="caps?.can_edit_config ? '支持' : '不支持'" size="sm" /></dd>
            <dt>热重载</dt>
            <dd><StateBadge :state="caps?.can_reload ? 'ok' : 'warning'" :label="caps?.can_reload ? '支持' : '需重启'" size="sm" /></dd>
            <dt>动态代理</dt>
            <dd><StateBadge :state="caps?.can_dynamic_proxy ? 'ok' : 'warning'" :label="caps?.can_dynamic_proxy ? '支持' : '改写配置'" size="sm" /></dd>
            <dt>指标粒度</dt>
            <dd class="mono">{{ caps?.metrics_detail }}</dd>
            <dt>Prometheus</dt>
            <dd class="mono">{{ caps?.prometheus }}</dd>
            <dt>探测时间</dt>
            <dd class="mono">{{ fmtTime(caps?.probed_at) }}</dd>
          </dl>
        </AcrylicCard>

        <AcrylicCard title="说明">
          <ul v-if="caps?.notes.length" class="diags">
            <li v-for="(n, i) in caps.notes" :key="i" class="diag" :class="`diag-${n.level}`">
              <span class="diag-msg">{{ n.text }}</span>
            </li>
          </ul>
          <StateView v-else kind="empty" title="暂无说明" />
        </AcrylicCard>
      </div>
    </template>

    <!-- ============ 弹层 ============ -->

    <!-- 停止确认 -->
    <div v-if="confirmStop" class="modal-mask" @click.self="confirmStop = false">
      <div class="acrylic modal">
        <h3 class="modal-title">停止实例？</h3>
        <p class="aux">将向 {{ detail?.name }} 发送终止信号；正在转发的连接会中断。</p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmStop = false">取消</button>
          <button
            class="btn btn-danger"
            @click="
              confirmStop = false;
              doAction('stop')
            "
          >
            确认停止
          </button>
        </div>
      </div>
    </div>

    <!-- 保存配置确认 -->
    <div v-if="confirmSave" class="modal-mask" @click.self="confirmSave = false">
      <div class="acrylic modal">
        <h3 class="modal-title">保存并应用配置？</h3>
        <p class="aux">
          将先备份旧配置（带时间戳），再原子替换并尝试热加载；无法热加载时会提示需要重启。
        </p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmSave = false">取消</button>
          <button class="btn btn-primary" :disabled="saving" @click="saveConfig">
            {{ saving ? '保存中…' : '确认保存' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 恢复备份确认 -->
    <div v-if="confirmRestore.visible" class="modal-mask" @click.self="confirmRestore.visible = false">
      <div class="acrylic modal">
        <h3 class="modal-title">恢复这份备份？</h3>
        <p class="aux mono">{{ confirmRestore.backup?.path }}</p>
        <p class="aux">当前配置也会被备份，恢复后按热加载规则生效。</p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmRestore.visible = false">取消</button>
          <button class="btn btn-primary" @click="restoreBackup">确认恢复</button>
        </div>
      </div>
    </div>

    <!-- 删除代理确认 -->
    <div v-if="confirmDeleteProxy" class="modal-mask" @click.self="confirmDeleteProxy = null">
      <div class="acrylic modal">
        <h3 class="modal-title">删除代理 {{ confirmDeleteProxy.name }}？</h3>
        <p class="aux">
          动态代理会经 Admin API 立即移除；配置内的代理会改写配置文件并重启/热加载。
        </p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmDeleteProxy = null">取消</button>
          <button class="btn btn-danger" @click="deleteProxy">确认删除</button>
        </div>
      </div>
    </div>

    <!-- 回滚确认 -->
    <div v-if="confirmRollback" class="modal-mask" @click.self="confirmRollback = false">
      <div class="acrylic modal">
        <h3 class="modal-title">回滚到上一个配置备份？</h3>
        <p class="aux">将把最近一份配置备份写回实例，并按热加载规则生效。</p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmRollback = false">取消</button>
          <button class="btn btn-danger" @click="rollback">确认回滚</button>
        </div>
      </div>
    </div>

    <!-- 代理表单 -->
    <div v-if="proxyForm.visible" class="modal-mask" @click.self="proxyForm.visible = false">
      <div class="acrylic modal modal-lg">
        <h3 class="modal-title">{{ proxyForm.edit ? '编辑代理' : '新增代理' }}</h3>
        <div class="form-grid">
          <label class="field">
            <span class="label">名称</span>
            <input v-model="proxyForm.spec.name" class="input mono" :disabled="proxyForm.edit" />
          </label>
          <label class="field">
            <span class="label">类型</span>
            <select v-model="proxyForm.spec.type" class="input">
              <option value="tcp">tcp</option>
              <option value="udp">udp</option>
              <option value="http">http</option>
              <option value="https">https</option>
              <option value="tcpmux">tcpmux</option>
              <option value="stcp">stcp</option>
              <option value="sudp">sudp</option>
              <option value="xtcp">xtcp</option>
            </select>
          </label>
          <label class="field span-2">
            <span class="label">本地地址（ip:port）</span>
            <input v-model="proxyForm.spec.local_addr" class="input mono" placeholder="127.0.0.1:8080" />
          </label>
          <label class="field">
            <span class="label">远端端口</span>
            <input
              v-model.number="proxyForm.spec.remote_port"
              class="input mono"
              type="number"
              min="1"
              max="65535"
            />
          </label>
          <label class="field">
            <span class="label">子域名（http/https 可选）</span>
            <input v-model="proxyForm.spec.subdomain" class="input mono" />
          </label>
          <label class="field span-2">
            <span class="label">自定义域名（逗号分隔）</span>
            <input
              class="input mono"
              :value="(proxyForm.spec.custom_domains ?? []).join(', ')"
              @input="
                proxyForm.spec.custom_domains = ($event.target as HTMLInputElement).value
                  .split(',')
                  .map((s) => s.trim())
                  .filter(Boolean)
              "
            />
          </label>
        </div>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="proxyForm.visible = false">取消</button>
          <button class="btn btn-primary" :disabled="proxySubmitting" @click="submitProxy">
            {{ proxySubmitting ? '提交中…' : '提交' }}
          </button>
        </div>
      </div>
    </div>

    <Transition name="toast">
      <div v-if="toast" class="toast" :class="`toast-${toast.kind}`" role="status">
        {{ toast.text }}
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* ---- 头部 ---- */
.detail-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
  margin-bottom: var(--space-4);
}

.head-left {
  display: flex;
  align-items: flex-start;
  gap: var(--space-3);
  min-width: 0;
}

.back {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  border-radius: var(--radius-control);
  border: 1px solid var(--border-strong);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  flex: 0 0 auto;
  margin-top: 2px;
}
.back:hover {
  background: var(--surface-sunken);
  color: var(--text);
}

.title-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.head-actions {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.chip {
  display: inline-block;
  padding: 1px 8px;
  border-radius: var(--radius-pill);
  font-size: 11.5px;
  background: var(--surface-sunken);
  color: var(--text-secondary);
  white-space: nowrap;
}
.chip-accent {
  background: var(--primary-soft);
  color: var(--primary);
}

/* ---- 页签 ---- */
.tabs {
  display: flex;
  gap: var(--space-1);
  border-bottom: 1px solid var(--divider);
  margin-bottom: var(--space-6);
  overflow-x: auto;
}

.tab {
  padding: 8px 14px;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--fs-body);
  font-family: inherit;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  white-space: nowrap;
  transition: color var(--dur-fast) var(--ease),
    border-color var(--dur-fast) var(--ease);
}
.tab:hover {
  color: var(--text);
}
.tab.active {
  color: var(--primary);
  border-bottom-color: var(--primary);
  font-weight: 500;
}

/* ---- 栅格 ---- */
.grid-2 {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--space-6);
  align-items: start;
}
.span-2 {
  grid-column: span 2;
}

/* ---- 键值表 ---- */
.kv {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  gap: var(--space-2) var(--space-3);
  font-size: var(--fs-body);
  align-items: center;
}
.kv-wide {
  grid-template-columns: 132px minmax(0, 1fr);
}
.kv dt {
  color: var(--text-secondary);
  font-size: var(--fs-aux);
}
.kv dd {
  margin: 0;
  min-width: 0;
  overflow-wrap: anywhere;
}

/* ---- 计数器 ---- */
.counters {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: var(--space-4);
}
.counter {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.c-label {
  font-size: var(--fs-aux);
  color: var(--text-secondary);
}
.c-value {
  font-size: 18px;
  font-weight: 600;
  text-align: left;
}

/* ---- 健康检查 ---- */
.health-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}
.checks {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.check {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--fs-body);
}
.check-name {
  flex: 0 0 110px;
}
.check-detail {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.notes {
  margin: var(--space-3) 0 0;
  padding-left: var(--space-4);
  color: var(--text-secondary);
  font-size: var(--fs-aux);
}
.notes-inline {
  margin-top: var(--space-2);
}
.err-line {
  margin-top: var(--space-3);
  color: var(--danger);
  font-size: var(--fs-aux);
}

/* ---- 配置 ---- */
.config-layout {
  display: grid;
  grid-template-columns: minmax(0, 2fr) minmax(280px, 1fr);
  gap: var(--space-6);
  align-items: start;
}

.editor-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-2) var(--space-4);
  border-bottom: 1px solid var(--divider);
  gap: var(--space-2);
}
.dirty {
  font-size: 11px;
  color: var(--warning);
  background: var(--warning-soft);
  padding: 1px 8px;
  border-radius: var(--radius-pill);
}

.editor {
  width: 100%;
  min-height: 460px;
  padding: var(--space-4);
  border: none;
  border-radius: 0 0 var(--radius-card) var(--radius-card);
  background: var(--surface-solid);
  color: var(--text);
  font-size: var(--fs-log);
  line-height: 1.6;
  resize: vertical;
  tab-size: 2;
  outline: none;
}
.editor:focus {
  box-shadow: inset 0 0 0 1px var(--primary-border);
}

.diags {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.diag {
  padding: 8px 10px;
  border-radius: var(--radius-control);
  font-size: var(--fs-aux);
  display: flex;
  flex-direction: column;
  gap: 2px;
  border-left: 2px solid transparent;
}
.diag-error {
  background: var(--danger-soft);
  border-left-color: var(--danger);
  color: var(--danger);
}
.diag-warning {
  background: var(--warning-soft);
  border-left-color: var(--warning);
  color: var(--warning);
}
.diag-info {
  background: var(--info-soft);
  border-left-color: var(--info);
  color: var(--text-secondary);
}
.diag-hint {
  color: var(--text-secondary);
  opacity: 0.9;
}

.backup-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}
.backups {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.backup {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 64px auto;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--fs-aux);
}

/* ---- 表格 ---- */
.card-inner-head {
  padding: var(--space-3) var(--space-4) 0;
}
.table-wrap {
  overflow-x: auto;
}
.table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--fs-body);
}
.table th {
  padding: var(--space-3) var(--space-4);
  text-align: left;
  font-size: var(--fs-aux);
  font-weight: 500;
  color: var(--text-secondary);
  background: var(--surface-sunken);
  border-bottom: 1px solid var(--divider);
  white-space: nowrap;
}
.table td {
  padding: var(--space-2) var(--space-4);
  border-bottom: 1px solid var(--divider);
  vertical-align: middle;
}
.table tr:last-child td {
  border-bottom: none;
}
.right {
  text-align: right;
}

/* ---- 日志 ---- */
.log-meta {
  padding: var(--space-2) var(--space-4);
  border-bottom: 1px solid var(--divider);
}
.log-view {
  margin: 0;
  padding: var(--space-4);
  max-height: 520px;
  overflow: auto;
  font-size: var(--fs-log);
  line-height: 1.55;
  background: var(--surface-sunken);
  border-radius: 0 0 var(--radius-card) var(--radius-card);
  white-space: pre;
}
.log-view code {
  font-family: var(--font-mono);
}

/* ---- 部署步骤 ---- */
.steps {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.step {
  display: flex;
  gap: var(--space-3);
  align-items: flex-start;
}
.step-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-top: 7px;
  flex: 0 0 auto;
  background: var(--neutral-dot);
}
.step-done .step-dot {
  background: var(--success);
}
.step-running .step-dot {
  background: var(--primary);
  box-shadow: 0 0 0 3px var(--primary-soft);
}
.step-failed .step-dot {
  background: var(--danger);
}
.step-skipped .step-dot {
  background: var(--border-strong);
}
.step-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.step-title {
  font-size: var(--fs-body);
}
.step-failed .step-title {
  color: var(--danger);
}
.step-msg {
  font-size: var(--fs-aux);
}
.step-fix {
  font-size: var(--fs-aux);
  color: var(--warning);
  background: var(--warning-soft);
  padding: 4px 8px;
  border-radius: var(--radius-control);
  margin-top: 2px;
}

.deploy-actions {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  flex-wrap: wrap;
}

/* ---- 弹层 ---- */
.modal-mask {
  position: fixed;
  inset: 0;
  background: rgba(15, 18, 22, 0.32);
  display: grid;
  place-items: center;
  padding: var(--space-4);
  z-index: 100;
  backdrop-filter: blur(2px);
}
.modal {
  width: 100%;
  max-width: 420px;
  padding: var(--space-5);
  box-shadow: var(--shadow-pop);
}
.modal-lg {
  max-width: 560px;
}
.modal-title {
  font-size: var(--fs-section);
  margin-bottom: var(--space-2);
}
.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  margin-top: var(--space-5);
}

.form-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--space-3);
  margin-top: var(--space-3);
}
.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.label {
  font-size: var(--fs-aux);
  color: var(--text-secondary);
}
.span-2 {
  grid-column: span 2;
}

/* ---- 通用控件 ---- */
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
  white-space: nowrap;
  transition: background-color var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease);
}
.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.btn-sm {
  height: 30px;
  padding: 0 12px;
  font-size: var(--fs-aux);
}
.btn-primary {
  background: var(--primary);
  color: #fff;
  box-shadow: var(--primary-shadow);
}
.btn-primary:hover:not(:disabled) {
  background: var(--primary-hover);
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
  background: var(--danger);
  color: #fff;
}
.btn-danger:hover:not(:disabled) {
  background: var(--danger-hover);
}
.btn-danger-ghost {
  background: transparent;
  border-color: var(--danger-soft);
  color: var(--danger);
}
.btn-danger-ghost:hover:not(:disabled) {
  background: var(--danger-soft);
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
  width: 100%;
  transition: border-color var(--dur-fast) var(--ease),
    box-shadow var(--dur-fast) var(--ease);
}
.input:focus {
  outline: none;
  border-color: var(--primary-border);
  box-shadow: 0 0 0 3px var(--primary-soft);
}
.input.mono {
  font-family: var(--font-mono);
  font-size: var(--fs-log);
}
.select {
  width: auto;
  min-width: 110px;
}
.search {
  width: 180px;
}

/* ---- toast ---- */
.toast {
  position: fixed;
  right: var(--space-6);
  bottom: var(--space-6);
  padding: 10px 16px;
  border-radius: var(--radius-control);
  font-size: var(--fs-body);
  box-shadow: var(--shadow-pop);
  backdrop-filter: var(--acrylic-blur);
  z-index: 120;
  max-width: 460px;
}
.toast-ok {
  background: var(--success-soft);
  color: var(--success);
}
.toast-err {
  background: var(--danger-soft);
  color: var(--danger);
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

@media (max-width: 1000px) {
  .grid-2 {
    grid-template-columns: minmax(0, 1fr);
  }
  .span-2 {
    grid-column: span 1;
  }
  .config-layout {
    grid-template-columns: minmax(0, 1fr);
  }
  .counters {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
