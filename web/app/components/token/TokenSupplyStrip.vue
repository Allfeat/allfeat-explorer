<script setup lang="ts">
// Token supply hero — five stat cells split across the page top:
// Total | Circulating | Locked (vesting) | Envelope reserves | Distributed %.
//
// `circulating + locked + envelope_reserves === total_supply` by
// construction in the backend. The "Distributed" cell summarises the
// envelope progression (1 − reserves/total) so users can see at a glance
// how much of the genesis allocation has already been earmarked.

import type { TokenOverview } from '@bindings'

const props = defineProps<{
  overview: TokenOverview
}>()

function pct(part: string, whole: string): number {
  try {
    const p = BigInt(part)
    const w = BigInt(whole)
    if (w === 0n) return 0
    // Multiply first to keep integer math; cap at 100 in case of rounding.
    const ratio = Number((p * 10000n) / w) / 100
    return Math.min(100, Math.max(0, ratio))
  }
  catch {
    return 0
  }
}

const distributedPct = computed(() =>
  100 - pct(props.overview.envelope_reserves, props.overview.total_supply),
)

const lockedPct = computed(() =>
  pct(props.overview.locked, props.overview.total_supply),
)

const circulatingPct = computed(() =>
  pct(props.overview.circulating, props.overview.total_supply),
)

const reservesPct = computed(() =>
  pct(props.overview.envelope_reserves, props.overview.total_supply),
)
</script>

<template>
  <div class="panel" style="overflow: hidden;">
    <div class="stats-row">
      <div class="stats-cell">
        <div class="ui-label">Total supply</div>
        <div class="big" style="color: var(--teal-500);">
          {{ fmtAFT(overview.total_supply, overview.decimals, 0) }}
        </div>
        <div class="mono text-xs dim">{{ overview.symbol }} · fixed genesis cap</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Circulating</div>
        <div class="big">{{ fmtAFT(overview.circulating, overview.decimals, 0) }}</div>
        <div class="mono text-xs dim">{{ fmtPct(circulatingPct) }} · freely transferable</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Locked (vesting)</div>
        <div class="big">{{ fmtAFT(overview.locked, overview.decimals, 0) }}</div>
        <div class="mono text-xs dim">{{ fmtPct(lockedPct) }} · held, unlocking over time</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Envelope reserves</div>
        <div class="big">{{ fmtAFT(overview.envelope_reserves, overview.decimals, 0) }}</div>
        <div class="mono text-xs dim">{{ fmtPct(reservesPct) }} · not yet distributed</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Distributed</div>
        <div class="big">{{ fmtPct(distributedPct) }}</div>
        <div class="mono text-xs dim">of genesis allocation claimed out</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.stats-row {
  display: flex;
  flex-wrap: wrap;
}

.stats-cell {
  padding: 18px 22px;
  border-right: 1px solid var(--line);
  flex: 1;
  min-width: 160px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.stats-cell:last-child {
  border-right: none;
}

.big {
  font-family: var(--font-display);
  font-size: 26px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
}

@media (max-width: 900px) {
  .stats-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
  }

  .stats-cell {
    padding: 14px 16px;
    min-width: 0;
    border-right: none;
    border-bottom: 1px solid var(--line);
  }

  .stats-cell:nth-child(odd) {
    border-right: 1px solid var(--line);
  }

  .stats-cell:nth-child(5) {
    grid-column: 1 / -1;
    border-right: none;
    border-bottom: none;
  }

  .stats-cell:nth-last-child(2),
  .stats-cell:nth-last-child(3) {
    border-bottom: none;
  }

  .big {
    font-size: 22px;
  }
}

@media (max-width: 420px) {
  .stats-cell {
    padding: 10px 12px;
    gap: 4px;
  }
  .big {
    font-size: 18px;
  }
}
</style>
