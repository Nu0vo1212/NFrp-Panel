<script setup lang="ts">
/** 登录页：居中亚克力卡片，错误提示清晰，无花哨动效。 */
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useSessionStore } from '@/stores/session'
import { useThemeStore } from '@/stores/theme'
import Icon from '@/components/icons/Icon.vue'
import brandLogo from '@/assets/logo/frppanel-mark.png'

const router = useRouter()
const route = useRoute()
const session = useSessionStore()
const theme = useThemeStore()

const username = ref('admin')
const password = ref('')
const showPassword = ref(false)
const localError = ref<string | null>(null)

const canSubmit = computed(
  () => username.value.trim().length > 0 && password.value.length > 0 && !session.loading,
)

async function submit() {
  localError.value = null
  if (!canSubmit.value) return
  try {
    await session.login(username.value.trim(), password.value)
    const redirect = (route.query.redirect as string) || '/instances'
    router.push(redirect)
  } catch (e) {
    localError.value = e instanceof Error ? e.message : '登录失败'
  }
}
</script>

<template>
  <div class="login-wrap">
    <button
      class="theme-toggle"
      :aria-label="theme.isDark ? '切换到浅色主题' : '切换到深色主题'"
      @click="theme.toggle()"
    >
      {{ theme.isDark ? '浅色' : '深色' }}
    </button>

    <div class="acrylic login-card">
      <div class="head">
        <img :src="brandLogo" :width="40" :height="40" alt="FrpPanel" />
        <div>
          <h1 class="title">FrpPanel</h1>
          <p class="aux">NFrp / 官方 frp 管理面板</p>
        </div>
      </div>

      <form class="form" @submit.prevent="submit">
        <label class="field">
          <span class="label">用户名</span>
          <input
            v-model="username"
            class="input"
            type="text"
            autocomplete="username"
            spellcheck="false"
            :disabled="session.loading"
          />
        </label>

        <label class="field">
          <span class="label">密码</span>
          <div class="input-group">
            <input
              v-model="password"
              class="input"
              :type="showPassword ? 'text' : 'password'"
              autocomplete="current-password"
              :disabled="session.loading"
            />
            <button
              type="button"
              class="reveal"
              @click="showPassword = !showPassword"
              :aria-label="showPassword ? '隐藏密码' : '显示密码'"
              :title="showPassword ? '隐藏密码' : '显示密码'"
            >
              <Icon :name="showPassword ? 'eyeOff' : 'eye'" :size="16" spring="smooth" />
            </button>
          </div>
        </label>

        <p v-if="localError" class="error" role="alert">{{ localError }}</p>

        <button class="submit" type="submit" :disabled="!canSubmit">
          {{ session.loading ? '登录中…' : '登录' }}
        </button>
      </form>

      <p class="foot aux">
        首次启动的管理员口令保存在后端 <code class="mono">data/admin.initial</code>，
        登录后请立即修改并删除该文件。
      </p>
    </div>
  </div>
</template>

<style scoped>
.login-wrap {
  min-height: 100vh;
  display: grid;
  place-items: center;
  padding: var(--space-6);
  position: relative;
}

.theme-toggle {
  position: fixed;
  top: var(--space-4);
  right: var(--space-4);
  padding: 6px 12px;
  border-radius: var(--radius-control);
  border: 1px solid var(--border-strong);
  background: var(--surface);
  backdrop-filter: var(--acrylic-blur);
  color: var(--text-secondary);
  font-size: var(--fs-aux);
  font-family: inherit;
  cursor: pointer;
}

.login-card {
  width: 100%;
  max-width: 400px;
  padding: var(--space-10) var(--space-8) var(--space-8);
}

.head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-6);
}


.title {
  font-size: var(--fs-title);
  font-weight: 600;
  letter-spacing: -0.01em;
}

.form {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.label {
  font-size: var(--fs-aux);
  color: var(--text-secondary);
}

.input-group {
  display: flex;
  gap: var(--space-2);
}

.input {
  flex: 1 1 auto;
  min-width: 0;
  height: 42px;
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
.input:disabled {
  opacity: 0.6;
}

.reveal :deep(.icon) {
  margin: 0 auto;
}

.reveal {
  flex: 0 0 auto;
  padding: 0 12px;
  height: 38px;
  border-radius: var(--radius-control);
  border: 1px solid var(--border-strong);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--fs-aux);
  font-family: inherit;
  cursor: pointer;
}
.reveal:hover {
  background: var(--surface-sunken);
}

.error {
  padding: 8px 12px;
  border-radius: var(--radius-control);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: var(--fs-aux);
  border: 1px solid transparent;
}

.submit {
  height: 44px;
  margin-top: var(--space-1);
  border: none;
  border-radius: var(--radius-control);
  background: var(--primary);
  color: #fff;
  font-size: var(--fs-body);
  font-weight: 600;
  font-family: inherit;
  cursor: pointer;
  box-shadow: var(--primary-shadow);
  transition: background-color var(--dur-fast) var(--ease),
    transform var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease);
}
.submit:hover:not(:disabled) {
  background: var(--primary-hover);
  transform: translateY(-1px);
}
.submit:active:not(:disabled) {
  background: var(--primary-active);
}
.submit:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.foot {
  margin-top: var(--space-5);
  padding-top: var(--space-4);
  border-top: 1px solid var(--divider);
  font-size: 11.5px;
  line-height: 1.6;
}

code.mono {
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--surface-sunken);
  font-size: 11px;
}
</style>
