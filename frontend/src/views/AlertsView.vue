<script setup lang="ts">
/** 告警通知：webhook 通道 + 指标阈值规则（仅 admin 可写）。 */
import { computed, onMounted, ref } from 'vue'
import { alertApi, instanceApi } from '@/api/endpoints'
import { errorMessage } from '@/api/client'
import type { AlertRuleView, InstanceSummary, WebhookView } from '@/api/types'
import { ALERT_EVENTS } from '@/api/types'
import { useSessionStore } from '@/stores/session'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'

const session = useSessionStore()
const canWrite = computed(() => session.user?.role === 'admin')

const hooks = ref<WebhookView[]>([])
const rules = ref<AlertRuleView[]>([])
const instances = ref<InstanceSummary[]>([])
const loading = ref(true)
const toast = ref<{ kind: 'ok' | 'err'; text: string } | null>(null)
let toastTimer: number | undefined

function notify(kind: 'ok' | 'err', text: string) {
  toast.value = { kind, text }
  window.clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => (toast.value = null), 4000)
}

async function load() {
  loading.value = true
  try {
    const [h, r, i] = await Promise.all([
      alertApi.webhooks(),
      alertApi.rules(),
      instanceApi.list(),
    ])
    hooks.value = h
    rules.value = r
    instances.value = i.items ?? []
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    loading.value = false
  }
}
onMounted(load)

// ---- webhook ----
const hookForm = ref({ visible: false, name: '', url: '', events: ['deploy_failed'] as string[] })
const testing = ref(false)
const submitting = ref(false)
function openHook() {
  hookForm.value = { visible: false, name: '', url: '', events: ['deploy_failed'] }
  hookForm.value.visible = true
}
function toggleHookEvent(e: string) {
  const arr = hookForm.value.events
  const i = arr.indexOf(e)
  if (i >= 0) arr.splice(i, 1)
  else arr.push(e)
}
async function submitHook() {
  if (!hookForm.value.name.trim() || !hookForm.value.url.trim()) return notify('err', '名称和地址不能为空')
  if (!hookForm.value.events.length) return notify('err', '至少选一个事件')
  submitting.value = true
  try {
    await alertApi.createWebhook({ name: hookForm.value.name.trim(), url: hookForm.value.url.trim(), events: hookForm.value.events })
    notify('ok', 'Webhook 已添加')
    hookForm.value.visible = false
    await load()
  } catch (e) { notify('err', errorMessage(e)) } finally { submitting.value = false }
}
async function testHook() {
  if (!hookForm.value.url.trim()) return notify('err', '请先填写地址')
  testing.value = true
  try {
    const r = await alertApi.testWebhook({ name: 'test', url: hookForm.value.url.trim(), events: [] })
    notify(r.ok ? 'ok' : 'err', r.ok ? '测试消息已送达' : '发送失败 请检查地址')
  } catch (e) { notify('err', errorMessage(e)) } finally { testing.value = false }
}
async function removeHook(id: string) {
  try { await alertApi.deleteWebhook(id); notify('ok', '已删除'); await load() } catch (e) { notify('err', errorMessage(e)) }
}

// ---- rule ----
const ruleForm = ref({ visible: false, instance_id: '', metric: 'bytes_in', op: 'gt', threshold: 1000, window_min: 5 })
function openRule() {
  const first = instances.value[0]?.id ?? ''
  ruleForm.value = { visible: true, instance_id: first, metric: 'bytes_in', op: 'gt', threshold: 1000, window_min: 5 }
}
async function submitRule() {
  if (!ruleForm.value.instance_id) return notify('err', '没有可用实例')
  submitting.value = true
  try {
    await alertApi.createRule({
      instance_id: ruleForm.value.instance_id,
      metric: ruleForm.value.metric,
      op: ruleForm.value.op,
      threshold: Number(ruleForm.value.threshold),
      window_min: Number(ruleForm.value.window_min),
    })
    notify('ok', '告警规则已添加')
    ruleForm.value.visible = false
    await load()
  } catch (e) { notify('err', errorMessage(e)) } finally { submitting.value = false }
}
async function removeRule(id: string) {
  try { await alertApi.deleteRule(id); notify('ok', '已删除'); await load() } catch (e) { notify('err', errorMessage(e)) }
}

