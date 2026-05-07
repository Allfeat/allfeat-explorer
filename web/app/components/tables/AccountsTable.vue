<script setup lang="ts">
// Top accounts table — rank by balance. Rows link to the account detail
// page. The "% of supply" column divides against a fixed max to match
// the maquette; once the backend exposes a real total-issuance figure we
// swap this out for a reactive denominator.

import type { Account } from '@bindings'
import { toBigIntSafe } from '~/utils/format'

defineProps<{
  accounts: readonly Account[]
  totalSupplyPlanck?: string
}>()

const { to } = useNetworkLink()
const { spec } = useActiveNetwork()
const tokenSymbol = computed(() => spec.value?.token ?? '')

function openAccount(address: string) {
  navigateTo(to(`/accounts/${address}`))
}

// `128.4M AFT` in the maquette, i.e. 128_400_000 AFT. In planck at 12
// decimals that's 128_400_000e12.
const DEFAULT_TOTAL_SUPPLY = '128400000000000000000000'

function supplyPct(balance: string, supply: string): string {
  const si = toBigIntSafe(supply)
  if (si === 0n) return '—'
  // Scale by 1e6 for 3 decimal digits, then divide by 1e4 in JS.
  const ratio = Number((toBigIntSafe(balance) * 1_000_000n) / si) / 10_000
  return fmtPct(ratio, 3)
}
</script>

<template>
  <table class="table">
    <thead>
      <tr>
        <th>Rank</th>
        <th>Account</th>
        <th class="right">Balance</th>
        <th class="right">% of supply</th>
        <th class="right">Last active</th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="(a, i) in accounts"
        :key="a.address"
        class="clickable-row"
        role="link"
        tabindex="0"
        :aria-label="`Open account ${shortAddr(a.address, 6, 6)}`"
        @click="openAccount(a.address)"
        @keydown.enter.space.prevent="openAccount(a.address)"
      >
        <td><span class="mono dim">#{{ i + 1 }}</span></td>
        <td data-label="Account"><Addr :text="a.address" /></td>
        <td data-label="Balance" class="right mono balance-cell">{{ fmtAFT(a.balance.total, 12, 2) }} {{ tokenSymbol }}</td>
        <td data-label="% supply" class="right mono text-xs dim">
          {{ supplyPct(a.balance.total, totalSupplyPlanck ?? DEFAULT_TOTAL_SUPPLY) }}
        </td>
        <td data-label="Last active" class="right">
          <span class="mono text-xs dim"><TimeAgo :timestamp="a.last_active_ms" /></span>
        </td>
      </tr>
    </tbody>
  </table>
</template>

<style scoped lang="scss">
.clickable-row {
  cursor: pointer;

  &:focus-visible {
    outline: 2px solid var(--teal-500);
    outline-offset: -2px;
  }
}

.balance-cell {
  font-weight: 600;
}
</style>
