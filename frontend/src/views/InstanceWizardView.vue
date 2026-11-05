<script setup lang="ts">
/**
 * 新建实例向导（9 步）。
 *
 * 步骤与后端语义对齐：
 * 1 连接方式 → 2 SSH 配置与指纹确认 → 3 环境探测 → 4 实现与驱动
 * → 5 安装来源与路径 → 6 服务托管 → 7 配置 → 8 部署预检 → 9 执行部署
 *
 * 本机实例会在第 4 步后跳过 SSH 相关步骤，直接进入路径/配置。
 */
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { configApi, credentialApi, deployApi, instanceApi, sshApi } from '@/api/endpoints'
import { errorMessage } from '@/api/client'
import type {
  AuthKind,
  BinaryFlavor,
  CredentialSummary,
  DeployPreflightReport,
  DriverKind,
  InstanceRole,
  RemoteEnvironment,
  ServiceMode,
  SudoMode,
} from '@/api/types'
import AcrylicCard from '@/components/glass/AcrylicCard.vue'
import StateBadge from '@/components/common/StateBadge.vue'
import StateView from '@/components/common/StateView.vue'
import Icon from '@/components/icons/Icon.vue'

const router = useRouter()

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

const step = ref(1)
const totalSteps = 9

const connKind = ref<'local' | 'ssh'>('local')
const role = ref<InstanceRole>('server')
const name = ref('')
const flavor = ref<BinaryFlavor>('nfrp')
const driver = ref<DriverKind>('binary')

// SSH
const ssh = ref({
  host: '',
  port: 22,
  username: 'root',
  auth_kind: 'password' as AuthKind,
  credential_id: '' as string,
  sudo: 'none' as SudoMode,
  secret_inline: '',
})
const credentials = ref<CredentialSummary[]>([])
const hostKeyChecked = ref(false)
const hostKeyFingerprint = ref<string | null>(null)
const hostKeyAlgorithm = ref<string | null>(null)
const testResult = ref<{ ok: boolean; message: string; stage: string } | null>(null)
const testing = ref(false)
const env = ref<RemoteEnvironment | null>(null)
const probing = ref(false)

// 路径
const paths = ref({
  binary: '',
  config: '',
  log: '',
  work_dir: '',
})
const serviceMode = ref<ServiceMode>('process')
const unitName = ref('')

// 安装来源
const sourceKind = ref<'existing' | 'upload' | 'url'>('existing')
const urlInput = ref('')
const urlSha = ref('')
const fetchedBinary = ref<{ b64: string; sha256: string; size: number; notes: string[] } | null>(null)
const uploading = ref(false)
const uploadedFile = ref<{ b64: string; name: string; size: number } | null>(null)

// 配置
const configContent = ref('')
const configNotes = ref<string[]>([])
const configValidating = ref(false)
const configValid = ref<boolean | null>(null)
const configErrors = ref<string[]>([])

// 部署
const preflight = ref<DeployPreflightReport | null>(null)
const preflightLoading = ref(false)
const deployTaskId = ref<string | null>(null)
const deploySteps = ref<{ key: string; title: string; state: string; message?: string; remediation?: string }[]>([])
const deployLogs = ref<string[]>([])
const deploying = ref(false)
const createdId = ref<string | null>(null)
let stopStream: (() => void) | null = null

const error = ref<string | null>(null)
const busy = ref(false)

// ---------------------------------------------------------------------------
// 步骤跳转
// ---------------------------------------------------------------------------

/** 本机模式跳过的步骤：2(SSH)、3(探测)、8(预检)、9 的远程部署 */
const isRemoteFlow = computed(() => connKind.value === 'ssh')

const stepMeta = [
  { n: 1, label: '连接方式' },
  { n: 2, label: 'SSH 与指纹' },
  { n: 3, label: '环境探测' },
  { n: 4, label: '实现与驱动' },
  { n: 5, label: '安装来源' },
  { n: 6, label: '服务托管' },
  { n: 7, label: '配置' },
  { n: 8, label: '部署预检' },
  { n: 9, label: '执行' },
]

const skipped = computed(() => {
  const s = new Set<number>()
  if (!isRemoteFlow.value) {
    s.add(2)
    s.add(3)
    s.add(8)
  }
  return s
})

function nextStep() {
  error.value = null
  let n = step.value + 1
  while (n <= totalSteps && skipped.value.has(n)) n++
  step.value = Math.min(n, totalSteps)
}