function eventLabel(v: string) { return ALERT_EVENTS.find((e) => e.value === v)?.label ?? v }
function instanceName(id: string) { return instances.value.find((i) => i.id === id)?.name ?? id }
function formatTs(s: string) { const d = new Date(s); return Number.isNaN(d.getTime()) ? s : d.toLocaleString('zh-CN', { hour12: false }) }
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>告警通知</h1>
        <p class="sub">实例异常 部署失败或指标超阈值时 主动推送到 Webhook</p>
      </div>
    </div>

    <StateView v-if="loading" kind="loading" title="加载中" />

    <template v-else>
      <AcrylicCard title="Webhook 通道" subtitle="支持企业微信 钉钉 飞书等任意 JSON Webhook">
        <template #actions>
          <button v-if="canWrite" class="btn btn-primary" @click="openHook"><Icon name="plus" :size="14" /> 添加</button>
        </template>
        <StateView v-if="!hooks.length" kind="empty" title="还没有通知通道" description="添加一个 Webhook 让异常主动找你" />
        <div v-else class="table-wrap">
          <table class="table">
            <thead><tr><th>名称</th><th>地址</th><th>订阅事件</th><th class="ops">操作</th></tr></thead>
            <tbody>
              <tr v-for="w in hooks" :key="w.id">
                <td>{{ w.name }}</td>
                <td class="mono truncate">{{ w.url }}</td>
                <td><span v-for="e in w.events" :key="e" class="tag">{{ eventLabel(e) }}</span></td>
                <td class="ops"><button v-if="canWrite" class="link danger" @click="removeHook(w.id)">删除</button><span v-else class="muted">—</span></td>
              </tr>
            </tbody>
          </table>
        </div>
      </AcrylicCard>

      <AcrylicCard title="指标阈值规则" subtitle="采样窗口内均值越界即触发 同一规则 30 分钟内不重复告警">
        <template #actions>
          <button v-if="canWrite" class="btn btn-primary" @click="openRule"><Icon name="plus" :size="14" /> 添加</button>
        </template>
        <StateView v-if="!rules.length" kind="empty" title="还没有阈值规则" description="给关键实例加一条 超阈值立即通知" />
        <div v-else class="table-wrap">
          <table class="table">
            <thead><tr><th>实例</th><th>条件</th><th>窗口</th><th>创建时间</th><th class="ops">操作</th></tr></thead>
            <tbody>
              <tr v-for="r in rules" :key="r.id">
                <td>{{ instanceName(r.instance_id) }}</td>
                <td class="mono">{{ r.metric }} {{ r.op }} {{ r.threshold }}</td>
                <td>{{ r.window_min }} 分钟</td>
                <td class="muted">{{ formatTs(r.created_at) }}</td>
                <td class="ops"><button v-if="canWrite" class="link danger" @click="removeRule(r.id)">删除</button><span v-else class="muted">—</span></td>
              </tr>
            </tbody>
          </table>
        </div>
      </AcrylicCard>
    </template>

    <div v-if="hookForm.visible" class="overlay" @click.self="hookForm.visible = false">
      <div class="modal">
        <h3 class="modal-title">添加 Webhook</h3>
        <div class="field"><label>名称</label><input v-model="hookForm.name" class="input" placeholder="例如 运维群" /></div>
        <div class="field"><label>地址</label><input v-model="hookForm.url" class="input" placeholder="https://..." /></div>
        <div class="field">
          <label>订阅事件</label>
          <div class="checks">
            <label v-for="e in ALERT_EVENTS" :key="e.value" class="check">
              <input type="checkbox" :checked="hookForm.events.includes(e.value)" @change="toggleHookEvent(e.value)" /> {{ e.label }}
            </label>
          </div>
        </div>
        <p class="hint">服务端以 JSON POST 推送 含 event/title/body/ts 字段 请求头带 X-Nfrp-Event</p>
        <div class="modal-actions">
          <button class="btn" @click="testHook" :disabled="testing">{{ testing ? '发送中…' : '发送测试' }}</button>
          <button class="btn" @click="hookForm.visible = false">取消</button>
          <button class="btn btn-primary" :disabled="submitting" @click="submitHook">{{ submitting ? '提交中…' : '确定' }}</button>
        </div>
      </div>
    </div>

    <div v-if="ruleForm.visible" class="overlay" @click.self="ruleForm.visible = false">
      <div class="modal">
        <h3 class="modal-title">添加阈值规则</h3>
        <div class="field"><label>实例</label><select v-model="ruleForm.instance_id" class="input"><option v-for="i in instances" :key="i.id" :value="i.id">{{ i.name }}</option></select></div>
        <div class="field-row">
          <div class="field"><label>指标</label><select v-model="ruleForm.metric" class="input"><option value="bytes_in">流入字节</option><option value="bytes_out">流出字节</option><option value="conns">连接数</option><option value="clients">客户端数</option><option value="proxies">代理数</option></select></div>
          <div class="field"><label>比较</label><select v-model="ruleForm.op" class="input"><option value="gt">大于</option><option value="gte">大于等于</option><option value="lt">小于</option><option value="lte">小于等于</option></select></div>
        </div>
        <div class="field-row">
          <div class="field"><label>阈值</label><input v-model.number="ruleForm.threshold" type="number" class="input" /></div>
          <div class="field"><label>窗口（分钟）</label><input v-model.number="ruleForm.window_min" type="number" min="1" class="input" /></div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="ruleForm.visible = false">取消</button>
          <button class="btn btn-primary" :disabled="submitting" @click="submitRule">{{ submitting ? '提交中…' : '确定' }}</button>
        </div>
      </div>
    </div>

    <div v-if="toast" class="toast" :class="toast.kind">{{ toast.text }}</div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; gap: 20px; }
