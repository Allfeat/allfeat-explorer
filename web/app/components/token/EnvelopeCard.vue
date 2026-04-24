<script setup lang="ts">
// Single envelope card. Renders cap + distributed progress + headline
// vesting params. The whole card is clickable → /token/envelopes/[slug].
//
// `slug` doubles as the URL segment and the EnvelopeId on the wire — the
// server accepts the same string.

import type { EnvelopeInfo } from '@bindings'
import { blocksToHumanDuration } from '~/utils/blocks'
import { categoryInfo, envelopeMeta } from '~/utils/envelopes'

const props = defineProps<{
  envelope: EnvelopeInfo
  decimals: number
  symbol: string
}>()

const { envelopeHref } = useNetworkLink()

const detailHref = computed(() => envelopeHref(props.envelope.id))

const distributedPct = computed(() => {
  try {
    const distributed = BigInt(props.envelope.distributed)
    const cap = BigInt(props.envelope.total_cap)
    if (cap === 0n) return 0
    return Math.min(100, Number((distributed * 10000n) / cap) / 100)
  }
  catch {
    return 0
  }
})

const cliffLabel = computed(() => blocksToHumanDuration(props.envelope.cliff_blocks))
const vestingLabel = computed(() => blocksToHumanDuration(props.envelope.vesting_duration_blocks))

const isUnique = computed(() => props.envelope.unique_beneficiary !== null)

const meta = computed(() => envelopeMeta(props.envelope.id))
const category = computed(() => (meta.value ? categoryInfo(meta.value.category) : null))
</script>

<template>
  <NuxtLink :to="detailHref" class="env-card panel" :title="`Open ${envelope.label}`">
    <div class="env-head">
      <div class="env-title">
        <EnvelopeIcon :id="envelope.id" :size="22" />
        <div class="env-title-text">
          <span class="env-label">{{ envelope.label }}</span>
          <span v-if="category" class="env-category dim mono text-xs">{{ category.label }}</span>
        </div>
      </div>
      <span v-if="isUnique" class="env-tag" title="Single auto-allocated beneficiary">
        Unique
      </span>
    </div>

    <p v-if="meta" class="env-blurb dim">{{ meta.blurb }}</p>

    <div class="env-cap">
      <div class="env-cap-amount">
        {{ fmtAFT(envelope.total_cap, decimals, 0) }}
        <span class="env-cap-unit">{{ symbol }}</span>
      </div>
      <div class="env-cap-sub mono text-xs dim">
        cap<span v-if="meta"> · {{ meta.supplyPct.toFixed(meta.supplyPct < 1 ? 1 : 0) }}% of genesis supply</span>
      </div>
    </div>

    <div class="env-progress">
      <div class="env-progress-track">
        <div class="env-progress-fill" :style="{ width: `${distributedPct}%` }" />
      </div>
      <div class="env-progress-meta">
        <span class="mono">{{ fmtPct(distributedPct) }}</span>
        <span class="dim">{{ fmtAFT(envelope.distributed, decimals, 0) }} distributed</span>
      </div>
    </div>

    <dl class="env-kv">
      <div>
        <dt>Upfront</dt>
        <dd class="mono">{{ envelope.upfront_pct }}%</dd>
      </div>
      <div>
        <dt>Cliff</dt>
        <dd class="mono">{{ cliffLabel }}</dd>
      </div>
      <div>
        <dt>Vesting</dt>
        <dd class="mono">{{ vestingLabel }}</dd>
      </div>
      <div>
        <dt>Allocations</dt>
        <dd class="mono">{{ fmtInt(envelope.allocation_count) }}</dd>
      </div>
    </dl>
  </NuxtLink>
</template>

<style scoped>
.env-card {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 20px;
  text-decoration: none;
  color: inherit;
  transition: border-color 0.15s ease, transform 0.15s ease;
}

.env-card:hover {
  border-color: var(--teal-500);
  transform: translateY(-1px);
}

.env-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
}

.env-title {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.env-title-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.env-label {
  font-family: var(--font-display);
  font-size: 16px;
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--ink);
}

.env-category {
  letter-spacing: 0.06em;
  text-transform: uppercase;
  line-height: 1;
}

.env-blurb {
  font-size: 12.5px;
  line-height: 1.45;
  margin: 0;
  /* Cap to 3 lines so cards stay uniform in the grid. */
  display: -webkit-box;
  -webkit-line-clamp: 3;
  line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.env-tag {
  padding: 2px 8px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 9.5px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--teal-500);
  background: rgba(0, 177, 140, 0.1);
  border: 1px solid rgba(0, 177, 140, 0.2);
  white-space: nowrap;
}

.env-cap {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.env-cap-amount {
  font-family: var(--font-display);
  font-size: 22px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
}

.env-cap-unit {
  font-size: 12px;
  font-weight: 500;
  color: var(--ink-dimmer);
}

.env-cap-sub {
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.env-progress {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.env-progress-track {
  height: 6px;
  border-radius: 3px;
  background: var(--chip-bg);
  overflow: hidden;
}

.env-progress-fill {
  height: 100%;
  background: linear-gradient(90deg, var(--teal-500), var(--teal-400, var(--teal-500)));
  transition: width 0.4s cubic-bezier(0.2, 0.8, 0.2, 1);
}

.env-progress-meta {
  display: flex;
  justify-content: space-between;
  font-size: 11.5px;
}

.env-kv {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px 12px;
  margin: 0;
}

.env-kv > div {
  display: flex;
  justify-content: space-between;
  font-size: 12px;
}

.env-kv dt {
  color: var(--ink-dimmer);
  letter-spacing: 0.04em;
}

.env-kv dd {
  margin: 0;
  color: var(--ink);
  font-size: 12.5px;
}
</style>
