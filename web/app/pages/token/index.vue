<script setup lang="ts">
// Token hub — mainnet-only landing page summarising:
//   - the token's supply breakdown (top stats strip)
//   - the on-chain treasury account (left card)
//   - the epoch-based vesting schedule (right card)
//   - per-envelope distribution progression (grid)
//
// Backend returns 404 on non-mainnet networks because
// `pallet-token-allocation` only exists in the production runtime; the
// nav entry is hidden in those cases, but a direct URL hit still falls
// through to a graceful "not available" panel.

import type { EnvelopeInfo, TokenOverview } from '@bindings'
import { blocksToHumanDuration, blocksToMs } from '~/utils/blocks'
import {
  ENVELOPE_CATEGORIES,
  type EnvelopeCategory,
  categorySupplyPct,
  envelopeMeta,
} from '~/utils/envelopes'

definePageMeta({ name: 'token-hub' })
useSeoMeta({
  title: 'Token · Allfeat Explorer',
  description: 'Allfeat (AFT) token supply, treasury, and pre-launch envelope allocations.',
  ogType: 'website',
})

const { spec, isMainnet } = useActiveNetwork()
const { accountHref } = useNetworkLink()
const addrLabel = useAddrLabel()

const { data: overview, pending, error, refresh } = useNetworkFetch<TokenOverview | null>(
  net => `token-overview:${net}`,
  net => `/networks/${net}/token/overview`,
  { default: () => null },
)

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'Token' },
]

const blocksToNextPayout = computed(() => {
  const o = overview.value
  if (!o) return 0n
  return BigInt(o.epoch.next_payout_block) - BigInt(o.epoch.head_block)
})

const nextPayoutLabel = computed(() => {
  const blocks = blocksToNextPayout.value
  if (blocks <= 0n) return 'imminent'
  return `~${blocksToHumanDuration(blocks)}`
})

const nextPayoutMs = computed(() => blocksToMs(blocksToNextPayout.value))

const treasuryHref = computed(() => {
  const addr = overview.value?.treasury.account
  return addr ? accountHref(addr) : '#'
})

// Group envelopes by their frontend-only category so the grid reads as
// "Team & Advisory / Public Sale / Ecosystem / Operations" sections rather
// than one long list of 13 cards.
const envelopesByCategory = computed(() => {
  const buckets: Record<EnvelopeCategory, EnvelopeInfo[]> = {
    'team-advisory': [],
    'public-sale': [],
    'ecosystem': [],
    'operations': [],
  }
  const envs = overview.value?.envelopes ?? []
  for (const env of envs) {
    const meta = envelopeMeta(env.id)
    if (meta) buckets[meta.category].push(env)
  }
  return ENVELOPE_CATEGORIES
    .map(cat => ({
      ...cat,
      envelopes: buckets[cat.id],
      supplyPct: categorySupplyPct(cat.id),
    }))
    .filter(c => c.envelopes.length > 0)
})
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div class="token-hero">
      <h1 class="token-h1">
        Explore the life of the <span class="accent">{{ spec?.token ?? 'AFT' }}</span> token.
      </h1>
      <p class="token-lede">
        Live view of circulating supply, treasury, and genesis envelopes — read
        straight from the chain head as vesting unlocks, payouts fire, and
        allocations get claimed.
      </p>
    </div>

    <!-- Mainnet-only feature; render a clean fallback if a user lands here on
         a testnet network via direct URL. -->
    <div v-if="!isMainnet" class="panel" style="margin-top: 32px; padding: 32px; text-align: center;">
      <h3 style="margin-top: 0;">Not available on this network</h3>
      <p class="dim">
        The token allocation pallet ships in the Allfeat mainnet runtime only.
        Switch to Allfeat (top-right) to see the {{ spec?.token ?? 'AFT' }} supply breakdown.
      </p>
    </div>

    <template v-else>
      <div v-if="overview" style="margin-top: 28px;">
        <TokenSupplyStrip :overview="overview" />
      </div>

      <div v-if="overview" class="panel" style="margin-top: 16px; padding: 20px 22px;">
        <div class="supply-bar-head">
          <div class="ui-label">Supply distribution</div>
          <div class="dim text-xs mono">
            {{ fmtAFT(overview.total_supply, overview.decimals, 0) }} {{ overview.symbol }} total
          </div>
        </div>
        <SupplyBreakdownBar :overview="overview" />
      </div>

      <div class="panel" style="margin-top: 16px; padding: 20px 22px;">
        <TokenEmissionChart />
      </div>

      <div class="token-info-grid">
        <!-- Treasury card -->
        <div class="panel" v-if="overview">
          <div class="panel-head">
            <h3>
              <svg
                class="panel-head-icon"
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M3 10 12 4l9 6" />
                <path d="M5 10v9h14v-9" />
                <path d="M8 19v-6M12 19v-6M16 19v-6" />
                <path d="M3 19h18" />
              </svg>
              Treasury
            </h3>
            <span class="tag">{{ overview.symbol }}</span>
          </div>
          <div class="treasury-body">
            <div class="treasury-stats">
              <div class="treasury-stat">
                <div class="ui-label">Free</div>
                <div class="treasury-amount">
                  {{ fmtAFT(overview.treasury.balance, overview.decimals, 2) }}
                  <span class="treasury-unit">{{ overview.symbol }}</span>
                </div>
              </div>
              <div class="treasury-stat">
                <div class="ui-label">Locked</div>
                <div class="treasury-amount treasury-amount-sub">
                  {{ fmtAFT(overview.treasury.locked, overview.decimals, 2) }}
                  <span class="treasury-unit">{{ overview.symbol }}</span>
                </div>
              </div>
            </div>
            <div class="treasury-addr">
              <NuxtLink class="hash" :to="treasuryHref">
                {{ addrLabel(overview.treasury.account) }}
              </NuxtLink>
              <CopyButton :text="overview.treasury.account" />
            </div>
          </div>
        </div>

        <!-- Epoch / next payout card -->
        <div class="panel" v-if="overview">
          <div class="panel-head">
            <h3>
              <svg
                class="panel-head-icon"
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <circle cx="12" cy="12" r="8" />
                <path d="M12 7v5l3 2" />
              </svg>
              Vesting schedule
            </h3>
            <span class="tag">epoch #{{ overview.epoch.index }}</span>
          </div>
          <Kv>
            <KvRow label="Head block">
              <span class="mono">#{{ fmtInt(overview.epoch.head_block) }}</span>
            </KvRow>
            <KvRow label="Next payout block">
              <span class="mono">#{{ fmtInt(overview.epoch.next_payout_block) }}</span>
            </KvRow>
            <KvRow label="Time to next payout">
              <span class="mono">{{ nextPayoutLabel }}</span>
              <span v-if="nextPayoutMs > 0" class="dim">
                · <TimeAgo :timestamp="Date.now() + nextPayoutMs" />
              </span>
            </KvRow>
            <KvRow label="Epoch duration">
              <span class="mono">{{ blocksToHumanDuration(overview.epoch.epoch_duration_blocks) }}</span>
              <span class="dim mono text-xs">
                · {{ fmtInt(overview.epoch.epoch_duration_blocks) }} blocks
              </span>
            </KvRow>
          </Kv>
        </div>
      </div>

      <div class="page-title" style="margin-top: 32px;">
        <div>
          <h1>Envelopes</h1>
        </div>
        <div class="navs">
          <button
            type="button"
            class="icon-square"
            aria-label="Refresh"
            :disabled="pending"
            @click="() => refresh()"
          >
            ↻
          </button>
        </div>
      </div>

      <div v-if="overview">
        <section
          v-for="group in envelopesByCategory"
          :key="group.id"
          class="env-group"
        >
          <div class="env-group-head">
            <div class="env-group-title">
              <h2>{{ group.label }}</h2>
              <span class="env-group-pct mono">{{ fmtPct(group.supplyPct) }} of supply</span>
            </div>
            <p class="env-group-blurb dim">{{ group.blurb }}</p>
          </div>
          <div class="env-grid">
            <EnvelopeCard
              v-for="env in group.envelopes"
              :key="env.id"
              :envelope="env"
              :decimals="overview.decimals"
              :symbol="overview.symbol"
            />
          </div>
        </section>
      </div>
      <div v-else-if="pending" style="margin-top: 20px;">
        <SkeletonRows :rows="6" :columns="['1fr', '1fr', '1fr']" />
      </div>
      <p v-else-if="error" class="dim" style="padding: 40px; text-align: center;">
        Failed to load token overview.
      </p>
    </template>
  </section>
