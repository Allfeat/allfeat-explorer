<script setup lang="ts">
// Allocation list for a single envelope. Beneficiaries link to their
// account page; amounts are formatted to 4 decimals (typical AFT range
// for these envelopes).

import type { Allocation } from '@bindings'

const props = defineProps<{
  allocations: readonly Allocation[]
  decimals: number
  symbol: string
}>()

const { accountHref } = useNetworkLink()
const addrLabel = useAddrLabel()
</script>

<template>
  <div class="panel">
    <div class="panel-head">
      <h3>Allocations</h3>
      <span class="tag">{{ fmtInt(allocations.length) }} total</span>
    </div>

    <table v-if="allocations.length > 0" class="table alloc-table">
      <thead>
        <tr>
          <th>#</th>
          <th>Beneficiary</th>
          <th class="right">Total</th>
          <th class="right">Released</th>
          <th class="right">Claimable now</th>
          <th class="right">Vested</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="a in allocations" :key="a.id">
          <td><span class="mono dim">#{{ a.id }}</span></td>
          <td data-label="Beneficiary">
            <NuxtLink class="hash" :to="accountHref(a.beneficiary)">
              {{ addrLabel(a.beneficiary) }}
            </NuxtLink>
          </td>
          <td data-label="Total" class="right mono">
            {{ fmtAFT(a.total, decimals, 4) }} {{ symbol }}
          </td>
          <td data-label="Released" class="right mono">
            {{ fmtAFT(a.released, decimals, 4) }}
          </td>
          <td data-label="Claimable now" class="right mono">
            <span :class="{ 'claimable-positive': a.claimable_now !== '0' }">
              {{ fmtAFT(a.claimable_now, decimals, 4) }}
            </span>
          </td>
          <td data-label="Vested" class="right">
            <div class="vested-cell">
              <span class="mono text-xs dim">{{ a.percent_vested }}%</span>
              <div class="vested-bar">
                <div class="vested-fill" :style="{ width: `${a.percent_vested}%` }" />
              </div>
            </div>
          </td>
        </tr>
      </tbody>
    </table>

    <p v-else class="dim" style="padding: 40px; text-align: center;">
      No allocations issued from this envelope yet.
    </p>
  </div>
</template>

<style scoped>
.alloc-table th.right,
.alloc-table td.right {
  text-align: right;
}

.claimable-positive {
  color: var(--teal-500);
  font-weight: 600;
}

.vested-cell {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 110px;
  justify-content: flex-end;
}

.vested-bar {
  width: 70px;
  height: 4px;
  border-radius: 2px;
  background: var(--chip-bg);
  overflow: hidden;
}

.vested-fill {
  height: 100%;
  background: var(--teal-500);
  transition: width 0.3s ease;
}

@media (max-width: 640px) {
  .vested-cell {
    min-width: 0;
    justify-content: flex-start;
  }
  .vested-bar {
    display: none;
  }
}
</style>
