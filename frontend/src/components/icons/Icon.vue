<script lang="ts">
export default { inheritAttrs: false }
</script>

<script setup lang="ts">
/**
 * 统一图标组件：`<Icon name="play" />`。
 *
 * 内部用 Morphicons 的 `MorphIcon` 渲染 Lucide 数据；`name` 变化时自动做弹簧变形，
 * 因此像「播放 ↔ 停止」「显示 ↔ 隐藏」这类状态切换天然有过渡，无需额外代码。
 */
import { computed } from 'vue'
import { MorphIcon, icons, type IconName } from './index'

const props = withDefaults(
  defineProps<{
    name: IconName
    size?: number
    strokeWidth?: number
    /** 有 label 时是语义图标（role=img）；否则装饰性（aria-hidden） */
    label?: string
    /** 弹簧手感 */
    spring?: 'snappy' | 'smooth' | 'bouncy'
    reducedMotion?: 'never' | 'user' | 'always'
    /** 旋转动画（用于 loading） */
    spin?: boolean
  }>(),
  {
    size: 16,
    strokeWidth: 1.75,
    spring: 'snappy',
    reducedMotion: 'user',
    spin: false,
  },
)

const icon = computed(() => icons[props.name])
</script>

<template>
  <span class="icon" :class="{ spin }" :style="{ width: `${size}px`, height: `${size}px` }">
    <MorphIcon
      :icon="icon"
      :size="size"
      :stroke-width="strokeWidth"
      :spring="spring"
      :reduced-motion="reducedMotion"
      :label="label"
      v-bind="$attrs"
    />
  </span>
</template>

<style scoped>
.icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 auto;
  color: currentColor;
  line-height: 0;
}
.icon :deep(svg) {
  display: block;
}
.spin {
  animation: icon-spin 1s linear infinite;
}
@keyframes icon-spin {
  to {
    transform: rotate(360deg);
  }
}
@media (prefers-reduced-motion: reduce) {
  .spin {
    animation: none;
  }
}
</style>
