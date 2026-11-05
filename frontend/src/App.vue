<script setup lang="ts">
import { onMounted } from 'vue'
import { useSessionStore } from '@/stores/session'
import { useThemeStore } from '@/stores/theme'

const session = useSessionStore()
const theme = useThemeStore()

onMounted(async () => {
  theme.apply(theme.mode)
  // 有 token 时恢复会话（失败则静默，路由守卫会引导到登录）
  if (session.isAuthenticated) {
    try {
      await session.fetchMe()
    } catch {
      /* 交给 401 拦截与路由守卫 */
    }
  }
})
</script>

<template>
  <RouterView />
</template>
