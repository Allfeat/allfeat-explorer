<script setup lang="ts">
// Allocations a single account holds across the genesis envelopes.
//
// `pallet-token-allocation` has no manual-claim extrinsic — vested funds are
// auto-distributed to each beneficiary at every epoch. So the UI frames the
// "pending" bucket as "next distribution" rather than "claimable", with a
// live countdown to the upcoming payout block. The animation on the
// pulsing dot + ticking timer communicates the automatic cadence at a
// glance.
//
// On non-mainnet networks the parent skips rendering this panel — the
// wrapping conditional lives in AccountDetail.vue so we don't have to
// resolve `isMainnet` in two places.

import type { Allocation, EnvelopeInfo, EpochInfo } from '@bindings'
import { blocksToMs } from '~/utils/blocks'
import { toBigIntSafe } from '~/utils/format'

const props = defineProps<{
  allocations: readonly Allocation[]
  decimals: number
  symbol: string
  epoch?: EpochInfo | null
  envelopes?: readonly EnvelopeInfo[]
  blockTimeSecs?: number
}>()

const { envelopeHref } = useNetworkLink()
const now = useWallClock()

const totalReleased = computed<string>(() => {
  let sum = 0n
  for (const a of props.allocations) sum += toBigIntSafe(a.released)
  return sum.toString()
})

const totalAllocated = computed<string>(() => {
  let sum = 0n
  for (const a of props.allocations) sum += toBigIntSafe(a.total)
  return sum.toString()
})

// Per-envelope vesting duration lookup keyed by envelope id. Empty when
// the parent hasn't threaded overview data through yet — per-row and
// summary next-epoch values fall back to the server-side `claimable_now`
// snapshot in that case.
const vestingByEnvelope = computed<Record<string, bigint>>(() => {
  const out: Record<string, bigint> = {}
  for (const env of props.envelopes ?? []) {
    out[env.id as string] = toBigIntSafe(env.vesting_duration_blocks)
  }
  return out
})

// Linear vest with SCALE-level semantics mirroring
// src/data/rpc/mappers/token.rs::claimable_amount. Returns the amount of
// `vested_total` that is pending distribution at block `h`, net of
// already-released funds.
function pendingAtBlock(a: Allocation, h: bigint): bigint {
  const start = toBigIntSafe(a.start_block)
  const vestedTotal = toBigIntSafe(a.vested_total)
  const released = toBigIntSafe(a.released)
  const duration = vestingByEnvelope.value[a.envelope as string] ?? 0n
  if (h <= start) return 0n
  if (duration === 0n) {
    return vestedTotal > released ? vestedTotal - released : 0n
  }
  const elapsed = h - start
  const vested = elapsed >= duration
    ? vestedTotal
    : (vestedTotal * elapsed) / duration
  return vested > released ? vested - released : 0n
}

// Per-row: amount the pallet will auto-distribute at the next epoch — the
// vested-minus-released delta evaluated at `next_payout_block`. When the
// parent hasn't threaded epoch + envelope data through yet, fall back to
// the current `claimable_now` snapshot so the column still shows a
// sensible approximation.
function nextEpochReceive(a: Allocation): bigint {
  if (!props.epoch || (props.envelopes?.length ?? 0) === 0) {
    return toBigIntSafe(a.claimable_now)
  }
  const nextBlock = toBigIntSafe(props.epoch.next_payout_block)
  if (nextBlock === 0n) return toBigIntSafe(a.claimable_now)
  return pendingAtBlock(a, nextBlock)
}

const totalNextEpoch = computed<bigint>(() => {
  let sum = 0n
  for (const a of props.allocations) sum += nextEpochReceive(a)
  return sum
})

const hasEpochData = computed(() =>
  props.epoch !== null && props.epoch !== undefined && (props.envelopes?.length ?? 0) > 0,
)

// Client-side countdown to the next epoch payout. `blocksToMs` handles the
// 6s default + caller-supplied cadence; the wall-clock ticks every second
// so the label updates smoothly without any per-component timer.
const msUntilNextEpoch = computed<number>(() => {
  if (!props.epoch) return 0
  const blocks = toBigIntSafe(props.epoch.next_payout_block) - toBigIntSafe(props.epoch.head_block)
  if (blocks <= 0n) return 0
  return blocksToMs(blocks, props.blockTimeSecs ?? 0)
})

// Render anchor (ms since Unix epoch) for the countdown — captured on
// mount so wall-clock drift renders a descending label instead of a
// constant.
const anchorMs = ref<number>(0)
onMounted(() => { anchorMs.value = Date.now() })

const remainingMs = computed<number>(() => {
  if (msUntilNextEpoch.value <= 0) return 0
  if (anchorMs.value === 0) return msUntilNextEpoch.value
  const elapsed = now.value - anchorMs.value
  return Math.max(0, msUntilNextEpoch.value - elapsed)
})

