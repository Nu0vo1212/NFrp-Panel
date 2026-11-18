<script setup lang="ts">
/** 用户管理：创建账号、分配角色、重置口令、删除（仅 admin 可用）。 */
import { computed, onMounted, ref } from 'vue'
import { userApi } from '@/api/endpoints'
import { errorMessage } from '@/api/client'
import type { UpdateUserRequest, UserRole, UserSummary } from '@/api/types'
import { useSessionStore } from '@/stores/session'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'

const session = useSessionStore()
const canWrite = computed(() => session.user?.role === 'admin')

const users = ref<UserSummary[]>([])
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
    users.value = await userApi.list()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    loading.value = false
  }
}
onMounted(load)

const form = ref({ visible: false, editing: null as string | null, username: '', password: '', role: 'viewer' as UserRole, changingPw: false })
const submitting = ref(false)

function openCreate() {
  form.value = { visible: true, editing: null, username: '', password: '', role: 'viewer', changingPw: false }
}
function openEditRole(u: UserSummary) {
  form.value = { visible: true, editing: u.id, username: u.username, password: '', role: u.role, changingPw: false }
}
function openResetPw(u: UserSummary) {
  form.value = { visible: true, editing: u.id, username: u.username, password: '', role: u.role, changingPw: true }
}

async function submit() {
  if (!form.value.username.trim()) return notify('err', '用户名不能为空')
  if (!form.value.editing && form.value.password.length < 8) return notify('err', '密码至少 8 位')
  if (form.value.changingPw && form.value.password.length < 8) return notify('err', '新密码至少 8 位')
  submitting.value = true
  try {
    if (!form.value.editing) {
      await userApi.create({ username: form.value.username.trim(), password: form.value.password, role: form.value.role })
      notify('ok', '用户已创建')
    } else {
      const body: UpdateUserRequest = form.value.changingPw
        ? { password: form.value.password }
        : { role: form.value.role }
      await userApi.update(form.value.editing, body)
      notify('ok', form.value.changingPw ? '口令已重置 该用户所有登录已失效' : '角色已更新')
    }
    form.value.visible = false
    await load()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    submitting.value = false
  }
}

