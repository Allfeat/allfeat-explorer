<script setup lang="ts">
// Interactive stacked area chart of AFT emission across the first 5 years
// of vesting. Data comes from the static schedule in `tokenSchedule.ts`
// (chain-independent — this is the *plan*, not live state).
//
// Implementation: custom SVG with a fixed viewBox (1100×380) that scales to
// 100% width via the intrinsic SVG aspect ratio. Mouse-tracked crosshair
// maps pointer X back to the nearest month and surfaces per-envelope
// breakdown in a floating tooltip.

import { computed, ref } from 'vue'
import { SCHEDULE, buildSchedule, monthLabel } from '~/utils/tokenSchedule'
import type { EnvelopeId } from '@bindings'

const MONTHS = 60
const VB_W = 1100
const VB_H = 380
const PAD = { l: 64, r: 16, t: 12, b: 46 }
const CHART_W = VB_W - PAD.l - PAD.r
const CHART_H = VB_H - PAD.t - PAD.b
const Y_MAX = 1_000_000_000

const data = computed(() => buildSchedule(MONTHS))

function xFor(month: number): number {
  return PAD.l + (month / MONTHS) * CHART_W
}
function yFor(value: number): number {
  return PAD.t + CHART_H - (value / Y_MAX) * CHART_H
}

// Precompute stacked paths once per schedule change.
const stackedSeries = computed(() => {
  const points = data.value
  const n = points.length
  const cumulative = new Array<number>(n).fill(0)
  const stacks: {
    id: EnvelopeId
    label: string
    color: string
    areaPath: string
  }[] = []

  for (const e of SCHEDULE) {
    const tops: string[] = []
    const bots: string[] = []
    for (let i = 0; i < n; i++) {
      const v = points[i]!.values[e.id] ?? 0
      const bottom = cumulative[i]!
      const top = bottom + v
      const cmd = i === 0 ? 'M' : 'L'
      tops.push(`${cmd} ${xFor(i).toFixed(2)} ${yFor(top).toFixed(2)}`)
      bots.push(`L ${xFor(i).toFixed(2)} ${yFor(bottom).toFixed(2)}`)
      cumulative[i] = top
    }
    const areaPath = `${tops.join(' ')} ${bots.reverse().join(' ')} Z`
    stacks.push({ id: e.id, label: e.label, color: e.color, areaPath })
  }
  return stacks
})

// Hover / crosshair state.
const chartWrap = ref<HTMLDivElement | null>(null)
const hoverMonth = ref<number | null>(null)
const tooltipSide = ref<'left' | 'right'>('right')

function updateHover(clientX: number) {
  const el = chartWrap.value
  if (!el) return
  const rect = el.getBoundingClientRect()
  const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
  const svgX = ratio * VB_W
  const localX = svgX - PAD.l
  const m = Math.round((localX / CHART_W) * MONTHS)
  hoverMonth.value = Math.max(0, Math.min(MONTHS, m))
  // Flip tooltip when the crosshair is in the right 40% of the chart.
  tooltipSide.value = m / MONTHS > 0.6 ? 'left' : 'right'
}
function onMove(ev: MouseEvent) {
  updateHover(ev.clientX)
}
function onTouch(ev: TouchEvent) {
  const t = ev.touches[0]
  if (t) updateHover(t.clientX)
}
function onLeave() {
  hoverMonth.value = null
}

const hoverPoint = computed(() => {
  if (hoverMonth.value === null) return null
  return data.value[hoverMonth.value] ?? null
})

// Breakdown rows for the tooltip, sorted descending by value, non-zero only.
const hoverRows = computed(() => {
  const p = hoverPoint.value
  if (!p) return []
  return SCHEDULE
    .map(e => ({ id: e.id, label: e.label, color: e.color, value: p.values[e.id] ?? 0 }))
    .filter(r => r.value > 0)
    .sort((a, b) => b.value - a.value)
})

// Horizontal tooltip offset in viewBox units relative to crosshair.
const tooltipLeftPct = computed(() => {
  if (hoverMonth.value === null) return 0
  return (xFor(hoverMonth.value) / VB_W) * 100
})

// Y-axis ticks every 100M, x-axis ticks every 6 months.
const Y_TICKS = [0, 1e8, 2e8, 3e8, 4e8, 5e8, 6e8, 7e8, 8e8, 9e8, 1e9]
const X_TICKS = computed(() => {
  const ticks: { month: number; label: string }[] = []
  for (let m = 0; m <= MONTHS; m += 6) {
    ticks.push({ month: m, label: monthLabel(m) })
  }
  return ticks
})

