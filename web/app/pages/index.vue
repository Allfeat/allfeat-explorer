<script setup lang="ts">
// Dashboard — the explorer's landing page. Composes the live hero, the
// ATS stats strip, and four feed panels. All live data flows through
// the shared Pinia store, seeded by the layout's `useLiveBlocks` call
// and pushed via the WS plugin; extra fetches here seed the panels that
// aren't already in the store.

import type { AtsStats } from '@bindings'
import { storeToRefs } from 'pinia'

definePageMeta({ name: 'home' })
useSeoMeta({
  title: 'Allfeat Explorer',
  description: 'Live block and timestamp explorer for the Allfeat music-industry blockchain.',
  ogTitle: 'Allfeat Explorer',
  ogDescription: 'Real-time view of blocks, transfers, and ATS timestamps on Allfeat.',
  ogType: 'website',
})

const { spec } = useActiveNetwork()
const { blocks, waveformBlocks, extrinsics, events, atsFeed } = storeToRefs(useLiveStore())

// Extrinsics + events + ATS feed panels need their own seeds — the
// layout only seeds blocks. Each composable pushes the fetched payload
// into the shared store so SSR renders with data and the WS plugin keeps
// the buffers fresh after hydration. Non-blocking so route transitions
// aren't held up by the round-trip on client navigation. Neither
// extrinsics nor events has a WS topic, so both composables watch
// head-block changes and re-seed on every new block.
useLiveExtrinsics({ count: 25 })
useLiveEvents({ count: 25 })
useLiveAtsFeed({ count: 6 })

const { data: atsStats } = useNetworkFetch<AtsStats | null>(
  net => `ats-stats:${net}`,
  net => `/networks/${net}/ats/stats`,
  { default: () => null },
)

const head = computed(() => blocks.value[0] ?? null)
const finalizedHeadNumber = computed(() => blocks.value.find(b => b.finalized)?.number ?? null)
</script>

<template>
  <section class="container dashboard">
    <div class="reveal d-1">
      <WaveformHero
        :blocks="waveformBlocks"
        :head="head"
        :finalized-head-number="finalizedHeadNumber"
        :network="spec"
      />
    </div>

    <div v-if="atsStats" class="reveal d-3 dashboard__row">
      <AtsStatsStrip :stats="atsStats" />
    </div>

    <div class="reveal d-4 dashboard__two-col dashboard__row dashboard__row--ats">
      <LatestAtsPanel :items="atsFeed" />
      <AtsPromoPanel :total-timestamped="atsStats?.total ?? null" />
    </div>

    <div class="reveal d-5 dashboard__two-col dashboard__row dashboard__row--main">
      <LatestBlocksPanel :blocks="blocks" />
      <LatestExtrinsicsPanel :extrinsics="extrinsics" :events="events" />
    </div>
  </section>
</template>

<style scoped lang="scss">
.dashboard {
  padding: 40px 0 64px;
}

.dashboard__row {
  margin-top: 24px;
}

.dashboard__two-col {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 24px;
}

// AtsPromo is decorative marketing copy — below 900px we hide it so the
// LatestAts panel stretches to full width rather than stacking a big
// promo card on top of every other feed on mobile.
@media (max-width: 900px) {
  .dashboard__row--ats {
    grid-template-columns: 1fr;
  }
  .dashboard__row--ats > :last-child {
    display: none;
  }
}

// Keep Latest blocks / Latest extrinsics side-by-side down into tablet
// portrait — the compact rows already fit two-up above 720px, so
// stacking them earlier just adds scroll without improving readability.
@media (max-width: 720px) {
  .dashboard__row--main {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 640px) {
  .dashboard {
    padding: 24px 0 48px;
  }
  .dashboard__row {
    margin-top: 16px;
  }
  .dashboard__two-col {
    gap: 16px;
  }
}
</style>
