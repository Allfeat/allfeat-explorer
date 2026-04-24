<script setup lang="ts">
// Account detail page — /accounts/:address. Two fetches in parallel:
// the account itself and the owner-scoped ATS page. The ATS response
// is now `Page<AtsRecord>`; `page_info.total` replaces the dedicated
// `/ats/count` endpoint (removed in the pagination migration).

import type { Account, Allocation, AtsRecord, Page, TokenOverview } from '@bindings'
import { detectKinds } from '~/utils/searchPatterns'

const route = useRoute()
const address = computed<string>(() => String(route.params.address ?? ''))

// Token allocations only ship on the production runtime; gating the
// fetch on `isMainnet` avoids hammering the testnet API with guaranteed 404s.
const { spec, isMainnet } = useActiveNetwork()
const addrLabel = useAddrLabel()

// A 404 on a well-formed SS58 address means "no activity yet", not "not
// found" — every valid address exists implicitly on a Substrate chain.
// We branch on shape here so only truly malformed paths land in the
// generic NotFound view.
const isValidShape = computed(() =>
  detectKinds(address.value).kinds.has('account'),
)

const { data: account, error } = useNetworkFetch<Account | null>(
  net => `account-detail:${net}:${address.value}`,
  net => `/networks/${net}/accounts/${address.value}`,
  { default: () => null, watch: [address] },
)

const { data: atsPage } = useNetworkFetch<Page<AtsRecord>>(
  net => `account-ats:${net}:${address.value}`,
  net => `/networks/${net}/accounts/${address.value}/ats?count=10`,
  {
    default: (): Page<AtsRecord> => ({
      items: [],
      page_info: { total: null, next_cursor: null, has_more: false },
    }),
    watch: [address],
  },
)

const ats = computed<AtsRecord[]>(() => atsPage.value?.items ?? [])
// `page_info.total` is a string (u64) so we cast via Number — u32 caps
// well below MAX_SAFE_INTEGER, no BigInt needed.
const atsCount = computed<number>(() => {
  const t = atsPage.value?.page_info.total
  return t != null ? Number(t) : 0
})

// Allocations endpoint 404s on non-mainnet (the pallet doesn't ship
// there); we still fire the fetch and let it fall through to its empty
// default — a single benign 404 per page load is cheaper than threading
// an `enabled` flag through the wrapper.
const { data: allocations } = useNetworkFetch<Allocation[]>(
  net => `account-allocations:${net}:${address.value}`,
  net => `/networks/${net}/accounts/${address.value}/allocations`,
  { default: (): Allocation[] => [], watch: [address] },
)

// Token overview feeds the per-envelope vesting duration + epoch head /
// next-payout block pair that the allocations panel uses to compute the
// "next payout" estimate and countdown. Same 404 semantics as the
// allocations fetch above on testnet networks — falls through to its
// empty default.
const { data: tokenOverview } = useNetworkFetch<TokenOverview | null>(
  net => `account-token-overview:${net}`,
  net => `/networks/${net}/token/overview`,
  { default: (): TokenOverview | null => null },
)

const breadcrumb = computed(() => [
  { label: 'Home', to: '/' },
  { label: 'Accounts', to: '/accounts' },
  { label: addrLabel(address.value) },
])

definePageMeta({ name: 'account-detail' })
useSeoMeta({
  title: () => account.value
    ? `${addrLabel(account.value.address)} · Account · Allfeat Explorer`
    : 'Account · Allfeat Explorer',
  description: () => account.value
    ? `Account ${addrLabel(account.value.address)} on ${spec.value?.name ?? 'Allfeat'}.`
    : 'Account details on Allfeat.',
  ogType: 'profile',
})
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <LazyAccountDetail
      v-if="account"
      :account="account"
      :ats="ats"
      :ats-count="atsCount"
      :allocations="isMainnet ? allocations : []"
      :token-symbol="spec?.token ?? 'AFT'"
      :token-overview="isMainnet ? tokenOverview : null"
      :block-time-secs="Number(spec?.block_time_secs ?? 6)"
    />

    <InactiveAccountDetail
      v-else-if="error && isValidShape"
      :address="address"
      :network-name="spec?.name"
    />

    <NotFoundPanel
      v-else-if="error"
      entity="Account"
      :name="addrLabel(address)"
      :network-name="spec?.name"
      back-to="/accounts"
      back-label="Back to accounts"
    />

    <div v-else class="panel" style="margin-top: 40px;">
      <div class="panel-body">
        <SkeletonRows :rows="6" :columns="['160px', '1fr']" />
      </div>
    </div>
  </section>
</template>