</template>

<style scoped>
.token-hero {
  padding-bottom: 28px;
  border-bottom: 1px solid var(--line);
}

.token-h1 {
  font-family: var(--font-display);
  font-size: clamp(40px, 5.5vw, 64px);
  line-height: 0.95;
  letter-spacing: -0.04em;
  font-weight: 700;
  text-wrap: balance;
  max-width: 900px;
}

.accent {
  color: var(--teal-500);
  font-style: italic;
}

.token-lede {
  color: var(--ink-dim);
  font-size: 16px;
  max-width: 620px;
  margin-top: 18px;
  line-height: 1.5;
}

.token-info-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
  margin-top: 16px;
}

@media (max-width: 900px) {
  .token-info-grid {
    grid-template-columns: 1fr;
  }
}

.supply-bar-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}

.panel-head-icon {
  color: var(--teal-500);
  margin-right: 6px;
  vertical-align: -3px;
}

.treasury-body {
  padding: 18px 22px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.treasury-stats {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 20px;
}

.treasury-stat {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.treasury-amount {
  font-family: var(--font-display);
  font-size: 24px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.05;
  word-break: break-word;
}

.treasury-amount-sub {
  color: var(--ink-dim);
}

.treasury-unit {
  font-size: 12px;
  font-weight: 500;
  color: var(--ink-dimmer);
  margin-left: 3px;
}

.treasury-addr {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 10px;
  border-top: 1px solid var(--line);
  font-family: var(--font-mono);
  font-size: 12.5px;
}

.env-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 16px;
  margin-top: 8px;
}

.env-group {
  margin-top: 28px;
}

.env-group:first-child {
  margin-top: 16px;
}

.env-group-head {
  padding-bottom: 12px;
  margin-bottom: 4px;
  border-bottom: 1px solid var(--line);
}

.env-group-title {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
}

.env-group-title h2 {
  font-family: var(--font-display);
  font-size: 20px;
  font-weight: 700;
  letter-spacing: -0.02em;
  margin: 0;
}

.env-group-pct {
  font-size: 12px;
  color: var(--ink-dimmer);
  letter-spacing: 0.04em;
}

.env-group-blurb {
  font-size: 13px;
  margin: 6px 0 0;
  max-width: 720px;
  line-height: 1.45;
}

@media (max-width: 520px) {
  .treasury-stats {
    grid-template-columns: 1fr;
    gap: 14px;
  }
}
</style>