function fmtM(v: number): string {
  if (v >= 1e9) return `${+(v / 1e9).toFixed(1)}B`
  if (v >= 1e7) return `${Math.round(v / 1e6)}M`
  if (v >= 1e6) return `${+(v / 1e6).toFixed(1)}M`
  if (v >= 1e3) return `${Math.round(v / 1e3)}K`
  return Math.round(v).toString()
}

// Group the legend for readability (mirrors the report figure).
const legendGroups = computed(() => {
  const groups: Record<string, typeof SCHEDULE[number][]> = {
    'team-advisory': [],
    'public-sale': [],
    'operations-ecosystem': [],
  }
  for (const e of SCHEDULE) {
    groups[e.group]!.push(e)
  }
  return [
    { id: 'team-advisory', label: 'Team & Advisory', items: groups['team-advisory']! },
    { id: 'public-sale', label: 'Public Sale', items: groups['public-sale']! },
    { id: 'operations-ecosystem', label: 'Ecosystem & Ops', items: groups['operations-ecosystem']! },
  ]
})
</script>

<template>
  <div class="emission-chart">
    <div class="emission-head">
      <div>
        <div class="ui-label">Token emission schedule</div>
        <h3 class="emission-title">60-month unlock curve</h3>
      </div>
      <div v-if="hoverPoint" class="emission-cursor mono">
        {{ monthLabel(hoverPoint.month) }} · <strong>{{ fmtM(hoverPoint.total) }} AFT</strong>
        <span class="dim">({{ fmtPct((hoverPoint.total / 1e9) * 100) }})</span>
      </div>
      <div v-else class="emission-cursor mono dim">
        Hover the chart to inspect any month
      </div>
    </div>

    <div
      ref="chartWrap"
      class="chart-wrap"
      @mousemove="onMove"
      @mouseleave="onLeave"
      @touchstart.passive="onTouch"
      @touchmove.passive="onTouch"
      @touchend="onLeave"
    >
      <svg
        :viewBox="`0 0 ${VB_W} ${VB_H}`"
        class="chart-svg"
        aria-hidden="true"
      >
        <!-- Y grid lines -->
        <g class="grid">
          <line
            v-for="t in Y_TICKS" :key="t"
            :x1="PAD.l" :y1="yFor(t)"
            :x2="VB_W - PAD.r" :y2="yFor(t)"
          />
        </g>

        <!-- Stacked areas -->
        <g class="series">
          <path
            v-for="s in stackedSeries" :key="s.id"
            :d="s.areaPath"
            :fill="s.color"
            :class="['stack', { faded: hoverMonth !== null }]"
          />
        </g>

        <!-- Hover crosshair -->
        <g v-if="hoverMonth !== null && hoverPoint" class="crosshair">
          <line
            :x1="xFor(hoverMonth)" :y1="PAD.t"
            :x2="xFor(hoverMonth)" :y2="VB_H - PAD.b"
          />
          <circle
            :cx="xFor(hoverMonth)" :cy="yFor(hoverPoint.total)"
            r="4"
          />
        </g>

        <!-- Y axis labels -->
        <g class="y-axis">
          <text
            v-for="t in Y_TICKS" :key="`y-${t}`"
            :x="PAD.l - 10" :y="yFor(t)"
            text-anchor="end" dominant-baseline="middle"
          >{{ fmtM(t) }}</text>
        </g>

        <!-- X axis labels -->
        <g class="x-axis">
          <text
            v-for="t in X_TICKS" :key="`x-${t.month}`"
            :x="xFor(t.month)" :y="VB_H - PAD.b + 20"
            text-anchor="middle"
          >{{ t.label }}</text>
          <text
            v-for="t in X_TICKS" :key="`xm-${t.month}`"
            :x="xFor(t.month)" :y="VB_H - PAD.b + 36"
            text-anchor="middle"
            class="tick-month"
          >{{ t.month }}</text>
        </g>
      </svg>

      <!-- Tooltip overlay -->
      <div
        v-if="hoverPoint"
        class="tooltip"
        :class="[`tooltip--${tooltipSide}`]"
        :style="{ left: `${tooltipLeftPct}%` }"
      >
        <div class="tooltip-head">
          <span class="mono">{{ monthLabel(hoverPoint.month) }}</span>
          <span class="dim text-xs">t+{{ hoverPoint.month }} mo</span>
        </div>
        <div class="tooltip-total">
          <strong>{{ fmtM(hoverPoint.total) }}</strong>
          <span class="dim">AFT unlocked</span>
        </div>
        <div class="tooltip-rows">
          <div v-for="r in hoverRows" :key="r.id" class="tooltip-row">
            <span class="dot" :style="{ background: r.color }" />
            <span class="row-label">{{ r.label }}</span>
            <span class="mono row-val">{{ fmtM(r.value) }}</span>
          </div>
        </div>
      </div>
    </div>

    <div class="legend">
      <div v-for="g in legendGroups" :key="g.id" class="legend-group">
        <div class="legend-group-title ui-label">{{ g.label }}</div>
        <div class="legend-items">
          <div v-for="e in g.items" :key="e.id" class="legend-item">
            <span class="dot" :style="{ background: e.color }" />
            <span>{{ e.label }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.emission-chart {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.emission-head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
}

.emission-title {
  font-family: var(--font-display);
  font-size: 20px;
  font-weight: 700;
  letter-spacing: -0.02em;
  margin: 4px 0 0;
}

.emission-cursor {
  font-size: 12.5px;
  padding: 6px 10px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--chip-bg);
}

