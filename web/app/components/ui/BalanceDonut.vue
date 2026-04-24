<script setup lang="ts">
// Two-segment donut for a Balance (transferable + reserved). Segments
// are sized by BigInt ratios so huge u128 amounts render accurately.

import { toBigIntSafe } from '~/utils/format'

const props = withDefaults(defineProps<{
  total: string
  transferable: string
  reserved: string
  size?: number
}>(), {
  size: 112,
})

function ratio(part: string, total: string): number {
  const t = toBigIntSafe(total)
  if (t === 0n) return 0
  return Number((toBigIntSafe(part) * 10_000n) / t) / 10_000
}

const transferableRatio = computed(() => ratio(props.transferable, props.total))
const reservedRatio = computed(() => ratio(props.reserved, props.total))

const R = 44
const C = computed(() => props.size / 2)
const circumference = 2 * Math.PI * R

const transferableLen = computed(() => transferableRatio.value * circumference)
const reservedLen = computed(() => reservedRatio.value * circumference)
const reservedOffset = computed(() => -transferableRatio.value * circumference)
</script>

<template>
  <svg
    :viewBox="`0 0 ${props.size} ${props.size}`"
    :width="props.size"
    :height="props.size"
    aria-hidden="true"
  >
    <circle :cx="C" :cy="C" :r="R" stroke="var(--line)" stroke-width="14" fill="none" />
    <circle
      :cx="C"
      :cy="C"
      :r="R"
      stroke="var(--teal-500)"
      stroke-width="14"
      fill="none"
      :stroke-dasharray="`${transferableLen} ${circumference - transferableLen}`"
      :transform="`rotate(-90 ${C} ${C})`"
    />
    <circle
      :cx="C"
      :cy="C"
      :r="R"
      stroke="var(--red-500)"
      stroke-width="14"
      fill="none"
      :stroke-dasharray="`${reservedLen} ${circumference - reservedLen}`"
      :stroke-dashoffset="reservedOffset"
      :transform="`rotate(-90 ${C} ${C})`"
    />
  </svg>
</template>
