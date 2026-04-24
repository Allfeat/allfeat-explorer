<script setup lang="ts">
// Live head-block card shown atop the dashboard. Reads the newest block
// from the Pinia live store (seeded by the layout's useLiveBlocks, then
// live-pushed by the WebSocket plugin). The "Next in Ns" countdown uses
// useWallClock so the label ticks every second without allocating a
// component-local interval.

import type { Block, NetworkSpec } from '@bindings'

const props = defineProps<{
  head: Block | null
  finalizedHeadNumber: string | null
  network: NetworkSpec | null
}>()

const now = useWallClock()
const addrLabel = useAddrLabel()

const blockTimeSecs = computed<number>(() => {
  const raw = props.network?.block_time_secs
  if (!raw) return 6
  const parsed = Number(raw)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 6
})

const nextIn = computed<number>(() => {
  if (!props.head) return 0
  const elapsedMs = now.value - props.head.timestamp_ms
  const perMs = blockTimeSecs.value * 1000
  const remainder = ((elapsedMs % perMs) + perMs) % perMs
  return Math.max(0, (perMs - remainder) / 1000)
})
</script>

<template>
  <div class="panel hero-card" style="padding: 28px; position: relative; overflow: hidden;">
    <div class="dotgrid" style="position: absolute; inset: 0; opacity: 0.6;" />
    <div style="position: relative;">
      <div class="hero-head-row">
        <LiveDot />
        <span class="ui-label" style="white-space: nowrap;">Head block · live</span>
        <span class="mono text-xs dim" style="margin-left: auto; white-space: nowrap;">
          Next in <span :key="Math.floor(nextIn)" class="ticker">{{ nextIn.toFixed(1) }}s</span>
        </span>
      </div>

      <div style="margin-bottom: 6px;">
        <div class="ui-label" style="margin-bottom: 4px;">Block</div>
        <div class="hero-block-num">
          <span v-if="head">
            #<span :key="head.number" class="ticker" style="display: inline-block;">{{ fmtInt(head.number) }}</span>
          </span>
          <span v-else>—</span>
        </div>
      </div>

      <div v-if="head" style="margin-top: 6px;">
        <Hash :text="head.hash" :head="10" :tail="10" />
      </div>

      <div v-if="head" class="hero-stats-row">
        <div class="hero-stat">
          <div class="ui-label" style="margin-bottom: 2px;">Author</div>
          <div class="hero-stat-val">{{ addrLabel(head.author, head.author_name) }}</div>
        </div>
        <div class="hero-stat hero-stat-bordered">
          <div class="ui-label" style="margin-bottom: 2px;">Extrinsics</div>
          <div class="hero-stat-val">{{ head.extrinsic_count }}</div>
        </div>
        <div class="hero-stat hero-stat-bordered">
          <div class="ui-label" style="margin-bottom: 2px;">Events</div>
          <div class="hero-stat-val">{{ head.event_count }}</div>
        </div>
      </div>

      <div v-if="head" style="margin-top: 20px;">
        <div style="display: flex; justify-content: space-between; margin-bottom: 6px; gap: 8px;">
          <span class="ui-label" style="white-space: nowrap;">Ref time</span>
          <span class="mono text-xs" style="white-space: nowrap;">{{ head.ref_time_pct }}% used</span>
        </div>
        <div class="bar">
          <i :style="{ width: `${head.ref_time_pct}%` }" />
        </div>
      </div>

      <div v-if="finalizedHeadNumber" style="margin-top: 16px;" class="mono text-xs dim">
        Finalized #{{ fmtInt(finalizedHeadNumber) }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.hero-head-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 16px;
  flex-wrap: nowrap;
}

.hero-block-num {
  font-family: var(--font-display);
  font-size: 44px;
  font-weight: 700;
  letter-spacing: -0.03em;
  line-height: 1;
  white-space: nowrap;
}

.hero-stats-row {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  margin-top: 22px;
  border-top: 1px solid var(--line);
}

.hero-stat {
  padding: 14px 16px;
}

.hero-stat-bordered {
  border-left: 1px solid var(--line);
}

.hero-stat-val {
  font-family: var(--font-display);
  font-size: 18px;
  font-weight: 600;
}
</style>