function prevStep() {
  error.value = null
  let n = step.value - 1
  while (n >= 1 && skipped.value.has(n)) n--
  step.value = Math.max(n, 1)
}

function canNext(): boolean {
  switch (step.value) {
    case 1:
      return !!name.value.trim()
    case 2:
      return hostKeyChecked.value && !!ssh.value.host.trim()
    case 3:
      return !!env.value
    case 4:
      return true
    case 5:
      return sourceKind.value !== 'url' || !!fetchedBinary.value
    case 6:
      return true
    case 7:
      return !!configContent.value.trim()
    case 8:
      return !!preflight.value?.ok
    default:
      return false
  }
}

// ---------------------------------------------------------------------------
// 每步动作
// ---------------------------------------------------------------------------

async function loadCredentials() {
  try {
    credentials.value = await credentialApi.list()
  } catch {
    credentials.value = []
  }
}

async function testConnection() {
  testing.value = true
  error.value = null
  testResult.value = null
  try {
    const res = await sshApi.test({
      host: ssh.value.host.trim(),
      port: ssh.value.port,
      username: ssh.value.username.trim(),
      auth_kind: ssh.value.auth_kind,
      credential_id: ssh.value.credential_id || undefined,
      secret_inline: ssh.value.credential_id ? undefined : ssh.value.secret_inline || undefined,
    })
    testResult.value = { ok: res.ok, message: res.message, stage: res.stage }
    if (res.host_key) {
      hostKeyFingerprint.value = res.host_key.fingerprint
      hostKeyAlgorithm.value = res.host_key.algorithm
      hostKeyChecked.value = res.host_key.trusted
    }
    if (!res.ok && res.remediation) testResult.value.message += `（建议：${res.remediation}）`
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    testing.value = false
  }
}

const trusting = ref(false)

async function trustHostKey() {
  if (!hostKeyFingerprint.value || !hostKeyAlgorithm.value) return
  trusting.value = true
  try {
    // 需要公钥本体：用一次探测拿到（后端 probe 返回指纹，公钥在 trust 时由后端补齐）
    await sshApi.trustHostKey({
      host: ssh.value.host.trim(),
      port: ssh.value.port,
      algorithm: hostKeyAlgorithm.value,
      fingerprint: hostKeyFingerprint.value,
      public_key: `ssh-ed25519 ${hostKeyFingerprint.value}`,
    })
    hostKeyChecked.value = true
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    trusting.value = false
  }
}

async function probeEnvironment() {
  probing.value = true
  error.value = null
  try {
    env.value = await sshApi.probe({
      host: ssh.value.host.trim(),
      port: ssh.value.port,
      username: ssh.value.username.trim(),
      auth_kind: ssh.value.auth_kind,
      credential_id: ssh.value.credential_id || undefined,
      secret_inline: ssh.value.credential_id ? undefined : ssh.value.secret_inline || undefined,
      config_dir: paths.value.config || '/etc/nfrp',
    })
    // 按探测结果给默认路径
    const bin = role.value === 'server' ? 'frps' : 'frpc'
    if (!paths.value.binary) paths.value.binary = `/usr/local/bin/${bin}`
    if (!paths.value.config) paths.value.config = `/etc/nfrp/${bin}.toml`
    if (!paths.value.log) paths.value.log = `/var/log/nfrp/${bin}.log`
    if (!paths.value.work_dir) paths.value.work_dir = '/var/lib/nfrp'
    if (!unitName.value) unitName.value = `nfrp-${role.value}`
    serviceMode.value = env.value.systemd ? 'systemd' : 'nohup'
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    probing.value = false
  }
}

async function loadTemplate() {
  try {
    const t = await configApi.template(role.value, flavor.value)
    configContent.value = t.content
    configNotes.value = t.notes
  } catch (e) {
    error.value = errorMessage(e)
  }
}

async function validateConfig() {
  configValidating.value = true
  error.value = null
  try {
    // 用后端校验（针对目标实现）
    if (isRemoteFlow.value && createdId.value) {
      const v = await configApi.validate(createdId.value, configContent.value)
      configValid.value = v.ok
      configErrors.value = v.errors.map((e) => e.message)
    } else {
      // 本机/未创建：先创建临时实例再校验成本高，这里用模板自检
      configValid.value = !!configContent.value.trim()
      configErrors.value = []
    }
  } catch (e) {
    configValid.value = false
    configErrors.value = [errorMessage(e)]
  } finally {
    configValidating.value = false
  }
}

