<script setup lang="ts">
// Account detail. Top-row shows Balance (donut + stats) next to a small
// Overview KV. Below that, the ATS registrations panel renders either an
// empty state or a compact list. We don't have per-account extrinsics /
// transfers endpoints yet, so those tabs from the maquette are elided
// until the backend grows the corresponding queries.

import type { Account, Allocation, AtsRecord, TokenOverview } from '@bindings'
import type { AddressFormat } from '~/components/ui/AddressFormatToggle.vue'
import { GENERIC_SS58_PREFIX, reencodeSs58 } from '~/utils/ss58'

const props = withDefaults(defineProps<{
  account: Account
  ats: readonly AtsRecord[]
  atsCount: number
  allocations?: readonly Allocation[]
  tokenSymbol?: string
  tokenOverview?: TokenOverview | null
  blockTimeSecs?: number
}>(), {
  allocations: () => [],
  tokenSymbol: 'AFT',
  tokenOverview: null,
  blockTimeSecs: 6,
})

const { atsHref } = useNetworkLink()
const known = useKnownAccount(() => props.account.address)
const network = useActiveNetwork()

// Hide the toggle when the network already uses the generic prefix —
// re-encoding would produce the exact same string and the button would
// look broken.
const networkPrefix = computed(() => network.spec.value?.ss58_prefix ?? GENERIC_SS58_PREFIX)
const canToggleFormat = computed(() => networkPrefix.value !== GENERIC_SS58_PREFIX)

const addressFormat = ref<AddressFormat>('mainnet')

const displayAddress = computed(() => {
  if (addressFormat.value === 'mainnet' || !canToggleFormat.value) return props.account.address
  try {
    return reencodeSs58(props.account.address, GENERIC_SS58_PREFIX)
  } catch {
    return props.account.address
  }
})

</script>

<template>
  <div>
    <div class="page-title account-title">
      <div class="account-id">
        <Identicon :seed="account.address" large />
        <div>
          <h1 v-if="known" class="account-known-title">
            {{ known.name }}
          </h1>
          <h1 class="account-addr-title" :class="{ 'secondary': !!known }">
            <span>{{ displayAddress }}</span>
            <CopyButton :text="displayAddress" />
            <AddressFormatToggle v-if="canToggleFormat" v-model="addressFormat" />
          </h1>
          <p v-if="known?.note" class="known-note">{{ known.note }}</p>
        </div>
      </div>
    </div>

    <div class="acct-grid">
      <div class="panel">
        <div class="panel-head">
          <h3>Balance</h3>
          <span class="tag">AFT · Native</span>
        </div>
        <div style="padding: 24px;">
          <div class="balance-row">
            <BalanceDonut
              :total="account.balance.total"
              :transferable="account.balance.transferable"
              :reserved="account.balance.reserved"
            />
            <div style="flex: 1; min-width: 0;">
              <div class="ui-label">Total</div>
              <div class="balance-total">
                {{ fmtAFT(account.balance.total, 12, 2) }}
                <span style="color: var(--ink-dimmer); font-size: 20px;">AFT</span>
              </div>
              <div style="margin-top: 18px; display: flex; flex-direction: column; gap: 8px;">
                <div class="bal-row">
                  <span class="swatch swatch-teal" />
                  <span style="flex: 1;">Transferable</span>
                  <span class="mono" style="font-weight: 600; white-space: nowrap;">
                    {{ fmtAFT(account.balance.transferable, 12, 2) }} AFT
                  </span>
                </div>
                <div class="bal-row">
                  <span class="swatch swatch-red" />
                  <span style="flex: 1;">Reserved</span>
                  <span class="mono" style="font-weight: 600; white-space: nowrap;">
                    {{ fmtAFT(account.balance.reserved, 12, 2) }} AFT
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="panel">
        <div class="panel-head">
          <h3>Overview</h3>
        </div>
        <Kv>
          <KvRow label="Nonce">
            <span class="mono">{{ fmtInt(account.nonce) }}</span>
          </KvRow>
          <KvRow label="First seen">
            <span class="mono text-xs">{{ new Date(account.first_seen_ms).toISOString().slice(0, 10) }}</span>
            <span class="dim"> · <TimeAgo :timestamp="account.first_seen_ms" /></span>
          </KvRow>
          <KvRow label="Last activity">
            <span class="mono text-xs"><TimeAgo :timestamp="account.last_active_ms" /></span>
          </KvRow>
          <KvRow label="ATS registrations">
            <span class="mono">{{ fmtInt(atsCount) }}</span>
          </KvRow>
        </Kv>
      </div>
    </div>

    <AccountAllocationsPanel
      v-if="allocations.length > 0"
      style="margin-top: 24px;"
      :allocations="allocations"
      :decimals="12"
      :symbol="tokenSymbol"
      :epoch="tokenOverview?.epoch ?? null"
      :envelopes="tokenOverview?.envelopes ?? []"
      :block-time-secs="blockTimeSecs"
    />

    <div class="panel" style="margin-top: 24px;">
      <div class="panel-head">
        <h3>ATS registrations</h3>
        <span class="tag">{{ fmtInt(atsCount) }} total</span>
      </div>
      <table v-if="ats.length > 0" class="table">
        <thead>
          <tr>
            <th>#</th>
            <th>ATS</th>
            <th>Versions</th>
            <th class="right">Created</th>
            <th class="right">Total deposit</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="record in ats" :key="record.ats_id">
            <td><span class="mono dim">#{{ record.ats_id }}</span></td>
            <td data-label="ATS">
              <NuxtLink class="hash" :to="atsHref(record.ats_id)">
                ATS #{{ record.ats_id }}
              </NuxtLink>
            </td>
            <td data-label="Versions"><Chip>v{{ record.version_count }}</Chip></td>
            <td data-label="Created" class="right">
              <span class="mono text-xs dim"><TimeAgo :timestamp="record.created_at_ms" /></span>
            </td>
            <td data-label="Deposit" class="right mono text-xs">{{ fmtAFT(record.total_deposit, 12, 4) }} AFT</td>
          </tr>
        </tbody>
      </table>
      <p v-else class="dim" style="padding: 40px; text-align: center;">
        This account hasn't registered any ATS entries yet.
      </p>
    </div>
  </div>