const confirmDelete = ref<UserSummary | null>(null)
async function remove() {
  if (!confirmDelete.value) return
  try {
    await userApi.remove(confirmDelete.value.id)
    notify('ok', '用户已删除')
    confirmDelete.value = null
    await load()
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

function roleLabel(r: UserRole) {
  return r === 'admin' ? '管理员' : '只读'
}
function formatTs(s: string | null) {
  if (!s) return '从未登录'
  const d = new Date(s)
  return Number.isNaN(d.getTime()) ? s : d.toLocaleString('zh-CN', { hour12: false })
}
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>用户管理</h1>
        <p class="sub">管理面板账号与权限 管理员可读写 只读账号仅能查看</p>
      </div>
      <button v-if="canWrite" class="btn btn-primary" @click="openCreate">
        <Icon name="plus" :size="15" /> 新建用户
      </button>
    </div>

    <AcrylicCard title="账号列表" :subtitle="`共 ${users.length} 个账号`">
      <StateView v-if="loading" kind="loading" title="加载中" />
      <StateView v-else-if="!users.length" kind="empty" title="还没有用户" description="点击右上角新建" />
      <div v-else class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>用户名</th>
              <th>角色</th>
              <th>最近登录</th>
              <th>创建时间</th>
              <th class="ops">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="u in users" :key="u.id">
              <td class="mono">{{ u.username }}</td>
              <td>
                <span class="role" :class="u.role">{{ roleLabel(u.role) }}</span>
              </td>
              <td class="muted">{{ formatTs(u.last_login_at) }}</td>
              <td class="muted">{{ formatTs(u.created_at) }}</td>
              <td class="ops">
                <template v-if="canWrite">
                  <button class="link" @click="openEditRole(u)">改角色</button>
                  <button class="link" @click="openResetPw(u)">重置口令</button>
                  <button class="link danger" @click="confirmDelete = u">删除</button>
                </template>
                <span v-else class="muted">—</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </AcrylicCard>

    <!-- 新建 / 编辑 / 重置口令 -->
    <div v-if="form.visible" class="overlay" @click.self="form.visible = false">
      <div class="modal">
        <h3 class="modal-title">{{ form.editing ? (form.changingPw ? '重置口令' : '修改角色') : '新建用户' }}</h3>
        <div v-if="!form.editing" class="field">
          <label>用户名</label>
          <input v-model="form.username" class="input" placeholder="登录用户名" autocomplete="off" />
        </div>
        <div v-if="!form.editing || form.changingPw" class="field">
          <label>{{ form.changingPw ? '新口令' : '初始口令' }}</label>
          <input v-model="form.password" type="password" class="input" placeholder="至少 8 位" autocomplete="new-password" />
        </div>
        <div v-if="!form.editing || !form.changingPw" class="field">
          <label>角色</label>
          <select v-model="form.role" class="input">
            <option value="viewer">只读 viewer</option>
            <option value="admin">管理员 admin</option>
          </select>
        </div>
        <p class="hint">口令变更或角色调整会使该账号的所有登录立即失效 需要重新登录</p>
        <div class="modal-actions">
          <button class="btn" @click="form.visible = false">取消</button>
          <button class="btn btn-primary" :disabled="submitting" @click="submit">{{ submitting ? '提交中…' : '确定' }}</button>
        </div>
      </div>
    </div>

    <!-- 删除确认 -->
    <div v-if="confirmDelete" class="overlay" @click.self="confirmDelete = null">
      <div class="modal">
        <h3 class="modal-title">删除用户 {{ confirmDelete.username }}？</h3>
        <p class="hint">该操作不可撤销 删除后该账号所有登录立即失效</p>
        <div class="modal-actions">
          <button class="btn" @click="confirmDelete = null">取消</button>
          <button class="btn btn-danger" @click="remove">确认删除</button>
        </div>
      </div>
    </div>

    <div v-if="toast" class="toast" :class="toast.kind">{{ toast.text }}</div>
  </div>
</template>

<style scoped>
.page { display: flex; flex-direction: column; gap: 20px; }
.head { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
h1 { font-size: 20px; font-weight: 600; margin: 0; }
.sub { color: var(--text-secondary); font-size: 13px; margin: 6px 0 0; }
.btn { display: inline-flex; align-items: center; gap: 6px; padding: 8px 14px; border: 1px solid var(--border); border-radius: 4px; background: var(--surface); color: var(--text); font-size: 13px; cursor: pointer; }
.btn:hover { border-color: var(--primary); }
.btn-primary { background: var(--primary); border-color: var(--primary); color: #fff; }
.btn-primary:disabled { opacity: .55; cursor: not-allowed; }
.btn-danger { background: #c0392b; border-color: #c0392b; color: #fff; }
.table-wrap { overflow-x: auto; }
.table { width: 100%; border-collapse: collapse; font-size: 13px; }
.table th { text-align: left; padding: 10px 12px; color: var(--text-secondary); font-weight: 500; border-bottom: 1px solid var(--border); }
.table td { padding: 11px 12px; border-bottom: 1px solid var(--border); }
.table .ops { text-align: right; white-space: nowrap; }
.mono { font-family: var(--font-mono, monospace); }
.muted { color: var(--text-secondary); }
.role { display: inline-block; padding: 2px 9px; border-radius: 8px; font-size: 12px; }
.role.admin { background: rgba(251,114,153,.14); color: var(--primary); }
.role.viewer { background: var(--surface-sunken); color: var(--text-secondary); }
.link { background: none; border: none; color: var(--primary); font-size: 12px; cursor: pointer; padding: 0 6px; }
.link:hover { text-decoration: underline; }
.link.danger { color: #c0392b; }
.overlay { position: absolute; inset: 0; background: rgba(0,0,0,.4); display: flex; align-items: center; justify-content: center; z-index: 50; }
.modal { background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 24px; width: min(420px, 92vw); box-shadow: 0 12px 40px rgba(0,0,0,.2); }
.modal-title { font-size: 16px; margin: 0 0 18px; }
.field { margin-bottom: 14px; }
.field label { display: block; font-size: 12px; color: var(--text-secondary); margin-bottom: 6px; }
.input { width: 100%; box-sizing: border-box; padding: 9px 11px; border: 1px solid var(--border); border-radius: 4px; background: var(--bg); color: var(--text); font-size: 13px; }
.hint { font-size: 12px; color: var(--text-secondary); line-height: 1.6; }
.modal-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 20px; }
.toast { position: absolute; right: 24px; bottom: 24px; padding: 11px 16px; border-radius: 8px; font-size: 13px; color: #fff; z-index: 60; }
.toast.ok { background: #1d9e75; }
.toast.err { background: #c0392b; }
</style>