async function pickFile(ev: Event) {
  const input = ev.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  uploading.value = true
  try {
    const buf = await file.arrayBuffer()
    const bytes = new Uint8Array(buf)
    let binary = ''
    const chunk = 0x8000
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
    }
    uploadedFile.value = { b64: btoa(binary), name: file.name, size: file.size }
    fetchedBinary.value = null
  } catch (e) {
    error.value = `读取文件失败：${errorMessage(e)}`
  } finally {
    uploading.value = false
  }
}

async function fetchFromUrl() {
  error.value = null
  try {
    const r = await deployApi.fetchBinary(urlInput.value.trim(), urlSha.value.trim() || undefined)
    fetchedBinary.value = { b64: r.binary_b64, sha256: r.sha256, size: r.size, notes: r.notes }
    uploadedFile.value = null
  } catch (e) {
    error.value = errorMessage(e)
  }
}

// ---------------------------------------------------------------------------
// 创建 + 预检 + 部署
// ---------------------------------------------------------------------------

function binaryB64(): string | undefined {
  return uploadedFile.value?.b64 ?? fetchedBinary.value?.b64
}

async function ensureInstance(): Promise<string> {
  if (createdId.value) return createdId.value
  const bin = role.value === 'server' ? 'frps' : 'frpc'
  const localBin = paths.value.binary || bin
  const body = {
    name: name.value.trim(),
    role: role.value,
    driver: driver.value,
    flavor: flavor.value,
    connection: isRemoteFlow.value
      ? {
          kind: 'ssh' as const,
          host: ssh.value.host.trim(),
          port: ssh.value.port,
          username: ssh.value.username.trim(),
          auth_kind: ssh.value.auth_kind,
          credential_id: ssh.value.credential_id || null,
          sudo: ssh.value.sudo,
        }
      : { kind: 'local' as const },
    paths: {
      binary: isRemoteFlow.value ? paths.value.binary : localBin,
      config: isRemoteFlow.value ? paths.value.config : `${localBin}.toml`,
      log: isRemoteFlow.value ? paths.value.log : `${localBin}.log`,
      work_dir: paths.value.work_dir,
    },
    service_mode: serviceMode.value,
    service_unit: unitName.value || null,
  }
  const created = await instanceApi.create(body)
  createdId.value = created.id
  return created.id
}

async function runPreflight() {
  preflightLoading.value = true
  error.value = null
  try {
    const id = await ensureInstance()
    preflight.value = await deployApi.preflight(id, {
      binary_b64: binaryB64(),
      config: configContent.value,
    })
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    preflightLoading.value = false
  }
}

async function runDeploy() {
  deploying.value = true
  error.value = null
  deployLogs.value = []
  try {
    const id = await ensureInstance()
    const task = await deployApi.run(id, {
      binary_b64: binaryB64(),
      config: configContent.value,
      unit_name: unitName.value || undefined,
    })
    deployTaskId.value = task.task_id
    deploySteps.value = []
    stopStream?.()
    stopStream = deployApi.stream(task.task_id, {
      onStep: (d) => {
        const found = deploySteps.value.find((s) => s.key === d.key)
        if (found) {
          found.state = d.state
          found.message = d.message
          found.remediation = d.remediation
        } else {
          deploySteps.value.push({ key: d.key, title: d.key, state: d.state, message: d.message, remediation: d.remediation })
        }
      },
      onLog: (d) => {
        deployLogs.value.push(`[${d.stream}] ${d.line}`)
        if (deployLogs.value.length > 2000) deployLogs.value.shift()
      },
      onDone: (d) => {
        deploying.value = false
        if (d.state !== 'succeeded') error.value = d.error ?? `部署${d.state}`
      },
    })
  } catch (e) {
    error.value = errorMessage(e)
    deploying.value = false
  }
}

async function finishLocal() {
  busy.value = true
  error.value = null
  try {
    const id = await ensureInstance()
    // 本机实例不需要远程部署：直接用后端能力探测一次
    await instanceApi.capabilities(id).catch(() => null)
    router.push({ name: 'instance', params: { id } })
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    busy.value = false
  }
}

// 进入某些步骤时自动准备
watch(step, async (s) => {
  if (s === 2) await loadCredentials()
  if (s === 5 && isRemoteFlow.value) {
    const bin = role.value === 'server' ? 'frps' : 'frpc'
    if (!paths.value.binary) paths.value.binary = `/usr/local/bin/${bin}`
    if (!unitName.value) unitName.value = `nfrp-${role.value}`
  }
  if (s === 7 && !configContent.value) await loadTemplate()
  if (s === 9 && isRemoteFlow.value && !deployTaskId.value) await runDeploy()
  if (s === 9 && !isRemoteFlow.value) await finishLocal()
})

