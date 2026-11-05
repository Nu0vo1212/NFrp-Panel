<script setup lang="ts">
/**
 * 亚克力卡片：半透明 + 模糊 + 细边框 + 柔和阴影。
 * 不支持 backdrop-filter 时由 tokens.css 的 @supports 降级为不透明背景。
 */
withDefaults(
  defineProps<{
    title?: string
    subtitle?: string
    /** 右侧操作区是否与内容同排 */
    padding?: 'none' | 'sm' | 'md' | 'lg'
    flat?: boolean
    raised?: boolean
  }>(),
  { padding: 'md', flat: false, raised: false },
)
</script>

<template>
  <section
    class="acrylic card"
    :class="[{ 'acrylic--flat': flat, 'acrylic--raised': raised }, `pad-${padding}`]"
  >
    <header v-if="title || $slots.actions" class="card-head">
      <div class="card-titles">
        <h3 v-if="title" class="card-title">{{ title }}</h3>
        <p v-if="subtitle" class="card-sub">{{ subtitle }}</p>
      </div>
      <div v-if="$slots.actions" class="card-actions">
        <slot name="actions" />
      </div>
    </header>
    <div class="card-body">
      <slot />
    </div>
    <footer v-if="$slots.footer" class="card-foot">
      <slot name="footer" />
    </footer>
  </section>
</template>

<style scoped>
.card {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.pad-none {
  padding: 0;
}
.pad-sm {
  padding: var(--space-4);
}
.pad-md {
  padding: var(--space-6);
}
.pad-lg {
  padding: var(--space-8);
}

.card-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
  margin-bottom: var(--space-5);
}

.card-titles {
  min-width: 0;
}

.card-title {
  font-size: var(--fs-section);
  font-weight: 600;
  color: var(--text);
}

.card-sub {
  margin-top: var(--space-1);
  font-size: var(--fs-aux);
  color: var(--text-secondary);
}

.card-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 0 0 auto;
}

.card-body {
  flex: 1 1 auto;
  min-width: 0;
}

.card-foot {
  margin-top: var(--space-4);
  padding-top: var(--space-3);
  border-top: 1px solid var(--divider);
}
</style>