.head h1 { font-size: 20px; font-weight: 600; margin: 0; }
.sub { color: var(--text-secondary); font-size: 13px; margin: 6px 0 0; }
.btn { display: inline-flex; align-items: center; gap: 6px; padding: 7px 12px; border: 1px solid var(--border); border-radius: 4px; background: var(--surface); color: var(--text); font-size: 13px; cursor: pointer; }
.btn:hover { border-color: var(--primary); }
.btn-primary { background: var(--primary); border-color: var(--primary); color: #fff; }
.btn-primary:disabled, .btn:disabled { opacity: .55; cursor: not-allowed; }
.table-wrap { overflow-x: auto; }
.table { width: 100%; border-collapse: collapse; font-size: 13px; }
.table th { text-align: left; padding: 10px 12px; color: var(--text-secondary); font-weight: 500; border-bottom: 1px solid var(--border); }
.table td { padding: 11px 12px; border-bottom: 1px solid var(--border); }
.table .ops { text-align: right; white-space: nowrap; }
.mono { font-family: var(--font-mono); }
.muted { color: var(--text-secondary); }
.truncate { max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tag { display: inline-block; margin: 0 4px 2px 0; padding: 2px 8px; border-radius: 8px; background: var(--primary-soft); color: var(--primary); font-size: 12px; }
.link { background: none; border: none; color: var(--primary); font-size: 12px; cursor: pointer; padding: 0 6px; }
.link:hover { text-decoration: underline; }
.link.danger { color: var(--danger); }
.overlay { position: absolute; inset: 0; background: rgba(0,0,0,.4); display: flex; align-items: center; justify-content: center; z-index: 50; }
.modal { background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 24px; width: min(460px, 92vw); box-shadow: var(--shadow-pop); }
.modal-title { font-size: 16px; margin: 0 0 18px; }
.field { margin-bottom: 14px; flex: 1; }
.field-row { display: flex; gap: 12px; }
.field label { display: block; font-size: 12px; color: var(--text-secondary); margin-bottom: 6px; }
.input { width: 100%; box-sizing: border-box; padding: 9px 11px; border: 1px solid var(--border); border-radius: 4px; background: var(--bg); color: var(--text); font-size: 13px; }
.checks { display: flex; flex-wrap: wrap; gap: 12px; }
.check { font-size: 13px; color: var(--text); display: inline-flex; align-items: center; gap: 5px; cursor: pointer; }
.hint { font-size: 12px; color: var(--text-secondary); line-height: 1.6; }
.modal-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 20px; }
.toast { position: absolute; right: 24px; bottom: 24px; padding: 11px 16px; border-radius: 8px; font-size: 13px; color: #fff; z-index: 60; }
.toast.ok { background: var(--success); }
.toast.err { background: var(--danger); }
</style>
