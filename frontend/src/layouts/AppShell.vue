<script setup lang="ts">
/**
 * 应用外壳：左侧固定导航 + 顶部工具栏 + 内容区。
 *
 * 交互约定：导航仅做颜色与背景的即时反馈，不播放按压/光晕动画。
 */
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useSessionStore } from '@/stores/session'
import { useThemeStore } from '@/stores/theme'
import StateBadge from '@/components/common/StateBadge.vue'
import Icon from '@/components/icons/Icon.vue'
import type { IconName } from '@/components/icons'
import brandLogo from '@/assets/logo/frppanel-mark.png'

const route = useRoute()
const router = useRouter()
const session = useSessionStore()
const theme = useThemeStore()

interface NavItem {
  name: string
  label: string
  path: string
  /** 图标名（统一走 Morphicons + Lucide） */
  icon: IconName
}

const navItems: NavItem[] = [
  { name: 'instances', label: '实例', path: '/instances', icon: 'server' },
  { name: 'alerts', label: '告警通知', path: '/alerts', icon: 'activity' },
  { name: 'users', label: '用户管理', path: '/users', icon: 'users' },
  { name: 'audit', label: '审计日志', path: '/audit', icon: 'audit' },
  { name: 'settings', label: '设置', path: '/settings', icon: 'settings' },
]

const activeName = computed(() => (route.name as string) ?? '')
const isActive = (name: string) =>
  activeName.value === name || activeName.value.startsWith(name)

const pageTitle = computed(() => {
  const map: Record<string, string> = {
    instances: '实例',
    instance: '实例详情',
    'instance-new': '新建实例',
    alerts: '告警通知',
    users: '用户管理',
    audit: '审计日志',
    settings: '设置',
  }
  return map[activeName.value] ?? 'FrpPanel'
})

/** 点击导航：直接跳转，不再播放按压/光晕动画。 */
function onNavClick(item: NavItem) {
  router.push(item.path)
}

const contentKey = computed(() => route.fullPath)

async function logout() {
  await session.logout()
  router.push({ name: 'login' })
}
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <RouterLink to="/instances" class="brand" aria-label="FrpPanel 首页">
        <img :src="brandLogo" :width="30" :height="30" alt="FrpPanel" class="brand-logo" />
        <div class="brand-text">
          <strong>FrpPanel</strong>
          <span class="aux">NFrp / frp 管理面板</span>
        </div>
      </RouterLink>

      <nav class="nav" aria-label="主导航">
        <RouterLink
          v-for="item in navItems"
          :key="item.name"
          :to="item.path"
          class="nav-item"
          :class="{ active: isActive(item.name) }"
          :aria-current="isActive(item.name) ? 'page' : undefined"
          @click.prevent="onNavClick(item)"
        >
          <span class="nav-icon">
            <Icon :name="item.icon" :size="17" />
          </span>
          <span class="nav-label">{{ item.label }}</span>
          <span class="nav-bar" aria-hidden="true" />
        </RouterLink>
      </nav>

      <div class="sidebar-foot">
        <div class="who">
          <div class="who-name">{{ session.user?.username ?? '未登录' }}</div>
          <StateBadge
            :state="session.user?.role === 'admin' ? 'ok' : 'info'"
            :label="session.user?.role === 'admin' ? '管理员' : '只读'"
            size="sm"
          />
        </div>
      </div>
    </aside>

    <div class="main">
      <header class="topbar acrylic">
        <h1 class="topbar-title">{{ pageTitle }}</h1>
        <div class="topbar-actions">
          <button
            class="icon-btn"
            :title="theme.isDark ? '切换到浅色' : '切换到深色'"
            :aria-label="theme.isDark ? '切换到浅色主题' : '切换到深色主题'"
            @click="theme.toggle()"
          >
            <Icon :name="theme.isDark ? 'moon' : 'sun'" :size="17" />
          </button>
          <button class="text-btn" @click="logout">
            <Icon name="logout" :size="15" />
            退出
          </button>
        </div>
      </header>

      <main class="content">
        <RouterView v-slot="{ Component }">
          <Transition name="page" mode="out-in">
            <component :is="Component" :key="contentKey" />
          </Transition>
        </RouterView>
      </main>
    </div>
  </div>
</template>

<style scoped>
.shell {
  display: grid;
  grid-template-columns: var(--sidebar-w) minmax(0, 1fr);
  min-height: 100vh;
}

/* ---- 侧栏 ---- */
.sidebar {
  display: flex;
  flex-direction: column;
  gap: var(--space-8);
  padding: var(--space-8) var(--space-4);
  position: sticky;
  top: 0;
  height: 100vh;
}