</template>

<style scoped>
.account-title {
  align-items: flex-start;
}

.account-id {
  display: flex;
  gap: 20px;
  align-items: center;
  min-width: 0;
}

.account-id > div {
  min-width: 0;
}

.account-addr-title {
  display: flex;
  align-items: center;
  gap: 12px;
  font-family: var(--font-mono);
  font-size: 22px;
  font-weight: 500;
  color: var(--ink-dim);
  letter-spacing: 0;
  word-break: break-all;
}

.account-addr-title.secondary {
  font-size: 13px;
  font-weight: 400;
  color: var(--ink-dimmer, rgba(255, 255, 255, 0.45));
  margin-top: 4px;
}

.account-known-title {
  font-family: var(--font-display);
  font-size: 28px;
  font-weight: 700;
  letter-spacing: -0.01em;
  line-height: 1.1;
  margin: 2px 0 0;
}

.known-note {
  margin: 10px 0 0;
  font-size: 13px;
  color: var(--ink-dim);
  max-width: 60ch;
}

.acct-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 24px;
  margin-top: 28px;
}

@media (max-width: 900px) {
  .acct-grid {
    grid-template-columns: 1fr;
  }
}

.balance-row {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 28px;
  align-items: center;
}

@media (max-width: 560px) {
  .balance-row {
    grid-template-columns: 1fr;
  }
}

.balance-total {
  font-family: var(--font-display);
  font-size: 36px;
  font-weight: 700;
  letter-spacing: -0.025em;
  line-height: 1;
  margin-top: 2px;
}

@media (max-width: 640px) {
  .account-id {
    gap: 14px;
    flex-wrap: wrap;
  }
  .account-addr-title {
    font-size: 13px;
    gap: 8px;
  }
  .account-known-title {
    font-size: 22px;
  }
  .balance-total {
    font-size: 28px;
  }
}

.bal-row {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 12.5px;
}

.swatch {
  width: 8px;
  height: 8px;
  border-radius: 2px;
  flex-shrink: 0;
}

.swatch-teal {
  background: var(--teal-500);
}

.swatch-red {
  background: var(--red-500);
}
</style>
