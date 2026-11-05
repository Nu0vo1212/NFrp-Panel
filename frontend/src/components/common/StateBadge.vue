<script setup lang="ts">
/** 状态点 + 文案徽标。颜色只表达状态，不做大面积铺色。 */
import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    state: string
    label?: string
    size?: 'sm' | 'md'
    pulse?: boolean
  }>(),
  { size: 'md', pulse: false },
)

/** 把各驱动的状态字符串归一到四种视觉语义。 */
const tone = computed(() => {
  const s = props.state
  if (
    [
      'running',
      'ok',
      'healthy',
      'success',
      'deployed',
      'pass',
      'active',
      'done',
      'hot',
      'enabled',
    ].includes(s)
  )
    return 'success'
  if (['stopped', 'unknown', 'skipped', 'offline', 'none', 'pending'].includes(s)) return 'neutral'
  if (
    ['warning', 'warn', 'restart_required', 'waiting', 'rolled_back', 'deploying'].includes(s)
  )
    return 'warning'
  if (
    ['error',
      'failed',
      'fail',
      'start_error',
      'unhealthy',
      'denied',
      'failure',
      'unsupported',
    ].includes(s)
  )
    return 'danger'
  if (['info', 'dynamic', 'ssh', 'config_reload', 'admin_api'].includes(s)) return 'info'
  return 'neutral'
})

const text = computed(() => {
  if (props.label) return props.label
  const map: Record<string, string> = {
    running: '运行中',
    stopped: '已停止',
    unknown: '未知',
    error: '异常',
    deployed: '已部署',
    not_deployed: '未部署',
    failed: '失败',
    rolled_back: '已回滚',
    healthy: '健康',
    unhealthy: '不健康',
    waiting: '等待中',
    start_error: '启动失败',
    offline: '离线',
    pass: '通过',
    warn: '警告',
    fail: '失败',
    skipped: '跳过',
    done: '完成',
    pending: '等待',
    hot: '已热加载',
    restart_required: '需重启',
    restarted: '已重启',
    unsupported: '不支持',
  }
  return map[props.state] ?? props.state
})
</script>

<template>
  <span class="badge" :class="[`tone-${tone}`, `size-${size}`]">
    <span class="dot" :class="[`dot--${tone}`, { 'dot--pulse': pulse && tone === 'success' }]" />
    <span class="label">{{ text }}</span>
  </span>
</template>

<style scoped>
.badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  font-size: var(--fs-aux);
  line-height: 1.6;
  white-space: nowrap;
  border: 1px solid transparent;
}

.size-md {
  font-size: var(--fs-body);
  padding: 3px 10px;
}

.tone-success {
  color: var(--success);
  background: var(--success-soft);
}
.tone-warning {
  color: var(--warning);
  background: var(--warning-soft);
}
.tone-danger {
  color: var(--danger);
  background: var(--danger-soft);
}
.tone-info {
  color: var(--info);
  background: var(--info-soft);
}
.tone-neutral {
  color: var(--text-secondary);
  background: var(--surface-sunken);
}

.dot--success {
  background: var(--success);
}
.dot--warning {
  background: var(--warning);
}
.dot--danger {
  background: var(--danger);
}
.dot--info {
  background: var(--info);
}
.dot--neutral {
  background: var(--neutral-dot);
}

.label {
  font-weight: 500;
}
</style>
