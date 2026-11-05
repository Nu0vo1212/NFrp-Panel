<script setup lang="ts">
/** 设置页：默认值、安全、SSH、凭据管理、known_hosts、主题。 */
import { computed, onMounted, ref } from 'vue'
import { credentialApi, settingsApi, sshApi } from '@/api/endpoints'
import { errorMessage } from '@/api/client'
import type { CredentialSummary, HostKeyRecord, Settings } from '@/api/types'
import { useSessionStore } from '@/stores/session'
import { useThemeStore } from '@/stores/theme'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateBadge from '@/components/common/StateBadge.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'

const session = useSessionStore()
const theme = useThemeStore()

const settings = ref<Settings | null>(null)
const loading = ref(true)
const error = ref<string | null>(null)
const saving = ref(false)
const toast = ref<{ kind: 'ok' | 'err'; text: string } | null>(null)
let toastTimer: number | undefined

function notify(kind: 'ok' | 'err', text: string) {
  toast.value = { kind, text }
  window.clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => (toast.value = null), 4000)
}

/** 局部编辑副本 */
const draft = ref<{
  auto_restart_on_save: boolean
  status_poll_ms: number
  allow_custom_command: boolean
  theme: 'light' | 'dark'
}>({ auto_restart_on_save: false, status_poll_ms: 10000, allow_custom_command: false, theme: 'light' })

const dirty = ref(false)