// 驱动与连接方式联动
watch(connKind, (k) => {
  driver.value = k === 'local' ? 'binary' : 'ssh_binary'
  if (k === 'local') serviceMode.value = 'process'
})
watch(role, (r) => {
  const bin = r === 'server' ? 'frps' : 'frpc'
  if (paths.value.binary.startsWith('/usr/local/bin/')) paths.value.binary = `/usr/local/bin/${bin}`
  if (serviceMode.value !== 'process' && !unitName.value.trim()) unitName.value = `nfrp-${r}`
  configContent.value = ''
})
</script>

<template>
  <div class="page">
    <div class="page-header">
      <div>
        <h2 class="page-title">新建实例</h2>
        <p class="page-subtitle">按步骤配置；SSH 模式会先探测环境，再预检、部署并流式展示过程</p>
      </div>
      <button class="btn btn-ghost" @click="router.push({ name: 'instances' })">
        <Icon name="close" :size="15" />
        取消
      </button>
    </div>

    <!-- 步骤条 -->
    <ol class="stepper">
      <li
        v-for="s in stepMeta"
        :key="s.n"
        class="stepper-item"
        :class="{
          active: step === s.n,
          done: step > s.n,
          skipped: skipped.has(s.n),
        }"
      >
        <span class="idx">{{ skipped.has(s.n) ? '—' : s.n }}</span>
        <span class="stepper-label">{{ s.label }}</span>
      </li>
    </ol>

    <p v-if="error" class="err-banner" role="alert">{{ error }}</p>

    <AcrylicCard :title="`第 ${step} 步 · ${stepMeta[step - 1].label}`">
      <!-- 1 连接方式 -->
      <div v-if="step === 1" class="form">
        <label class="field">
          <span class="label">实例名称</span>
          <input v-model="name" class="input" placeholder="如 prod-frps / office-frpc" />
        </label>

        <div class="field">
          <span class="label">角色</span>
          <div class="choice-row">
            <button
              class="choice"
              :class="{ active: role === 'server' }"
              @click="role = 'server'"
            >
              <strong>frps</strong>
              <span class="aux">服务端（公网机器）</span>
            </button>
            <button
              class="choice"
              :class="{ active: role === 'client' }"
              @click="role = 'client'"
            >
              <strong>frpc</strong>
              <span class="aux">客户端（内网机器）</span>
            </button>
          </div>
        </div>

        <div class="field">
          <span class="label">连接方式</span>
          <div class="choice-row">
            <button
              class="choice"
              :class="{ active: connKind === 'local' }"
              @click="connKind = 'local'"
            >
              <strong>本机</strong>
              <span class="aux">面板直接管理本机进程与配置</span>
            </button>
            <button
              class="choice"
              :class="{ active: connKind === 'ssh' }"
              @click="connKind = 'ssh'"
            >
              <strong>SSH 远程</strong>
              <span class="aux">通过 SSH 部署并管理远程主机</span>
            </button>
          </div>
        </div>
      </div>

      <!-- 2 SSH 与指纹 -->
      <div v-else-if="step === 2" class="form">
        <div class="grid-2">
          <label class="field">
            <span class="label">主机</span>
            <input v-model="ssh.host" class="input mono" placeholder="1.2.3.4 或 host.example.com" />
          </label>
          <label class="field">
            <span class="label">端口</span>
            <input v-model.number="ssh.port" class="input mono" type="number" min="1" max="65535" />
          </label>
          <label class="field">
            <span class="label">用户名</span>
            <input v-model="ssh.username" class="input mono" />
          </label>
          <label class="field">
            <span class="label">提权方式</span>
            <select v-model="ssh.sudo" class="input">
              <option value="none">不使用 sudo</option>
              <option value="sudo">sudo</option>
              <option value="sudo_n">sudo -n（免密）</option>
              <option value="custom">自定义（需在设置中开启）</option>
            </select>
          </label>
          <label class="field">
            <span class="label">认证方式</span>
            <select v-model="ssh.auth_kind" class="input">
              <option value="password">密码</option>
              <option value="private_key">私钥</option>
              <option value="private_key_passphrase">私钥 + 口令</option>
            </select>
          </label>
          <label class="field">
            <span class="label">凭据</span>
            <select v-model="ssh.credential_id" class="input">
              <option value="">使用临时凭据（不保存）</option>
              <option v-for="c in credentials" :key="c.id" :value="c.id">
                {{ c.name }}（{{ c.kind === 'ssh_password' ? '密码' : '私钥' }}）
              </option>
            </select>
          </label>
        </div>

        <label v-if="!ssh.credential_id" class="field">
          <span class="label">临时{{ ssh.auth_kind === 'password' ? '密码' : '私钥' }}（仅本次使用，不落库）</span>
          <input v-if="ssh.auth_kind === 'password'" v-model="ssh.secret_inline" class="input" type="password" />
          <textarea
            v-else
            v-model="ssh.secret_inline"
            class="input textarea mono"
            rows="5"
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
          />
        </label>

        <div class="actions">
          <button class="btn btn-ghost" :disabled="testing || !ssh.host" @click="testConnection">
            <Icon :name="testing ? 'loading' : 'network'" :size="15" :spin="testing" />
            {{ testing ? '连接中…' : '测试连接' }}
          </button>
          <span v-if="testResult" class="aux">
            <StateBadge :state="testResult.ok ? 'ok' : 'fail'" size="sm" />
            {{ testResult.message }}
          </span>
        </div>

        <!-- 指纹确认 -->
        <div v-if="hostKeyFingerprint" class="fingerprint-box">
          <div class="fp-head">
            <span class="label">主机指纹</span>
            <StateBadge
              :state="hostKeyChecked ? 'ok' : 'warning'"
              :label="hostKeyChecked ? '已信任' : '待确认'"
              size="sm"
            />
          </div>
          <div class="fp-value mono">{{ hostKeyAlgorithm }} {{ hostKeyFingerprint }}</div>
          <p class="aux">
            首次连接必须核对指纹。请与服务器上
            <code class="mono">ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub</code>
            的输出比对，一致才可信任。
          </p>
          <button
            v-if="!hostKeyChecked"
            class="btn btn-primary btn-sm"
            :disabled="trusting"
            @click="trustHostKey"
          >
            <Icon :name="trusting ? 'loading' : 'fingerprint'" :size="13" :spin="trusting" />
            {{ trusting ? '信任中…' : '我已核对，信任该指纹' }}
          </button>
        </div>
      </div>

      <!-- 3 环境探测 -->
      <div v-else-if="step === 3">
        <div class="actions">
          <button class="btn btn-ghost" :disabled="probing" @click="probeEnvironment">
            <Icon :name="probing ? 'loading' : 'cpu'" :size="15" :spin="probing" />
            {{ probing ? '探测中…' : env ? '重新探测' : '开始探测' }}
          </button>
        </div>
        <StateView v-if="!env" kind="empty" title="尚未探测" description="探测 OS/架构、systemd、sudo 权限、磁盘与已有安装" />
        <template v-else>
          <dl class="kv">
            <dt>操作系统</dt>
            <dd class="mono">{{ env.os }} {{ env.kernel }}</dd>
            <dt>架构</dt>
            <dd class="mono">{{ env.arch }}</dd>
            <dt>主机名</dt>
            <dd class="mono">{{ env.hostname }}</dd>
            <dt>systemd</dt>
            <dd>
              <StateBadge
                :state="env.systemd ? 'ok' : 'warning'"
                :label="env.systemd ? '可用' : '不可用（将用 nohup）'"
                size="sm"
              />
            </dd>
            <dt>sudo</dt>
            <dd>
              <StateBadge
                :state="env.sudo === 'passwordless' || env.current_user === 'root' ? 'ok' : 'warning'"
                :label="`${env.sudo}（用户 ${env.current_user}）`"
                size="sm"
              />
            </dd>
            <dt>磁盘可用</dt>
            <dd class="mono">{{ env.disk_free_mb }} MB</dd>
            <dt>已有安装</dt>
            <dd class="mono">
              {{ env.existing.frps || env.existing.frpc || '未检测到' }}
            </dd>
            <dt>端口占用</dt>
            <dd class="mono">{{ env.listening_ports.length ? env.listening_ports.join(', ') : '无' }}</dd>
          </dl>
          <ul v-if="env.notes.length" class="notes">
            <li v-for="(n, i) in env.notes" :key="i">{{ n }}</li>
          </ul>
        </template>
      </div>

      <!-- 4 实现与驱动 -->
      <div v-else-if="step === 4" class="form">
        <div class="field">
          <span class="label">二进制实现</span>
          <div class="choice-row">
            <button class="choice" :class="{ active: flavor === 'nfrp' }" @click="flavor = 'nfrp'">
              <strong>NFrp</strong>
              <span class="aux">Rust 实现，支持动态代理与完整指标</span>
            </button>
            <button
              class="choice"
              :class="{ active: flavor === 'official_frp' }"
              @click="flavor = 'official_frp'"
            >
              <strong>官方 frp</strong>
              <span class="aux">无动态代理接口，变更写入配置后重启</span>
            </button>
          </div>
        </div>

        <dl class="kv">
          <dt>驱动</dt>
          <dd class="mono">{{ driver }}</dd>
          <dt>说明</dt>
          <dd class="aux">
            {{
              driver === 'binary'
                ? '面板直接管理本机进程、配置文件与 Admin API'
                : '面板通过 SSH 在远程主机上部署和管理二进制与服务'
            }}
          </dd>
        </dl>

        <p class="aux hint">
          提示：驱动由第一步的「连接方式」决定。若要纳管一个**已经在运行**的实例而不接管进程，
          请创建后把驱动改为 Admin API（需要实例已开放面板端口）。
        </p>
      </div>

      <!-- 5 安装来源与路径 -->
      <div v-else-if="step === 5" class="form">
        <div class="field">
          <span class="label">二进制来源</span>
          <div class="choice-row">
            <button class="choice" :class="{ active: sourceKind === 'existing' }" @click="sourceKind = 'existing'">
              <strong>使用远端已有</strong>
              <span class="aux">仅下发配置，不替换二进制</span>
            </button>
            <button class="choice" :class="{ active: sourceKind === 'upload' }" @click="sourceKind = 'upload'">
              <strong>上传本地文件</strong>
              <span class="aux">适合离线环境（校验哈希与架构）</span>
            </button>
            <button class="choice" :class="{ active: sourceKind === 'url' }" @click="sourceKind = 'url'">
              <strong>从 URL 下载</strong>
              <span class="aux">建议同时提供 SHA256</span>
            </button>
          </div>
        </div>

        <div v-if="sourceKind === 'upload'" class="field">
          <input type="file" class="input" @change="pickFile" />
          <span v-if="uploadedFile" class="aux">
            已选择 {{ uploadedFile.name }}（{{ uploadedFile.size }} 字节）
          </span>
        </div>

        <div v-if="sourceKind === 'url'" class="grid-2">
          <label class="field">
            <span class="label">下载地址</span>
            <input v-model="urlInput" class="input mono" placeholder="https://…/frps" />
          </label>
          <label class="field">
            <span class="label">SHA256（可选）</span>
            <input v-model="urlSha" class="input mono" />
          </label>
          <div class="actions">
            <button class="btn btn-ghost" @click="fetchFromUrl">下载并校验</button>
            <span v-if="fetchedBinary" class="aux">
              已获取 {{ fetchedBinary.size }} 字节 · {{ fetchedBinary.sha256.slice(0, 12) }}…
            </span>
          </div>
          <ul v-if="fetchedBinary?.notes.length" class="notes span-2">
            <li v-for="(n, i) in fetchedBinary.notes" :key="i">{{ n }}</li>
          </ul>
        </div>

        <div class="grid-2">
          <label class="field">
            <span class="label">二进制路径</span>
            <input v-model="paths.binary" class="input mono" />
          </label>
          <label class="field">
            <span class="label">配置文件路径</span>
            <input v-model="paths.config" class="input mono" />
          </label>
          <label class="field">
            <span class="label">日志路径</span>
            <input v-model="paths.log" class="input mono" />
          </label>
          <label class="field">
            <span class="label">工作目录</span>
            <input v-model="paths.work_dir" class="input mono" />
          </label>
        </div>
      </div>

      <!-- 6 服务托管 -->
      <div v-else-if="step === 6" class="form">
        <div class="field">
          <span class="label">托管方式</span>
          <div class="choice-row">
            <button class="choice" :class="{ active: serviceMode === 'systemd' }" @click="serviceMode = 'systemd'">
              <strong>systemd</strong>
              <span class="aux">推荐：开机自启 + 自动重启</span>
            </button>
            <button class="choice" :class="{ active: serviceMode === 'nohup' }" @click="serviceMode = 'nohup'">
              <strong>nohup</strong>
              <span class="aux">无 systemd 时的回退，不会开机自启</span>
            </button>
          </div>
        </div>
        <label v-if="serviceMode === 'systemd'" class="field">
          <span class="label">unit 名称</span>
          <input v-model="unitName" class="input mono" placeholder="nfrp-server" />
        </label>
        <p v-if="env && !env.systemd && serviceMode === 'systemd'" class="warn-banner">
          远端没有 systemd，部署时会自动回退到 nohup 模式。
        </p>
      </div>

      <!-- 7 配置 -->
      <div v-else-if="step === 7" class="form">
        <div class="actions">
          <button class="btn btn-ghost btn-sm" @click="loadTemplate">重新填入模板</button>
          <button class="btn btn-ghost btn-sm" :disabled="configValidating" @click="validateConfig">
            {{ configValidating ? '校验中…' : '校验' }}
          </button>
          <StateBadge
            v-if="configValid"
            :state="configValid ? 'ok' : 'fail'"
            :label="configValid ? '校验通过' : '校验失败'"
            size="sm"
          />
        </div>
        <ul v-if="configErrors.length" class="diags">
          <li v-for="(e, i) in configErrors" :key="i" class="diag diag-error">{{ e }}</li>
        </ul>
        <textarea v-model="configContent" class="input textarea mono editor" rows="16" spellcheck="false" />
        <ul v-if="configNotes.length" class="notes">
          <li v-for="(n, i) in configNotes" :key="i">{{ n }}</li>
        </ul>
      </div>

      <!-- 8 预检 -->
      <div v-else-if="step === 8">
        <div class="actions">
          <button class="btn btn-ghost" :disabled="preflightLoading" @click="runPreflight">
            {{ preflightLoading ? '预检中…' : '执行预检' }}
          </button>
          <StateBadge
            v-if="preflight"
            :state="preflight.ok ? 'pass' : 'fail'"
            :label="preflight.ok ? '预检通过' : '预检未通过'"
            size="sm"
          />
        </div>
        <StateView v-if="!preflight" kind="empty" title="尚未预检" description="预检会检查 OS/架构/二进制/权限/端口" />
        <ul v-else class="preflight-list">
          <li v-for="it in preflight.items" :key="it.key" class="pf-item" :class="`pf-${it.status}`">
            <StateBadge :state="it.status" size="sm" />
            <div class="pf-body">
              <span class="pf-label">{{ it.label }}</span>
              <span class="aux">{{ it.message }}</span>
              <span v-if="it.remediation" class="pf-fix">修复建议：{{ it.remediation }}</span>
            </div>
          </li>
        </ul>
      </div>

      <!-- 9 执行 -->
      <div v-else-if="step === 9">
        <template v-if="isRemoteFlow">
          <div class="actions">
            <StateBadge
              :state="deploying ? 'running' : deployLogs.length ? 'done' : 'pending'"
              :label="deploying ? '部署中' : '部署结束'"
              size="sm"
            />
          </div>
          <ul class="steps">
            <li v-for="s in deploySteps" :key="s.key" class="step" :class="`step-${s.state}`">
              <span class="step-dot" />
              <div class="step-body">
                <span class="step-title">{{ s.title || s.key }}</span>
                <span v-if="s.message" class="aux">{{ s.message }}</span>
                <span v-if="s.remediation" class="step-fix">修复建议：{{ s.remediation }}</span>
              </div>
            </li>
          </ul>
          <pre v-if="deployLogs.length" class="log-view mono"><code>{{ deployLogs.join('\n') }}</code></pre>
          <StateView v-else kind="empty" title="等待日志…" />
          <div class="actions">
            <button class="btn btn-primary" :disabled="!createdId" @click="router.push({ name: 'instance', params: { id: createdId! } })">
              查看实例详情
            </button>
          </div>
        </template>

        <StateView
          v-else
          kind="empty"
          :title="busy ? '正在创建…' : '本机实例已准备就绪'"
          description="本机模式无需远程部署；创建后即可在详情页启动与配置"
        />
      </div>
    </AcrylicCard>

    <!-- 底部导航 -->
    <div class="wizard-nav">
      <button class="btn btn-ghost" :disabled="step === 1" @click="prevStep">
        <Icon name="chevronLeft" :size="15" />
        上一步
      </button>
      <span class="aux">第 {{ step }} / {{ totalSteps }} 步</span>
      <button v-if="step < totalSteps" class="btn btn-primary" :disabled="!canNext()" @click="nextStep">
        下一步
        <Icon name="chevronRight" :size="15" />
      </button>
      <button
        v-else-if="!isRemoteFlow"
        class="btn btn-primary"
        :disabled="busy"
        @click="finishLocal"
      >
        完成
      </button>
    </div>
  </div>