function fmtCountdown(ms: number): string {
  if (ms <= 0) return 'imminent'
  const total = Math.floor(ms / 1000)
  const d = Math.floor(total / 86_400)
  const h = Math.floor((total % 86_400) / 3_600)
  const m = Math.floor((total % 3_600) / 60)
  const s = total % 60
  if (d > 0) return `${d}d ${String(h).padStart(2, '0')}h ${String(m).padStart(2, '0')}m`
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m ${String(s).padStart(2, '0')}s`
  if (m > 0) return `${m}m ${String(s).padStart(2, '0')}s`
  return `${s}s`
}

const countdownLabel = computed<string>(() => fmtCountdown(remainingMs.value))
</script>

<template>
  <div v-if="allocations.length > 0" class="panel">
    <div class="panel-head">
      <h3>Token allocations</h3>
      <span class="tag">{{ allocations.length }} envelope{{ allocations.length === 1 ? '' : 's' }}</span>
    </div>

    <div class="alloc-summary">
      <div class="alloc-summary-cell">
        <div class="ui-label">Total allocated</div>
        <div class="alloc-summary-amount">{{ fmtAFT(totalAllocated, decimals, 2) }}</div>
      </div>
      <div class="alloc-summary-cell">
        <div class="ui-label">Distributed so far</div>
        <div class="alloc-summary-amount">{{ fmtAFT(totalReleased, decimals, 2) }}</div>
      </div>
      <div class="alloc-summary-cell alloc-summary-next">
        <div class="alloc-summary-next-head">
          <span class="next-epoch-dot" aria-hidden="true" />
          <div class="ui-label">Next distribution</div>
        </div>
        <div class="alloc-summary-amount" :class="{ 'pos': totalNextEpoch > 0n }">
          {{ fmtAFT(totalNextEpoch, decimals, 2) }}
        </div>
        <div v-if="hasEpochData" class="next-epoch-timer mono">
          in {{ countdownLabel }}
        </div>
      </div>
    </div>

    <table class="table alloc-table">
      <thead>
        <tr>
          <th>#</th>
          <th>Envelope</th>
          <th class="right">Total</th>
          <th class="right">Distributed</th>
          <th class="right">Next distribution</th>
          <th class="right">Vested</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="a in allocations" :key="a.id">
          <td><span class="mono dim">#{{ a.id }}</span></td>
          <td data-label="Envelope">
            <NuxtLink class="hash" :to="envelopeHref(a.envelope)">{{ a.envelope }}</NuxtLink>
          </td>
          <td data-label="Total" class="right mono">
            {{ fmtAFT(a.total, decimals, 2) }} {{ symbol }}
          </td>
          <td data-label="Distributed" class="right mono">
            {{ fmtAFT(a.released, decimals, 2) }}
          </td>
          <td data-label="Next distribution" class="right mono">
            <span class="next-epoch-cell" :class="{ 'next-epoch-cell-active': nextEpochReceive(a) > 0n }">
              <span v-if="nextEpochReceive(a) > 0n" class="next-epoch-cell-dot" aria-hidden="true" />
              {{ fmtAFT(nextEpochReceive(a), decimals, 2) }}
            </span>
          </td>
          <td data-label="Vested" class="right mono text-xs dim">{{ a.percent_vested }}%</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
.alloc-summary {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  border-bottom: 1px solid var(--line);
}

.alloc-summary-cell {
  padding: 16px 22px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  border-right: 1px solid var(--line);
}

.alloc-summary-cell:last-child {
  border-right: none;
}

.alloc-summary-amount {
  font-family: var(--font-display);
  font-size: 22px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
}

.alloc-summary-amount.pos {
  color: var(--teal-500);
}

.alloc-summary-next-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.pos {
  color: var(--teal-500);
  font-weight: 600;
}

/* Header right-alignment — `.table th { text-align: left }` from the shared
   stylesheet wins on specificity otherwise, leaving numeric headers visually
   misaligned with their right-aligned data. */
.alloc-table th.right {
  text-align: right;
}

.next-epoch-timer {
  font-size: 11.5px;
  color: var(--ink-dim);
  letter-spacing: 0.02em;
  margin-top: 2px;
}

.next-epoch-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--teal-500);
  box-shadow: 0 0 0 0 rgba(0, 177, 140, 0.55);
  animation: next-epoch-pulse 1.8s ease-in-out infinite;
  flex-shrink: 0;
}

@keyframes next-epoch-pulse {
  0%, 100% {
    box-shadow: 0 0 0 0 rgba(0, 177, 140, 0.55);
    transform: scale(1);
  }
  50% {
    box-shadow: 0 0 0 6px rgba(0, 177, 140, 0);
    transform: scale(1.15);
  }
}

.next-epoch-cell {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  justify-content: flex-end;
  color: var(--ink-dim);
}

.next-epoch-cell-active {
  color: var(--teal-500);
  font-weight: 600;
}

.next-epoch-cell-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--teal-500);
  box-shadow: 0 0 0 0 rgba(0, 177, 140, 0.55);
  animation: next-epoch-pulse 1.8s ease-in-out infinite;
  flex-shrink: 0;
}

@media (max-width: 720px) {
  .alloc-summary {
    grid-template-columns: 1fr;
  }
  .alloc-summary-cell {
    border-right: none;
    border-bottom: 1px solid var(--line);
  }
  .alloc-summary-cell:last-child {
    border-bottom: none;
  }
  .alloc-summary-amount {
    font-size: 20px;
  }
}
</style>