.emission-cursor strong {
  font-weight: 700;
  letter-spacing: -0.01em;
}

.chart-wrap {
  position: relative;
  width: 100%;
  touch-action: pan-y;
}

.chart-svg {
  display: block;
  width: 100%;
  height: auto;
  user-select: none;
}

.grid line {
  stroke: var(--line);
  stroke-width: 1;
  vector-effect: non-scaling-stroke;
}

.series .stack {
  transition: opacity 0.12s ease;
}

.series:hover .stack {
  opacity: 0.85;
}

.crosshair line {
  stroke: var(--ink);
  stroke-width: 1;
  stroke-dasharray: 4 4;
  opacity: 0.45;
  vector-effect: non-scaling-stroke;
}

.crosshair circle {
  fill: var(--ink);
  stroke: var(--bg-1);
  stroke-width: 2;
}

.y-axis text,
.x-axis text {
  font-family: var(--font-mono);
  font-size: 11px;
  fill: var(--ink-dimmer);
}

.x-axis .tick-month {
  font-size: 9px;
  fill: var(--ink-muted);
  letter-spacing: 0.04em;
}

.tooltip {
  position: absolute;
  top: 12px;
  min-width: 220px;
  padding: 10px 12px 12px;
  background: var(--bg-elev-solid);
  border: 1px solid var(--line-2);
  border-radius: 8px;
  box-shadow: var(--shadow-lift);
  font-size: 12px;
  pointer-events: none;
  z-index: 2;
}

.tooltip--right {
  transform: translateX(14px);
}

.tooltip--left {
  transform: translateX(calc(-100% - 14px));
}

.tooltip-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 6px;
}

.tooltip-head .mono {
  font-size: 11.5px;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--ink);
}

.tooltip-total {
  display: flex;
  align-items: baseline;
  gap: 6px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--line);
  margin-bottom: 8px;
}

.tooltip-total strong {
  font-family: var(--font-display);
  font-size: 22px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
}

.tooltip-total .dim {
  font-size: 11px;
}

.tooltip-rows {
  display: grid;
  gap: 4px;
  max-height: 260px;
  overflow-y: auto;
}

.tooltip-row {
  display: grid;
  grid-template-columns: 10px 1fr auto;
  align-items: center;
  gap: 7px;
}

.row-label {
  color: var(--ink-dim);
  font-size: 11.5px;
}

.row-val {
  font-size: 11.5px;
  font-weight: 600;
  color: var(--ink);
}

.dot {
  display: inline-block;
  width: 10px;
  height: 10px;
  border-radius: 2px;
  flex-shrink: 0;
}

.legend {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px 24px;
  padding-top: 14px;
  border-top: 1px solid var(--line);
}

.legend-group-title {
  font-size: 10px;
  margin-bottom: 6px;
}

.legend-items {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 14px;
}

.legend-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--ink-dim);
}

@media (max-width: 900px) {
  .legend {
    grid-template-columns: 1fr;
    gap: 12px;
  }

  .tooltip {
    min-width: 180px;
    font-size: 11px;
  }

  .tooltip-total strong {
    font-size: 18px;
  }
}

@media (max-width: 520px) {
  .tooltip {
    top: auto;
    bottom: 12px;
    min-width: 160px;
  }

  .tooltip-rows {
    max-height: 180px;
  }
}
</style>
