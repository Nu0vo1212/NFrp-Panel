<script setup lang="ts">
/** 审计日志：过滤、分页、导出 CSV。 */
import { computed, onMounted, ref } from 'vue'
import { auditApi } from '@/api/endpoints'
import { errorMessage, tokens } from '@/api/client'
import type { AuditLog } from '@/api/types'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateBadge from '@/components/common/StateBadge.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'

const items = ref<AuditLog[]>([])
const total = ref(0)
const page = ref(1)
const pageSize = ref(20)
const totalPages = ref(1)
const loading = ref(false)
const error = ref<string | null>(null)

const filter = ref({ action: '', result: '', actor: '', q: '' })

const actionOptions = computed(() => {
  // 从当前数据里提取，避免硬编码漏掉新动作
  const set = new Set(items.value.map((i) => i.action))
  return Array.from(set).sort()
})

async function load() {
  loading.value = true
  error.value = null
  try {
    const res = await auditApi.list({
      page: page.value,
      page_size: pageSize.value,
      action: filter.value.action || undefined,
      result: filter.value.result || undefined,
      actor: filter.value.actor || undefined,
      q: filter.value.q || undefined,
    })
    items.value = res.items
    total.value = res.pagination.total
    totalPages.value = res.pagination.total_pages
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

function applyFilter() {
  page.value = 1
  load()
}

function reset() {
  filter.value = { action: '', result: '', actor: '', q: '' }
  page.value = 1
  load()
}

function go(p: number) {
  if (p < 1 || p > totalPages.value) return
  page.value = p
  load()
}

function exportCsv() {
  const params = new URLSearchParams()
  if (filter.value.action) params.set('action', filter.value.action)
  if (filter.value.result) params.set('result', filter.value.result)
  if (filter.value.actor) params.set('actor', filter.value.actor)
  if (filter.value.q) params.set('q', filter.value.q)
  fetch(`${auditApi.exportUrl}?${params.toString()}`, {
    headers: { Authorization: `Bearer ${tokens.access}` },
  })
    .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`HTTP ${r.status}`))))
    .then((blob) => {
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = 'audit-logs.csv'
      a.click()
      URL.revokeObjectURL(url)
    })
    .catch((e) => (error.value = `导出失败：${errorMessage(e)}`))
}

function fmtTime(s: string) {
  const d = new Date(s)
  return Number.isNaN(d.getTime()) ? s : d.toLocaleString()
}

function detailText(d: Record<string, unknown>) {
  try {
    return JSON.stringify(d)
  } catch {
    return ''
  }
}

onMounted(load)
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div>
        <h2 class="page-title">审计日志</h2>
        <p class="page-subtitle">
          登录、配置变更、启停、代理增删、凭据与部署操作都会被记录（敏感字段已脱敏）
        </p>
      </div>
      <div class="head-actions">
        <button class="btn btn-ghost" @click="load" :disabled="loading">
          <Icon name="refresh" :size="15" />
          刷新
        </button>
        <button class="btn btn-ghost" @click="exportCsv">
          <Icon name="download" :size="15" />
          导出 CSV
        </button>
      </div>
    </div>

    <AcrylicCard padding="none">
      <div class="toolbar">
        <input
          v-model="filter.q"
          class="input search"
          type="search"
          placeholder="关键字（动作/目标/详情）…"
          @keyup.enter="applyFilter"
        />
        <select v-model="filter.result" class="input select">
          <option value="">全部结果</option>
          <option value="success">成功</option>
          <option value="failure">失败</option>
          <option value="denied">被拒绝</option>
        </select>
        <input v-model="filter.actor" class="input short" placeholder="操作者" @keyup.enter="applyFilter" />
        <input v-model="filter.action" class="input short" placeholder="动作（如 config.write）" @keyup.enter="applyFilter" />
        <button class="btn btn-primary btn-sm" @click="applyFilter">查询</button>
        <button class="btn btn-ghost btn-sm" @click="reset">重置</button>
      </div>

      <StateView v-if="loading && !items.length" kind="loading" :rows="6" />
      <StateView v-else-if="error" kind="error" :description="error">
        <button class="btn btn-ghost" @click="load">重试</button>
      </StateView>
      <StateView
        v-else-if="!items.length"
        kind="empty"
        title="没有匹配的审计记录"
        description="尝试清除筛选条件"
      />
      <div v-else class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th class="col-time">时间</th>
              <th class="col-actor">操作者</th>
              <th class="col-action">动作</th>
              <th class="col-target">目标</th>
              <th class="col-result">结果</th>
              <th class="num col-dur">耗时</th>
              <th class="col-detail">详情</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="log in items" :key="log.id">
              <td class="mono col-time">{{ fmtTime(log.ts) }}</td>
              <td class="col-actor">
                <span class="mono">{{ log.actor }}</span>
                <div class="aux mono">{{ log.actor_ip }}</div>
              </td>
              <td class="mono col-action">{{ log.action }}</td>
              <td class="col-target ellipsis" :title="log.target">{{ log.target }}</td>
              <td class="col-result">
                <StateBadge :state="log.result" size="sm" />
              </td>
              <td class="num mono col-dur">{{ log.duration_ms }} ms</td>
              <td class="col-detail">
                <details>
                  <summary class="aux">展开</summary>
                  <pre class="detail mono">{{ detailText(log.detail) }}</pre>
                </details>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div v-if="items.length" class="pager">
        <span class="aux">共 {{ total }} 条 · 第 {{ page }}/{{ totalPages }} 页</span>
        <div class="pager-btns">
          <button class="btn btn-ghost btn-sm" :disabled="page <= 1" @click="go(page - 1)">上一页</button>
          <button class="btn btn-ghost btn-sm" :disabled="page >= totalPages" @click="go(page + 1)">
            下一页
          </button>
        </div>
      </div>
    </AcrylicCard>
  </div>
</template>

<style scoped>
.head-actions {
  display: flex;
  gap: var(--space-2);
}
.toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-5) var(--space-6);
  border-bottom: 1px solid var(--divider);
  flex-wrap: wrap;
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
  flex: 1 1 220px;
  max-width: 320px;
}
.short {
  width: 150px;
}
.select {
  width: 130px;
}

.table-wrap {
  overflow-x: auto;
}
.table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--fs-body);
  table-layout: fixed;
}
.table th {
  padding: 14px var(--space-6);
  text-align: left;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  background: var(--surface);
  border-bottom: 1px solid var(--divider);
  white-space: nowrap;
}
.table td {
  padding: 12px var(--space-6);
  border-bottom: 1px solid var(--divider);
  vertical-align: top;
}
.col-time {
  width: 150px;
}
.col-actor {
  width: 130px;
}
.col-action {
  width: 150px;
}
.col-target {
  width: 16%;
}
.col-result {
  width: 84px;
}
.col-dur {
  width: 80px;
}
.col-detail {
  width: auto;
}
.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.detail {
  margin: var(--space-2) 0 0;
  padding: var(--space-2);
  background: var(--surface-sunken);
  border-radius: var(--radius-control);
  font-size: 11.5px;
  max-height: 160px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-all;
}
summary {
  cursor: pointer;
  color: var(--primary);
  font-size: var(--fs-aux);
}
.pager {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-3) var(--space-4);
  border-top: 1px solid var(--divider);
}
.pager-btns {
  display: flex;
  gap: var(--space-2);
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
</style>
