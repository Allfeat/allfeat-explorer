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

const totalUpfront = computed<bigint>(() => {
  let sum = 0n
  for (const a of props.allocations) sum += toBigIntSafe(a.upfront)
  return sum
})

// Tokens beneficiaries have actually received: upfront paid at allocation
// time + everything auto-released through vesting since then. `totalReleased`
// covers only the vested portion, so this is what to surface to contributors
// reconciling against their wallet / CEX statement.
const totalDistributed = computed<bigint>(() => {
  let sum = 0n
  for (const a of props.allocations) {
    sum += toBigIntSafe(a.upfront) + toBigIntSafe(a.released)
  }
  return sum
})

const hasAnyUpfront = computed<boolean>(() => totalUpfront.value > 0n)

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

function distributedFor(a: Allocation): bigint {
  return toBigIntSafe(a.upfront) + toBigIntSafe(a.released)
}

function distributedTitle(a: Allocation): string {
  if (toBigIntSafe(a.upfront) === 0n) return ''
  return `Upfront ${fmtAFT(a.upfront, props.decimals, 2)} + Vested ${fmtAFT(a.released, props.decimals, 2)}`
}

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

function liveRemainingMs(anchorVal: number): number {
  if (anchorVal <= 0) return 0
  if (anchorMs.value === 0) return anchorVal
  const elapsed = now.value - anchorMs.value
  return Math.max(0, anchorVal - elapsed)
}

const remainingMs = computed<number>(() => liveRemainingMs(msUntilNextEpoch.value))

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

// Per-allocation cliff state. `start_block` is already clamped to
// `max(allocation.start, envelope.cliff)` upstream, so a single comparison
// against the current head is enough to detect an in-cliff allocation.
// `anchorMs` is captured at prop-refresh time; the template subtracts
// wall-clock drift via liveRemainingMs to tick the countdown.
const cliffByAllocation = computed<Map<number, number>>(() => {
  const out = new Map<number, number>()
  if (!props.epoch) return out
  const head = toBigIntSafe(props.epoch.head_block)
  for (const a of props.allocations) {
    const start = toBigIntSafe(a.start_block)
    if (head < start) {
      out.set(a.id, blocksToMs(start - head, props.blockTimeSecs ?? 0))
    }
  }
  return out
})

function isInCliff(a: Allocation): boolean {
  return cliffByAllocation.value.has(a.id)
}

function allocCliffLabel(a: Allocation): string {
  const anchor = cliffByAllocation.value.get(a.id)
  if (anchor === undefined) return ''
  return fmtCountdown(liveRemainingMs(anchor))
}

const allInCliff = computed<boolean>(() =>
  props.allocations.length > 0
  && props.allocations.every(a => cliffByAllocation.value.has(a.id)),
)

// Earliest cliff end across all allocations — the moment the aggregate
// "next distribution" display starts behaving again, so the most
// informative countdown for the summary cell.
const summaryCliffAnchorMs = computed<number>(() => {
  let min = Infinity
  for (const v of cliffByAllocation.value.values()) {
    if (v < min) min = v
  }
  return min === Infinity ? 0 : min
})

const summaryCliffLabel = computed<string>(() =>
  fmtCountdown(liveRemainingMs(summaryCliffAnchorMs.value)),
)
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
        <div class="alloc-summary-amount">{{ fmtAFT(totalDistributed, decimals, 2) }}</div>
        <div v-if="hasAnyUpfront" class="alloc-summary-breakdown">
          Upfront {{ fmtAFT(totalUpfront, decimals, 2) }} · Vested {{ fmtAFT(totalReleased, decimals, 2) }}
        </div>
      </div>
      <div class="alloc-summary-cell alloc-summary-next">
        <div class="alloc-summary-next-head">
          <span v-if="allInCliff" class="cliff-dot" aria-hidden="true" />
          <span v-else class="next-epoch-dot" aria-hidden="true" />
          <div class="ui-label">{{ allInCliff ? 'Cliff period' : 'Next distribution' }}</div>
        </div>
        <template v-if="allInCliff">
          <div class="alloc-summary-amount cliff">ends in {{ summaryCliffLabel }}</div>
          <div class="next-epoch-timer">All allocations in cliff</div>
        </template>
        <template v-else>
          <div class="alloc-summary-amount" :class="{ 'pos': totalNextEpoch > 0n }">
            {{ fmtAFT(totalNextEpoch, decimals, 2) }}
          </div>
          <div v-if="hasEpochData" class="next-epoch-timer mono">
            in {{ countdownLabel }}
          </div>
        </template>
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
          <td data-label="Distributed" class="right mono" :title="distributedTitle(a)">
            {{ fmtAFT(distributedFor(a), decimals, 2) }}
          </td>
          <td data-label="Next distribution" class="right mono">
            <span v-if="isInCliff(a)" class="cliff-cell">
              <span class="cliff-cell-dot" aria-hidden="true" />
              Cliff · {{ allocCliffLabel(a) }}
            </span>
            <span v-else class="next-epoch-cell" :class="{ 'next-epoch-cell-active': nextEpochReceive(a) > 0n }">
              <span v-if="nextEpochReceive(a) > 0n" class="next-epoch-cell-dot" aria-hidden="true" />
              {{ fmtAFT(nextEpochReceive(a), decimals, 2) }}
            </span>
          </td>
          <td data-label="Vested" class="right mono text-xs dim">
            <span v-if="isInCliff(a)" class="cliff-vested">in cliff</span>
            <template v-else>{{ a.percent_vested }}%</template>
          </td>
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

.alloc-summary-breakdown {
  font-size: 11.5px;
  color: var(--ink-dim);
  letter-spacing: 0.02em;
  margin-top: 4px;
  font-variant-numeric: tabular-nums;
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

/* Cliff state — neutral/muted styling that communicates "waiting, not
   distributing yet" without looking like an error or a zero-value bug.
   Reuses the cream-200 tone from the shared warn palette. */
.cliff-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--ink-dim);
  opacity: 0.7;
  flex-shrink: 0;
}

.alloc-summary-amount.cliff {
  font-family: var(--font-mono);
  font-size: 17px;
  font-weight: 600;
  color: var(--ink-dim);
  letter-spacing: 0;
}

.cliff-cell {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  justify-content: flex-end;
  color: var(--ink-dim);
  font-weight: 500;
}

.cliff-cell-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: currentColor;
  opacity: 0.65;
  flex-shrink: 0;
}

.cliff-vested {
  color: var(--ink-dimmer);
  font-style: italic;
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