async function load() {
  loading.value = true
  error.value = null
  try {
    settings.value = await settingsApi.get()
    draft.value = {
      auto_restart_on_save: settings.value.frp.auto_restart_on_save,
      status_poll_ms: settings.value.frp.status_poll_ms,
      allow_custom_command: settings.value.ssh.allow_custom_command,
      theme: settings.value.theme,
    }
    dirty.value = false
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

async function save() {
  saving.value = true
  try {
    settings.value = await settingsApi.update({
      frp: {
        ...(settings.value!.frp),
        auto_restart_on_save: draft.value.auto_restart_on_save,
        status_poll_ms: draft.value.status_poll_ms,
      },
      ssh: {
        ...(settings.value!.ssh),
        allow_custom_command: draft.value.allow_custom_command,
      },
      theme: draft.value.theme,
    })
    theme.set(draft.value.theme)
    dirty.value = false
    notify('ok', '设置已保存（部分项需重启面板生效）')
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    saving.value = false
  }
}

// ---- 凭据 ----
const credentials = ref<CredentialSummary[]>([])
const credLoading = ref(false)
const credForm = ref({
  visible: false,
  name: '',
  kind: 'ssh_password' as 'ssh_password' | 'ssh_private_key',
  username: '',
  secret: '',
  passphrase: '',
})
const credSubmitting = ref(false)
const confirmDeleteCred = ref<CredentialSummary | null>(null)

async function loadCredentials() {
  credLoading.value = true
  try {
    credentials.value = await credentialApi.list()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    credLoading.value = false
  }
}

function openCredForm() {
  credForm.value = {
    visible: true,
    name: '',
    kind: 'ssh_password',
    username: '',
    secret: '',
    passphrase: '',
  }
}

async function submitCredential() {
  const f = credForm.value
  if (!f.name.trim() || !f.secret) {
    notify('err', '请填写名称与凭据内容')
    return
  }
  credSubmitting.value = true
  try {
    await credentialApi.create({
      name: f.name.trim(),
      kind: f.kind,
      username: f.username.trim() || undefined,
      secret: f.secret,
      passphrase: f.passphrase || undefined,
    })
    notify('ok', '凭据已加密保存（不会回显明文）')
    credForm.value.visible = false
    // 明文只在这个闭包里存在过，立刻清空
    f.secret = ''
    f.passphrase = ''
    await loadCredentials()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    credSubmitting.value = false
  }
}

async function deleteCredential() {
  const c = confirmDeleteCred.value
  if (!c) return
  try {
    await credentialApi.remove(c.id)
    notify('ok', '凭据已删除')
    confirmDeleteCred.value = null
    await loadCredentials()
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

// ---- known_hosts ----
const hostKeys = ref<HostKeyRecord[]>([])
const hostKeyLoading = ref(false)
const confirmDeleteHostKey = ref<HostKeyRecord | null>(null)

async function loadHostKeys() {
  hostKeyLoading.value = true
  try {
    hostKeys.value = await sshApi.hostKeys()
  } catch (e) {
    notify('err', errorMessage(e))
  } finally {
    hostKeyLoading.value = false
  }
}

async function deleteHostKey() {
  const h = confirmDeleteHostKey.value
  if (!h) return
  try {
    await sshApi.deleteHostKey(h.id)
    notify('ok', '主机指纹已移除；下次连接需重新确认')
    confirmDeleteHostKey.value = null
    await loadHostKeys()
  } catch (e) {
    notify('err', errorMessage(e))
  }
}

const canEdit = computed(() => session.canWrite)

function fmtTime(s: string | null) {
  if (!s) return '—'
  const d = new Date(s)
  return Number.isNaN(d.getTime()) ? s : d.toLocaleString()
}

onMounted(async () => {
  await load()
  await loadCredentials()
  await loadHostKeys()
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div>
        <h2 class="page-title">设置</h2>
        <p class="page-subtitle">面板默认值、安全项、SSH 默认值与凭据管理</p>
      </div>
      <div class="head-actions">
        <button
          class="btn btn-primary"
          :disabled="!dirty || saving || !canEdit"
          @click="save"
        >
          <Icon :name="saving ? 'loading' : 'save'" :size="15" :spin="saving" />
          {{ saving ? '保存中…' : '保存设置' }}
        </button>
      </div>
    </div>

    <StateView v-if="loading" kind="loading" :rows="8" />
    <StateView v-else-if="error" kind="error" :description="error">
      <button class="btn btn-ghost" @click="load">重试</button>
    </StateView>

    <div v-else class="grid">
      <!-- 默认值 -->
      <AcrylicCard title="默认值" subtitle="新建实例时使用的默认驱动与路径">
        <dl class="kv">
          <dt>默认驱动</dt>
          <dd class="mono">{{ settings?.defaults.driver }}</dd>
          <dt>默认实现</dt>
          <dd class="mono">{{ settings?.defaults.flavor }}</dd>
          <dt>二进制目录</dt>
          <dd class="mono">{{ settings?.defaults.binary_dir }}</dd>
          <dt>服务端二进制</dt>
          <dd class="mono">{{ settings?.defaults.server_binary }}</dd>
          <dt>客户端二进制</dt>
          <dd class="mono">{{ settings?.defaults.client_binary }}</dd>
          <dt>配置目录</dt>
          <dd class="mono">{{ settings?.defaults.config_dir }}</dd>
          <dt>日志目录</dt>
          <dd class="mono">{{ settings?.defaults.log_dir }}</dd>
        </dl>
        <p class="aux hint">默认值来自后端配置文件 panel.toml；此处只展示，修改请编辑该文件并重启。</p>
      </AcrylicCard>

      <!-- 行为 -->
      <AcrylicCard title="行为" subtitle="采样与保存策略">
        <div class="field-row">
          <div>
            <div class="label">保存配置后无法热重载时自动重启</div>
            <div class="aux">官方 frps 没有热重载，开启后保存即重启使其生效</div>
          </div>
          <label class="switch">
            <input
              type="checkbox"
              v-model="draft.auto_restart_on_save"
              :disabled="!canEdit"
              @change="dirty = true"
            />
            <span class="track" />
          </label>
        </div>

        <div class="field-row">
          <div>
            <div class="label">状态采样间隔（毫秒）</div>
            <div class="aux">用于监控趋势；过小会增加 Admin API 压力</div>
          </div>
          <input
            v-model.number="draft.status_poll_ms"
            class="input short mono"
            type="number"
            min="2000"
            step="1000"
            :disabled="!canEdit"
            @change="dirty = true"
          />
        </div>

        <div class="field-row">
          <div>
            <div class="label">主题</div>
            <div class="aux">跟随本机，不影响其他用户</div>
          </div>
          <select
            v-model="draft.theme"
            class="input short"
            @change="
              dirty = true;
              theme.set(draft.theme)
            "
          >
            <option value="light">浅色</option>
            <option value="dark">深色</option>
          </select>
        </div>
      </AcrylicCard>

      <!-- 安全 -->
      <AcrylicCard title="安全" subtitle="密钥指纹与审计保留">
        <dl class="kv">
          <dt>主密钥指纹</dt>
          <dd class="mono">{{ settings?.security.master_key_fingerprint }}</dd>
          <dt>Access Token</dt>
          <dd class="mono">{{ settings?.security.access_token_ttl_minutes }} 分钟</dd>
          <dt>审计保留</dt>
          <dd class="mono">{{ settings?.security.audit_retention_days }} 天</dd>
          <dt>审计开关</dt>
          <dd>
            <StateBadge
              :state="settings?.security.audit_enabled ? 'ok' : 'warning'"
              :label="settings?.security.audit_enabled ? '已启用' : '已关闭'"
              size="sm"
            />
          </dd>
        </dl>
        <p class="aux hint">
          主密钥用于加密 Admin API 密码与 SSH 凭据。指纹用于确认多台面板是否共用同一密钥，不会泄露密钥内容。
        </p>
      </AcrylicCard>

      <!-- SSH 默认值 -->
      <AcrylicCard title="SSH 默认值" subtitle="新建 SSH 实例时的默认参数">
        <dl class="kv">
          <dt>连接超时</dt>
          <dd class="mono">{{ settings?.ssh.connect_timeout_ms }} ms</dd>
          <dt>命令超时</dt>
          <dd class="mono">{{ settings?.ssh.command_timeout_ms }} ms</dd>
          <dt>默认端口</dt>
          <dd class="mono">{{ settings?.ssh.default_port }}</dd>
          <dt>默认用户</dt>
          <dd class="mono">{{ settings?.ssh.default_user }}</dd>
          <dt>安装目录</dt>
          <dd class="mono">{{ settings?.ssh.default_install_dir }}</dd>
          <dt>配置目录</dt>
          <dd class="mono">{{ settings?.ssh.default_config_dir }}</dd>
          <dt>日志目录</dt>
          <dd class="mono">{{ settings?.ssh.default_log_dir }}</dd>
          <dt>服务托管</dt>
          <dd class="mono">{{ settings?.ssh.default_service }}</dd>
        </dl>
        <div class="field-row top-border">
          <div>
            <div class="label">允许自定义提权命令</div>
            <div class="aux">
              默认关闭。开启后可在实例里配置自定义 sudo 前缀（仍限制字符，且写审计）
            </div>
          </div>
          <label class="switch">
            <input
              type="checkbox"
              v-model="draft.allow_custom_command"
              :disabled="!canEdit"
              @change="dirty = true"
            />
            <span class="track" />
          </label>
        </div>
      </AcrylicCard>

      <!-- 凭据 -->
      <AcrylicCard title="SSH 凭据" subtitle="加密存储，前端永不回显明文" class="span-2">
        <template #actions>
          <button class="btn btn-ghost btn-sm" @click="loadCredentials">
            <Icon name="refresh" :size="13" />
            刷新
          </button>
          <button class="btn btn-primary btn-sm" :disabled="!canEdit" @click="openCredForm">
            <Icon name="key" :size="13" />
            新增凭据
          </button>
        </template>

        <StateView v-if="credLoading" kind="loading" :rows="3" />
        <StateView
          v-else-if="!credentials.length"
          kind="empty"
          title="还没有凭据"
          description="创建 SSH 密码或私钥凭据，供 SSH 实例使用"
        />
        <div v-else class="table-wrap">
          <table class="table">
            <thead>
              <tr>
                <th>名称</th>
                <th>类型</th>
                <th>用户名</th>
                <th>内容</th>
                <th>算法</th>
                <th>被引用</th>
                <th>创建时间</th>
                <th class="right">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="c in credentials" :key="c.id">
                <td class="mono">{{ c.name }}</td>
                <td><span class="chip">{{ c.kind === 'ssh_password' ? '密码' : '私钥' }}</span></td>
                <td class="mono">{{ c.username ?? '—' }}</td>
                <td class="mono muted">{{ c.secret_preview }}</td>
                <td class="aux">{{ c.algorithm_hint ?? '—' }}</td>
                <td class="aux">{{ c.used_by.length ? c.used_by.join('、') : '未被引用' }}</td>
                <td class="aux mono">{{ fmtTime(c.created_at) }}</td>
                <td class="right">
                  <button
                    class="btn btn-danger-ghost btn-sm"
                    :disabled="!canEdit"
                    @click="confirmDeleteCred = c"
                  >
                    删除
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </AcrylicCard>

      <!-- known_hosts -->
      <AcrylicCard title="已信任的主机指纹" subtitle="首次连接必须人工确认；指纹变化会被拒绝" class="span-2">
        <template #actions>
          <button class="btn btn-ghost btn-sm" @click="loadHostKeys">
            <Icon name="refresh" :size="13" />
            刷新
          </button>
        </template>
        <StateView v-if="hostKeyLoading" kind="loading" :rows="3" />
        <StateView
          v-else-if="!hostKeys.length"
          kind="empty"
          title="还没有信任任何主机"
          description="在 SSH 实例向导中首次连接时会展示指纹并要求确认"
        />
        <div v-else class="table-wrap">
          <table class="table">
            <thead>
              <tr>
                <th>主机</th>
                <th>算法</th>
                <th>指纹</th>
                <th>信任时间</th>
                <th class="right">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="h in hostKeys" :key="h.id">
                <td class="mono">{{ h.host }}:{{ h.port }}</td>
                <td class="mono">{{ h.algorithm }}</td>
                <td class="mono fingerprint">{{ h.fingerprint }}</td>
                <td class="aux mono">{{ fmtTime(h.trusted_at) }}</td>
                <td class="right">
                  <button
                    class="btn btn-danger-ghost btn-sm"
                    :disabled="!canEdit"
                    @click="confirmDeleteHostKey = h"
                  >
                    移除
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </AcrylicCard>
    </div>

    <!-- 弹层：新增凭据 -->
    <div v-if="credForm.visible" class="modal-mask" @click.self="credForm.visible = false">
      <div class="acrylic modal">
        <h3 class="modal-title">新增 SSH 凭据</h3>
        <div class="form">
          <label class="field">
            <span class="label">名称</span>
            <input v-model="credForm.name" class="input" placeholder="如 prod-web-1" />
          </label>
          <label class="field">
            <span class="label">类型</span>
            <select v-model="credForm.kind" class="input">
              <option value="ssh_password">密码</option>
              <option value="ssh_private_key">私钥</option>
            </select>
          </label>
          <label class="field">
            <span class="label">用户名（可选，便于识别）</span>
            <input v-model="credForm.username" class="input mono" />
          </label>
          <label class="field">
            <span class="label">{{ credForm.kind === 'ssh_password' ? '密码' : '私钥内容' }}</span>
            <textarea
              v-if="credForm.kind === 'ssh_private_key'"
              v-model="credForm.secret"
              class="input textarea mono"
              placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
              rows="6"
              spellcheck="false"
            />
            <input v-else v-model="credForm.secret" class="input" type="password" />
          </label>
          <label v-if="credForm.kind === 'ssh_private_key'" class="field">
            <span class="label">私钥口令（可选）</span>
            <input v-model="credForm.passphrase" class="input" type="password" />
          </label>
          <p class="aux hint">
            内容会以 AES-256-GCM 加密存储，明文不会出现在任何响应、日志或审计里。
          </p>
        </div>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="credForm.visible = false">取消</button>
          <button class="btn btn-primary" :disabled="credSubmitting" @click="submitCredential">
            {{ credSubmitting ? '保存中…' : '保存' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 弹层：删除凭据 -->
    <div v-if="confirmDeleteCred" class="modal-mask" @click.self="confirmDeleteCred = null">
      <div class="acrylic modal">
        <h3 class="modal-title">删除凭据 {{ confirmDeleteCred.name }}？</h3>
        <p class="aux">
          被实例引用的凭据无法删除。删除后需要重新录入明文。
        </p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmDeleteCred = null">取消</button>
          <button class="btn btn-danger" @click="deleteCredential">确认删除</button>
        </div>
      </div>
    </div>

    <!-- 弹层：移除主机指纹 -->
    <div v-if="confirmDeleteHostKey" class="modal-mask" @click.self="confirmDeleteHostKey = null">
      <div class="acrylic modal">
        <h3 class="modal-title">移除该主机指纹？</h3>
        <p class="aux mono">{{ confirmDeleteHostKey.host }}:{{ confirmDeleteHostKey.port }}</p>
        <p class="aux">移除后再次连接会要求重新确认指纹。</p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="confirmDeleteHostKey = null">取消</button>
          <button class="btn btn-danger" @click="deleteHostKey">确认移除</button>
        </div>
      </div>
    </div>

    <Transition name="toast">
      <div v-if="toast" class="toast" :class="`toast-${toast.kind}`">{{ toast.text }}</div>
    </Transition>
  </div>
</template>

<style scoped>
.grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--space-6);
  align-items: start;
}
.span-2 {
  grid-column: span 2;
}

.kv {
  display: grid;
  grid-template-columns: 132px minmax(0, 1fr);
  gap: var(--space-2) var(--space-3);
  font-size: var(--fs-body);
  align-items: center;
}
.kv dt {
  color: var(--text-secondary);
  font-size: var(--fs-aux);
}
.kv dd {
  margin: 0;
  overflow-wrap: anywhere;
}

.hint {
  margin-top: var(--space-3);
  font-size: 11.5px;
  line-height: 1.6;
}

.field-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) 0;
  border-bottom: 1px solid var(--divider);
}
.field-row:last-child {
  border-bottom: none;
}
.top-border {
  margin-top: var(--space-3);
  border-top: 1px solid var(--divider);
  border-bottom: none;
}
.label {
  font-size: var(--fs-body);
}

.switch {
  position: relative;
  display: inline-block;
  width: 40px;
  height: 22px;
  flex: 0 0 auto;
}
.switch input {
  opacity: 0;
  width: 0;
  height: 0;
}
.track {
  position: absolute;
  inset: 0;
  background: var(--border-strong);
  border-radius: var(--radius-pill);
  transition: background-color var(--dur-base) var(--ease);
  cursor: pointer;
}
.track::after {
  content: '';
  position: absolute;
  top: 3px;
  left: 3px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: #fff;
  transition: transform var(--dur-base) var(--ease);
  box-shadow: var(--shadow-1);
}
.switch input:checked + .track {
  background: var(--primary);
}
.switch input:checked + .track::after {
  transform: translateX(18px);
}
.switch input:disabled + .track {
  opacity: 0.5;
  cursor: not-allowed;
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
.short {
  width: 140px;
}
.textarea {
  height: auto;
  padding: 8px 10px;
  resize: vertical;
  line-height: 1.5;
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
.fingerprint {
  font-size: 11.5px;
  max-width: 280px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chip {
  display: inline-block;
  padding: 1px 8px;
  border-radius: var(--radius-pill);
  font-size: 11.5px;
  background: var(--surface-sunken);
  color: var(--text-secondary);
}

/* 弹层 */
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
  max-width: 480px;
  padding: var(--space-5);
  box-shadow: var(--shadow-pop);
}
.modal-title {
  font-size: var(--fs-section);
  margin-bottom: var(--space-3);
}
.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  margin-top: var(--space-5);
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

/* 按钮 */
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
.btn-danger {
  background: var(--danger);
  color: #fff;
}
.btn-danger-ghost {
  background: transparent;
  border-color: var(--danger-soft);
  color: var(--danger);
}
.btn-danger-ghost:hover:not(:disabled) {
  background: var(--danger-soft);
}

.head-actions {
  display: flex;
  gap: var(--space-2);
}

.toast {
  position: fixed;
  right: var(--space-6);
  bottom: var(--space-6);
  padding: 10px 16px;
  border-radius: var(--radius-control);
  font-size: var(--fs-body);
  box-shadow: var(--shadow-pop);
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

@media (max-width: 1000px) {
  .grid {
    grid-template-columns: minmax(0, 1fr);
  }
  .span-2 {
    grid-column: span 1;
  }
}
</style>
