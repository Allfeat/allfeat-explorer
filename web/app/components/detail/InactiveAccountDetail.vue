<script setup lang="ts">
// Detail view for a well-formed SS58 that the backend has no record of.
//
// Substrate accounts exist implicitly: any valid SS58 has a zero-balance
// account until the first event touches it. Surfacing a generic 404 here
// is actively misleading (explorers are a lookup tool, not a write tool),
// so we render a lean placeholder with the address, a zero balance, and
// enough copy that the user understands why there's nothing else to show.
//
// Layout mirrors `AccountDetail.vue`'s title cluster so jumping from one
// state to the other on navigation doesn't reflow the page.

import type { AddressFormat } from '~/components/ui/AddressFormatToggle.vue'
import { GENERIC_SS58_PREFIX, reencodeSs58 } from '~/utils/ss58'

interface Props {
  address: string
  networkName?: string | null
}
const props = defineProps<Props>()
const known = useKnownAccount(() => props.address)
const network = useActiveNetwork()

const networkPrefix = computed(() => network.spec.value?.ss58_prefix ?? GENERIC_SS58_PREFIX)
const canToggleFormat = computed(() => networkPrefix.value !== GENERIC_SS58_PREFIX)

const addressFormat = ref<AddressFormat>('mainnet')

const displayAddress = computed(() => {
  if (addressFormat.value === 'mainnet' || !canToggleFormat.value) return props.address
  try {
    return reencodeSs58(props.address, GENERIC_SS58_PREFIX)
  } catch {
    return props.address
  }
})
</script>

<template>
  <div>
    <div class="page-title account-title">
      <div class="account-id">
        <Identicon :seed="address" large />
        <div>
          <h1 v-if="known" class="account-known-title">
            {{ known.name }}
          </h1>
          <h1 class="account-addr-title" :class="{ secondary: !!known }">
            <span>{{ displayAddress }}</span>
            <CopyButton :text="displayAddress" />
            <AddressFormatToggle v-if="canToggleFormat" v-model="addressFormat" />
          </h1>
          <p v-if="known?.note" class="known-note">{{ known.note }}</p>
        </div>
      </div>
    </div>

    <div class="panel inactive-panel">
      <div class="panel-head">
        <h3>Overview</h3>
        <span class="tag">No activity</span>
      </div>
      <div class="inactive-body">
        <div class="inactive-zero-row">
          <div>
            <div class="ui-label">Balance</div>
            <div class="inactive-zero-val">
              0
              <span class="inactive-zero-unit">AFT</span>
            </div>
          </div>
        </div>
        <p class="inactive-message">
          This address has no on-chain record on {{ networkName ?? 'this network' }} yet.
        </p>
        <p class="inactive-sub">
          Substrate accounts exist implicitly — once a transfer, extrinsic, or
          ATS registration touches this address, its live balance and history
          will appear here automatically.
        </p>
      </div>
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
  color: var(--ink-dimmer);
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

.inactive-panel {
  margin-top: 28px;
}

.inactive-body {
  padding: 28px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.inactive-zero-row {
  padding-bottom: 20px;
  border-bottom: 1px solid var(--line);
}

.inactive-zero-val {
  font-family: var(--font-display);
  font-size: 36px;
  font-weight: 700;
  letter-spacing: -0.025em;
  line-height: 1;
  margin-top: 4px;
  color: var(--ink-dim);
}

.inactive-zero-unit {
  font-size: 20px;
  color: var(--ink-dimmer);
  margin-left: 6px;
}

.inactive-message {
  margin: 0;
  font-size: 14px;
  color: var(--ink);
}

.inactive-sub {
  margin: 0;
  font-size: 13px;
  color: var(--ink-dim);
  line-height: 1.55;
  max-width: 70ch;
}
</style>
