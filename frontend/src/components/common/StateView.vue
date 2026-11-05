<script setup lang="ts">
/**
 * 统一的空态 / 错误态 / 无权限态 / 加载骨架。
 * 不使用花哨插画：一个简洁的图标位 + 一句话 + 一个动作。
 */
import { computed } from 'vue'
import Icon from '@/components/icons/Icon.vue'
import type { IconName } from '@/components/icons'

const props = withDefaults(
  defineProps<{
    kind?: 'empty' | 'error' | 'forbidden' | 'loading'
    title?: string
    description?: string
    /** 骨架行数（kind=loading 时生效） */
    rows?: number
  }>(),
  { kind: 'empty', rows: 4 },
)

/** 状态对应的图标（Morphicons + Lucide） */
const glyphIcon = computed<IconName>(() => {
  const map: Record<string, IconName> = {
    empty: 'layers',
    error: 'warning',
    forbidden: 'shield',
    loading: 'loading',
  }
  return map[props.kind] ?? 'layers'
})

const fallbackTitle = computed(() => {
  const map: Record<string, string> = {
    empty: '暂无数据',
    error: '加载失败',
    forbidden: '没有权限',
    loading: '加载中',
  }
  return map[props.kind] ?? '暂无数据'
})
</script>

<template>
  <div v-if="kind === 'loading'" class="skeleton-wrap" aria-busy="true" aria-live="polite">
    <div v-for="i in rows" :key="i" class="skeleton-row" :style="{ width: `${100 - i * 7}%` }" />
  </div>

  <div v-else class="state" :class="`state--${kind}`" role="status">
    <div class="glyph" aria-hidden="true">
      <Icon :name="glyphIcon" :size="30" :stroke-width="2.1" />
    </div>
    <p class="title">{{ title ?? fallbackTitle }}</p>
    <p v-if="description" class="desc">{{ description }}</p>
    <div v-if="$slots.default" class="actions"><slot /></div>
  </div>
</template>

<style scoped>
.state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  padding: var(--space-12) var(--space-6) var(--space-12);
  text-align: center;
  color: var(--text-secondary);
}

.glyph {
  display: grid;
  place-items: center;
  width: 64px;
  height: 64px;
  border-radius: var(--radius-card);
  background: var(--primary-soft);
  color: var(--primary);
  margin-bottom: var(--space-3);
}

.state--error .glyph {
  color: var(--danger);
  background: var(--danger-soft);
}

.state--forbidden .glyph {
  color: var(--warning);
  background: var(--warning-soft);
}

.title {
  font-size: 16px;
  font-weight: 600;
  letter-spacing: 0.01em;
  color: var(--text);
}
.title::before {
  content: '// ';
  font-family: var(--font-mono);
  font-weight: 500;
  color: var(--primary);
}

.desc {
  font-size: var(--fs-aux);
  max-width: 46ch;
  line-height: 1.7;
}

.actions {
  margin-top: var(--space-4);
}

/* 骨架 */
.skeleton-wrap {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  padding: var(--space-2) 0;
}

.skeleton-row {
  height: 12px;
  border-radius: var(--radius-pill);
  background: linear-gradient(
    90deg,
    var(--surface-sunken) 25%,
    var(--bg-subtle) 37%,
    var(--surface-sunken) 63%
  );
  background-size: 400% 100%;
  animation: shimmer 1.4s ease infinite;
}

@keyframes shimmer {
  0% {
    background-position: 100% 50%;
  }
  100% {
    background-position: 0 50%;
  }
}
</style>