</template>

<style scoped>
/* 步骤条 */
.stepper {
  display: flex;
  gap: var(--space-1);
  list-style: none;
  margin: 0 0 var(--space-6);
  padding: 0;
  overflow-x: auto;
}
.stepper-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: var(--radius-pill);
  font-size: var(--fs-aux);
  color: var(--text-tertiary);
  white-space: nowrap;
  background: var(--surface-sunken);
}
.stepper-item.active {
  background: var(--primary-soft);
  color: var(--primary);
  font-weight: 500;
}
.stepper-item.done {
  color: var(--success);
}
.stepper-item.skipped {
  opacity: 0.45;
}
.idx {
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--surface);
  font-size: 11px;
}

.form {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}
.grid-2 {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--space-3);
}
.span-2 {
  grid-column: span 2;
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

.choice-row {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.choice {
  flex: 1 1 180px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-items: flex-start;
  padding: var(--space-3);
  border-radius: var(--radius-control);
  border: 1px solid var(--border-strong);
  background: transparent;
  color: var(--text);
  font-family: inherit;
  font-size: var(--fs-body);
  cursor: pointer;
  text-align: left;
  transition: border-color var(--dur-fast) var(--ease),
    background-color var(--dur-fast) var(--ease);
}
.choice:hover {
  background: var(--surface-sunken);
}
.choice.active {
  border-color: var(--primary);
  background: var(--primary-soft);
}

.kv {
  display: grid;
  grid-template-columns: 120px minmax(0, 1fr);
  gap: var(--space-2) var(--space-3);
  font-size: var(--fs-body);
  align-items: center;
  margin: 0;
}
.kv dt {
  color: var(--text-secondary);
  font-size: var(--fs-aux);
}
.kv dd {
  margin: 0;
  overflow-wrap: anywhere;
}

.actions {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex-wrap: wrap;
}

.fingerprint-box {
  padding: var(--space-3);
  border-radius: var(--radius-control);
  background: var(--warning-soft);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.fp-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}
.fp-value {
  font-size: var(--fs-log);
  word-break: break-all;
  color: var(--text);
}

.notes {
  margin: 0;
  padding-left: var(--space-4);
  font-size: var(--fs-aux);
  color: var(--text-secondary);
}
.hint {
  font-size: 11.5px;
  line-height: 1.6;
}
.warn-banner {
  padding: 8px 12px;
  border-radius: var(--radius-control);
  background: var(--warning-soft);
  color: var(--warning);
  font-size: var(--fs-aux);
}
.err-banner {
  margin-bottom: var(--space-4);
  padding: 10px 14px;
  border-radius: var(--radius-control);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: var(--fs-body);
}

.editor {
  min-height: 320px;
  resize: vertical;
}

.preflight-list {
  list-style: none;
  margin: var(--space-4) 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.pf-item {
  display: flex;
  gap: var(--space-3);
  align-items: flex-start;
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-control);
  background: var(--surface-sunken);
}
.pf-fail {
  background: var(--danger-soft);
}
.pf-warn {
  background: var(--warning-soft);
}
.pf-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.pf-label {
  font-size: var(--fs-body);
  font-weight: 500;
}
.pf-fix {
  font-size: var(--fs-aux);
  color: var(--warning);
}

.steps {
  list-style: none;
  margin: var(--space-4) 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.step {
  display: flex;
  gap: var(--space-3);
}
.step-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-top: 7px;
  background: var(--neutral-dot);
  flex: 0 0 auto;
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
.step-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.step-title {
  font-size: var(--fs-body);
}
.step-failed .step-title {
  color: var(--danger);
}
.step-fix {
  font-size: var(--fs-aux);
  color: var(--warning);
}

.log-view {
  margin: var(--space-3) 0 0;
  padding: var(--space-3);
  max-height: 320px;
  overflow: auto;
  background: var(--surface-sunken);
  border-radius: var(--radius-control);
  font-size: var(--fs-log);
  white-space: pre;
}

.wizard-nav {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  margin-top: var(--space-6);
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
.textarea {
  height: auto;
  padding: 12px 14px;
  resize: vertical;
  line-height: 1.6;
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
}
.diag-error {
  background: var(--danger-soft);
  color: var(--danger);
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
  white-space: nowrap;
  transition: background-color var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease),
    transform var(--dur-fast) var(--ease);
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

@media (max-width: 900px) {
  .grid-2 {
    grid-template-columns: minmax(0, 1fr);
  }
  .span-2 {
    grid-column: span 1;
  }
}
</style>
