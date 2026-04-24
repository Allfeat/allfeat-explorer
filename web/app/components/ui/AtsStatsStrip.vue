<script setup lang="ts">
// Compact stats strip shown atop the ATS index page and on the
// dashboard. Reads an `AtsStats` prop and renders the cells verbatim.

import type { AtsStats } from '@bindings'

defineProps<{
  stats: AtsStats
}>()
</script>

<template>
  <div class="panel" style="overflow: hidden;">
    <div class="stats-row">
      <div class="stats-cell">
        <div class="ui-label">Total ATS</div>
        <div class="big" style="color: var(--teal-500);">{{ fmtInt(stats.total) }}</div>
        <div class="mono text-xs dim">Since genesis</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Last 24h</div>
        <div class="big">+{{ fmtInt(stats.last_24h) }}</div>
        <div class="mono text-xs dim">avg {{ fmtInt(stats.avg_per_day) }}/day</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Last 7d</div>
        <div class="big">+{{ fmtInt(stats.last_7d) }}</div>
        <div class="mono text-xs dim">{{ fmtInt(stats.last_30d) }} / 30d</div>
      </div>
      <div class="stats-cell">
        <div class="ui-label">Unique owners</div>
        <div class="big">{{ fmtInt(stats.unique_owners) }}</div>
        <div class="mono text-xs dim">Registered accounts</div>
      </div>
      <div class="stats-cell stats-protocol">
        <div class="ui-label">Protocol</div>
        <div style="display: flex; align-items: center; gap: 8px; margin-top: 2px;">
          <span class="big" style="font-size: 22px;">v{{ stats.protocol_version }}</span>
          <span class="stable-chip">stable</span>
        </div>
        <div class="mono text-xs dim" style="margin-top: 6px;">SHA-256 · Merkle d=5</div>
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
  font-size: 28px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
}

.stats-protocol .big {
  font-size: 22px;
}

.stable-chip {
  padding: 2px 8px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--teal-500);
  background: rgba(0, 177, 140, 0.1);
  border: 1px solid rgba(0, 177, 140, 0.2);
}

/* Lay out as a 2-column grid so five metrics read as 2 + 2 + 1 with
   the protocol cell spanning the bottom row. */
@media (max-width: 720px) {
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

  .stats-protocol {
    grid-column: 1 / -1;
    border-right: none;
    border-bottom: none;
  }

  /* The 3rd and 4th cells sit just above the spanning protocol row, so
     their bottom border is the page's section divider — drop it to avoid
     a double line. */
  .stats-cell:nth-last-child(2) { border-bottom: none; }
  .stats-cell:nth-last-child(3) { border-bottom: none; }

  .big { font-size: 24px; }
  .stats-protocol .big { font-size: 20px; }
}

/* Very narrow: keep 2-col, just tighten. Single-column would push the
   strip past 500px tall which is pure scroll tax — the values stay
   readable at 20px even on a 360px screen. */
@media (max-width: 420px) {
  .stats-cell {
    padding: 10px 12px;
    gap: 4px;
  }
  .big { font-size: 20px; }
  .stats-protocol .big { font-size: 18px; }
}
</style>