.brand {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: 0 var(--space-3);
  text-decoration: none;
  color: inherit;
  border-radius: var(--radius-control);
}
.brand:hover {
  text-decoration: none;
}
.brand-logo {
  flex: 0 0 auto;
}

.brand-text {
  display: flex;
  flex-direction: column;
  line-height: 1.3;
  min-width: 0;
}
.brand-text strong {
  font-size: var(--fs-body);
}
.brand-text .aux {
  font-family: var(--font-mono);
  font-size: 10.5px;
  letter-spacing: 0.01em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.nav-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: 11px var(--space-3);
  border-radius: var(--radius-control);
  color: var(--text-secondary);
  font-size: var(--fs-body);
  text-decoration: none;
  overflow: hidden;
  transition: background-color var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease), transform var(--dur-fast) var(--ease);
  -webkit-tap-highlight-color: transparent;
}
.nav-item:hover {
  background: var(--surface-sunken);
  color: var(--text);
  text-decoration: none;
}
.nav-item.active {
  background: var(--primary-soft);
  color: var(--primary);
  font-weight: 500;
}

.nav-icon {
  display: grid;
  place-items: center;
}
.nav-item.active .nav-icon {
  color: var(--primary);
}
.nav-label {
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  letter-spacing: 0.01em;
}

/* 选中条：从中间向上下展开 */
.nav-bar {
  position: absolute;
  left: 0;
  top: 50%;
  width: 2px;
  height: 0;
  border-radius: var(--radius-pill);
  background: var(--primary);
  transform: translateY(-50%);
  transition: height var(--dur-slow) var(--ease), opacity var(--dur-base) var(--ease);
  opacity: 0;
}
.nav-item.active .nav-bar {
  height: 60%;
  opacity: 1;
}

.sidebar-foot {
  margin-top: auto;
  padding: var(--space-3) var(--space-2);
  border-top: 1px solid var(--divider);
}

.who {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}
.who-name {
  font-size: var(--fs-body);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ---- 主区 ---- */
.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.topbar {
  position: sticky;
  top: 0;
  z-index: 10;
  height: var(--topbar-h);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: 0 var(--space-8);
  background: rgba(249, 249, 251, 0.78);
  backdrop-filter: var(--acrylic-blur);
  -webkit-backdrop-filter: var(--acrylic-blur);
  border-radius: 0;
  border: none;
  box-shadow: none;
}

[data-theme='dark'] .topbar {
  background: rgba(20, 22, 26, 0.72);
}

.topbar-title {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--fs-section);
  font-weight: 600;
  color: var(--text);
}
.topbar-title::before {
  content: '>';
  font-family: var(--font-mono);
  font-weight: 600;
  color: var(--primary);
}

.topbar-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.icon-btn {
  display: grid;
  place-items: center;
  width: 36px;
  height: 36px;
  border-radius: var(--radius-control);
  border: none;
  background: var(--surface);
  box-shadow: var(--shadow-1);
  color: var(--text-secondary);
  cursor: pointer;
  transition: background-color var(--dur-fast) var(--ease),
    color var(--dur-fast) var(--ease), transform var(--dur-fast) var(--ease);
}
.icon-btn:hover {
  background: var(--primary-soft);
  color: var(--primary);
}
.icon-btn:active {
  transform: scale(0.94);
}

.text-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 36px;
  padding: 0 14px;
  border-radius: var(--radius-control);
  border: none;
  background: var(--surface);
  box-shadow: var(--shadow-1);
  color: var(--text-secondary);
  font-size: var(--fs-body);
  font-family: inherit;
  cursor: pointer;
  transition: background-color var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
}
.text-btn:hover {
  background: var(--primary-soft);
  color: var(--primary);
}

.content {
  flex: 1 1 auto;
  min-width: 0;
}

/* 页面切换：轻微上移淡入 */
.page-enter-active {
  transition: opacity 140ms var(--ease), transform 140ms var(--ease);
}
.page-leave-active {
  transition: opacity 90ms var(--ease);
}
.page-enter-from {
  opacity: 0;
  transform: translateY(4px);
}
.page-leave-to {
  opacity: 0;
}

@media (max-width: 900px) {
  .shell {
    grid-template-columns: 64px minmax(0, 1fr);
  }
  .brand-text,
  .nav-label,
  .sidebar-foot .who {
    display: none;
  }
  .nav-item {
    justify-content: center;
  }
}

@media (prefers-reduced-motion: reduce) {
  .page-enter-from {
    transform: none;
  }
}
</style>
