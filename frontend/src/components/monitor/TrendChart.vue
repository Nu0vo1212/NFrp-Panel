<script setup lang="ts">
/** ECharts 封装：与主题一致的低饱和色板，弱化网格线，去掉多余边框/阴影。 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import * as echarts from 'echarts/core'
import { LineChart } from 'echarts/charts'
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
} from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import { useThemeStore } from '@/stores/theme'

echarts.use([
  LineChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  CanvasRenderer,
])

export interface SeriesInput {
  name: string
  points: [number, number][]
  color: string
  /** 是否画成面积（流量常用） */
  area?: boolean
  /** 数值格式化方式 */
  format?: 'bytes' | 'number'
}

const props = withDefaults(
  defineProps<{
    series: SeriesInput[]
    height?: number
    yFormat?: 'bytes' | 'number'
    emptyText?: string
  }>(),
  { height: 260, yFormat: 'number', emptyText: '暂无数据' },
)

const theme = useThemeStore()
const el = ref<HTMLDivElement | null>(null)
let chart: echarts.ECharts | null = null

const isEmpty = computed(() => !props.series.some((s) => s.points.length > 0))

function cssVar(name: string, fallback: string) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}

function fmtBytes(v: number): string {
  if (v < 1024) return `${v} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let n = v / 1024
  let i = 0
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024
    i++
  }
  return `${n.toFixed(n >= 10 ? 0 : 1)} ${units[i]}`
}

function buildOption(): echarts.EChartsCoreOption {
  const textColor = cssVar('--text-secondary', '#5b616e')
  const gridColor = cssVar('--divider', 'rgba(31,35,40,0.08)')
  const axisColor = cssVar('--border-strong', 'rgba(31,35,40,0.12)')

  const usesBytes = props.series.some((s) => s.format === 'bytes')

  return {
    animationDuration: 180,
    grid: { left: 8, right: 12, top: props.series.length > 1 ? 34 : 12, bottom: 6, containLabel: true },
    tooltip: {
      trigger: 'axis',
      backgroundColor: cssVar('--surface-solid', '#fff'),
      borderColor: gridColor,
      borderWidth: 1,
      textStyle: { color: cssVar('--text', '#1f2328'), fontSize: 12 },
      axisPointer: { type: 'line', lineStyle: { color: axisColor } },
      valueFormatter: (v: unknown) =>
        usesBytes ? fmtBytes(Number(v)) : String(Number(v)),
    },
    legend:
      props.series.length > 1
        ? {
            top: 0,
            right: 0,
            itemWidth: 10,
            itemHeight: 10,
            icon: 'roundRect',
            textStyle: { color: textColor, fontSize: 12 },
          }
        : undefined,
    xAxis: {
      type: 'time',
      axisLine: { lineStyle: { color: axisColor } },
      axisTick: { show: false },
      axisLabel: { color: textColor, fontSize: 11, hideOverlap: true },
      splitLine: { show: false },
    },
    yAxis: {
      type: 'value',
      minInterval: usesBytes ? undefined : 1,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: textColor,
        fontSize: 11,
        formatter: (v: number) => (usesBytes ? fmtBytes(v) : String(v)),
      },
      splitLine: { lineStyle: { color: gridColor } },
    },
    series: props.series.map((s) => ({
      name: s.name,
      type: 'line',
      showSymbol: false,
      smooth: 0.25,
      lineStyle: { width: 2, color: s.color },
      itemStyle: { color: s.color },
      areaStyle: s.area
        ? {
            color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
              { offset: 0, color: hexA(s.color, 0.22) },
              { offset: 1, color: hexA(s.color, 0) },
            ]),
          }
        : undefined,
      data: s.points.map(([ts, v]) => [ts * 1000, v]),
    })),
  }
}

function hexA(color: string, alpha: number): string {
  // 支持 #rrggbb
  if (/^#[0-9a-f]{6}$/i.test(color)) {
    const r = parseInt(color.slice(1, 3), 16)
    const g = parseInt(color.slice(3, 5), 16)
    const b = parseInt(color.slice(5, 7), 16)
    return `rgba(${r},${g},${b},${alpha})`
  }
  return color
}

function render() {
  if (!el.value) return
  if (!chart) chart = echarts.init(el.value, undefined, { renderer: 'canvas' })
  chart.setOption(buildOption(), true)
}

function onResize() {
  chart?.resize()
}

onMounted(() => {
  render()
  window.addEventListener('resize', onResize)
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', onResize)
  chart?.dispose()
  chart = null
})

watch(() => props.series, render, { deep: true })
// 主题切换时重建（颜色来自 CSS 变量）
watch(() => theme.mode, () => {
  chart?.dispose()
  chart = null
  render()
})
</script>

<template>
  <div class="chart-wrap" :style="{ height: `${height}px` }">
    <div ref="el" class="chart" :class="{ hidden: isEmpty }" />
    <div v-if="isEmpty" class="empty">
      <span class="aux">{{ emptyText }}</span>
    </div>
  </div>
</template>

<style scoped>
.chart-wrap {
  position: relative;
  width: 100%;
}
.chart {
  width: 100%;
  height: 100%;
}
.chart.hidden {
  visibility: hidden;
}
.empty {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
}
</style>
