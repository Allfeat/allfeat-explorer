<script setup lang="ts">
// Horizontal stacked bar that visualises how total supply splits across
// circulating / locked (vesting) / envelope reserves. Sums to 100% by
// construction on the backend. Renders zero-width segments as 0px so the
// colour stripes don't shimmer when one bucket is empty.

import type { TokenOverview } from '@bindings'

const props = defineProps<{
  overview: TokenOverview
}>()

function pct(part: string, whole: string): number {
  try {
    const p = BigInt(part)
    const w = BigInt(whole)
    if (w === 0n) return 0
    return Math.min(100, Math.max(0, Number((p * 10000n) / w) / 100))
  }
  catch {
    return 0
  }
}

const circulatingPct = computed(() => pct(props.overview.circulating, props.overview.total_supply))
const lockedPct = computed(() => pct(props.overview.locked, props.overview.total_supply))
const reservesPct = computed(() => pct(props.overview.envelope_reserves, props.overview.total_supply))
</script>

<template>
  <div class="supply-bar">
    <div class="supply-bar-track" role="img" :aria-label="`Supply breakdown — ${fmtPct(circulatingPct)} circulating, ${fmtPct(lockedPct)} locked, ${fmtPct(reservesPct)} reserved`">
      <div
        class="seg seg-circ"
        :style="{ width: `${circulatingPct}%` }"
        :title="`Circulating · ${fmtPct(circulatingPct)}`"
      />
      <div
        class="seg seg-locked"
        :style="{ width: `${lockedPct}%` }"
        :title="`Locked · ${fmtPct(lockedPct)}`"
      />
      <div
        class="seg seg-reserves"
        :style="{ width: `${reservesPct}%` }"
        :title="`Envelope reserves · ${fmtPct(reservesPct)}`"
      />
    </div>

    <div class="supply-bar-legend">
      <div class="legend-item">
        <span class="swatch swatch-circ" />
        <span class="dim text-xs">Circulating</span>
        <span class="mono text-xs" style="font-weight: 600;">{{ fmtPct(circulatingPct) }}</span>
      </div>
      <div class="legend-item">
        <span class="swatch swatch-locked" />
        <span class="dim text-xs">Locked</span>
        <span class="mono text-xs" style="font-weight: 600;">{{ fmtPct(lockedPct) }}</span>
      </div>
      <div class="legend-item">
        <span class="swatch swatch-reserves" />
        <span class="dim text-xs">Envelope reserves</span>
        <span class="mono text-xs" style="font-weight: 600;">{{ fmtPct(reservesPct) }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.supply-bar {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.supply-bar-track {
  display: flex;
  height: 10px;
  border-radius: 6px;
  overflow: hidden;
  background: var(--chip-bg);
  border: 1px solid var(--line);
}

.seg {
  height: 100%;
  transition: width 0.4s cubic-bezier(0.2, 0.8, 0.2, 1);
}

.seg-circ {
  background: linear-gradient(90deg, var(--teal-500), var(--teal-400, var(--teal-500)));
}

.seg-locked {
  background: repeating-linear-gradient(
    135deg,
    var(--teal-700),
    var(--teal-700) 4px,
    var(--teal-800) 4px,
    var(--teal-800) 8px
  );
}

.seg-reserves {
  background: var(--ink-dimmer);
  opacity: 0.55;
}

.supply-bar-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 14px 22px;
}

.legend-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
}

.swatch {
  width: 10px;
  height: 10px;
  border-radius: 2px;
  flex-shrink: 0;
  border: 1px solid var(--line);
}

.swatch-circ {
  background: var(--teal-500);
  border-color: var(--teal-500);
}

.swatch-locked {
  background: repeating-linear-gradient(
    135deg,
    var(--teal-700),
    var(--teal-700) 3px,
    var(--teal-800) 3px,
    var(--teal-800) 6px
  );
  border-color: var(--teal-700);
}

.swatch-reserves {
  background: var(--ink-dimmer);
  opacity: 0.7;
}
</style>
