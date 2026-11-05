/** 主题状态：浅色/深色，持久化到 localStorage（仅主题，不含任何敏感数据）。 */

import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'

export type ThemeMode = 'light' | 'dark'

const KEY = 'frppanel.theme'

/**
 * 初始主题。
 *
 * ★ UI 规范明确「默认浅色」，所以这里**不跟随系统深色偏好**：
 *   用户显式切换过（localStorage 有记录）才用其选择，否则一律浅色。
 */
function initial(): ThemeMode {
  const saved = localStorage.getItem(KEY)
  if (saved === 'light' || saved === 'dark') return saved
  return 'light'
}

export const useThemeStore = defineStore('theme', () => {
  const mode = ref<ThemeMode>(initial())

  const isDark = computed(() => mode.value === 'dark')

  function apply(m: ThemeMode) {
    document.documentElement.setAttribute('data-theme', m)
    document.documentElement.style.colorScheme = m
  }

  function set(m: ThemeMode) {
    mode.value = m
  }

  function toggle() {
    set(mode.value === 'dark' ? 'light' : 'dark')
  }

  watch(
    mode,
    (m) => {
      localStorage.setItem(KEY, m)
      apply(m)
    },
    { immediate: true },
  )

  return { mode, isDark, set, toggle, apply }
})
