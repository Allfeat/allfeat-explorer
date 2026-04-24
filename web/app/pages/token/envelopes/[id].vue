<script setup lang="ts">
// Envelope detail — drilldown from the /token hub. Shows the envelope
// config + a flat allocations table. URL slug doubles as the EnvelopeId
// on the wire so the route param is passed through verbatim.

import type { EnvelopeDetail, TokenOverview } from '@bindings'
import { blocksToHumanDuration } from '~/utils/blocks'
import { envelopeMeta } from '~/utils/envelopes'

const route = useRoute()
const slug = computed<string>(() => String(route.params.id ?? ''))

const { spec } = useActiveNetwork()
const { accountHref } = useNetworkLink()
const addrLabel = useAddrLabel()

// Pull the overview alongside the detail so the page can quote chain-wide
// context (token symbol, decimals, head block) without re-deriving it.
const { data: overview } = useNetworkFetch<TokenOverview | null>(
  net => `token-overview:${net}`,
  net => `/networks/${net}/token/overview`,
  { default: () => null },
)

const { data: detail, pending, error } = useNetworkFetch<EnvelopeDetail | null>(
  net => `envelope-detail:${net}:${slug.value}`,
  net => `/networks/${net}/token/envelopes/${slug.value}`,
  { default: () => null, watch: [slug] },
)

const decimals = computed(() => overview.value?.decimals ?? 12)
const symbol = computed(() => overview.value?.symbol ?? spec.value?.token ?? 'AFT')

const remainingCap = computed(() => {
  const env = detail.value?.envelope
  if (!env) return '0'
  return (BigInt(env.total_cap) - BigInt(env.distributed)).toString()
})

const distributedPct = computed(() => {
  const env = detail.value?.envelope
  if (!env) return 0
  const cap = BigInt(env.total_cap)
  if (cap === 0n) return 0
  return Math.min(100, Number((BigInt(env.distributed) * 10000n) / cap) / 100)
})

const breadcrumb = computed(() => [
  { label: 'Home', to: '/' },
  { label: 'Token', to: '/token' },
  { label: detail.value?.envelope.label ?? slug.value },
])

const uniqueBeneficiaryHref = computed(() => {
  const addr = detail.value?.envelope.unique_beneficiary
  return addr ? accountHref(addr) : null
})

const meta = computed(() => {
  const env = detail.value?.envelope
  return env ? envelopeMeta(env.id) : undefined
})

definePageMeta({ name: 'token-envelope-detail' })
useSeoMeta({
  title: () => detail.value?.envelope.label
    ? `${detail.value.envelope.label} · Token · Allfeat Explorer`
    : 'Envelope · Token · Allfeat Explorer',
  description: () => detail.value?.envelope.label
    ? `${detail.value.envelope.label} envelope: cap, distribution progress, vesting schedule, allocations.`
    : 'Token envelope detail.',
  ogType: 'website',
})

</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div v-if="detail">
      <div class="page-title">
        <div>
          <h1>{{ detail.envelope.label }}</h1>
          <p v-if="meta" class="env-detail-blurb dim">{{ meta.blurb }}</p>
        </div>
      </div>

      <div class="env-detail-grid">
        <div class="panel">
          <div class="panel-head">
            <h3>Cap & distribution</h3>
            <span class="tag">{{ symbol }}</span>
          </div>
          <div style="padding: 22px;">
            <div class="ui-label">Total cap</div>
            <div class="env-amount">
              {{ fmtAFT(detail.envelope.total_cap, decimals, 2) }}
              <span class="env-amount-unit">{{ symbol }}</span>
            </div>

            <div class="env-progress" style="margin-top: 18px;">
              <div class="env-progress-track">
                <div class="env-progress-fill" :style="{ width: `${distributedPct}%` }" />
              </div>
              <div class="env-progress-meta">
                <span class="mono">{{ fmtPct(distributedPct) }} distributed</span>
                <span class="dim mono text-xs">
                  {{ fmtAFT(detail.envelope.distributed, decimals, 2) }} / {{ fmtAFT(detail.envelope.total_cap, decimals, 2) }}
                </span>
              </div>
            </div>

            <div style="margin-top: 18px; display: flex; flex-direction: column; gap: 10px;">
              <div class="bal-row">
                <span class="swatch swatch-teal" />
                <span style="flex: 1;">Distributed</span>
                <span class="mono" style="font-weight: 600;">
                  {{ fmtAFT(detail.envelope.distributed, decimals, 2) }} {{ symbol }}
                </span>
              </div>
              <div class="bal-row">
                <span class="swatch swatch-dim" />
                <span style="flex: 1;">Reserve in envelope</span>
                <span class="mono" style="font-weight: 600;">
                  {{ fmtAFT(remainingCap, decimals, 2) }} {{ symbol }}
                </span>
              </div>
            </div>
          </div>
        </div>

        <div class="panel">
          <div class="panel-head">
            <h3>Vesting</h3>
          </div>
          <Kv>
            <KvRow label="Upfront">
              <span class="mono">{{ detail.envelope.upfront_pct }}%</span>
              <span class="dim text-xs"> paid at allocation</span>
            </KvRow>
            <KvRow label="Cliff">
              <span class="mono">{{ blocksToHumanDuration(detail.envelope.cliff_blocks) }}</span>
              <span class="dim mono text-xs"> · {{ fmtInt(detail.envelope.cliff_blocks) }} blocks</span>
            </KvRow>
            <KvRow label="Vesting duration">
              <span class="mono">{{ blocksToHumanDuration(detail.envelope.vesting_duration_blocks) }}</span>
              <span class="dim mono text-xs"> · {{ fmtInt(detail.envelope.vesting_duration_blocks) }} blocks</span>
            </KvRow>
            <KvRow label="Allocations">
              <span class="mono">{{ fmtInt(detail.envelope.allocation_count) }}</span>
            </KvRow>
            <KvRow label="Envelope account">
              <NuxtLink class="hash mono" :to="accountHref(detail.envelope.account)">
                {{ addrLabel(detail.envelope.account) }}
              </NuxtLink>
            </KvRow>
            <KvRow v-if="detail.envelope.unique_beneficiary" label="Unique beneficiary">
              <NuxtLink class="hash mono" :to="uniqueBeneficiaryHref ?? '#'">
                {{ addrLabel(detail.envelope.unique_beneficiary) }}
              </NuxtLink>
            </KvRow>
          </Kv>
        </div>
      </div>

      <div style="margin-top: 28px;">
        <EnvelopeAllocationsTable
          :allocations="detail.allocations"
          :decimals="decimals"
          :symbol="symbol"
        />
      </div>
    </div>

    <div v-else-if="pending" class="panel" style="margin-top: 28px;">
      <div class="panel-body">
        <SkeletonRows :rows="6" :columns="['160px', '1fr']" />
      </div>
    </div>

    <NotFoundPanel
      v-else-if="error"
      entity="Envelope"
      :name="slug"
      :network-name="spec?.name"
      back-to="/token"
      back-label="Back to token hub"
    />
  </section>
</template>

<style scoped>
.env-detail-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 24px;
  margin-top: 24px;
}

@media (max-width: 900px) {
  .env-detail-grid {
    grid-template-columns: 1fr;
  }
}

.env-detail-blurb {
  font-size: 14px;
  max-width: 680px;
  line-height: 1.5;
  margin: 8px 0 0;
}

.env-amount {
  font-family: var(--font-display);
  font-size: 32px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
  margin-top: 4px;
}

.env-amount-unit {
  font-size: 16px;
  font-weight: 500;
  color: var(--ink-dimmer);
  margin-left: 4px;
}

.env-progress {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.env-progress-track {
  height: 8px;
  border-radius: 4px;
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
  font-size: 12.5px;
}

.bal-row {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 13px;
}

.swatch {
  width: 10px;
  height: 10px;
  border-radius: 2px;
  flex-shrink: 0;
}

.swatch-teal {
  background: var(--teal-500);
}

.swatch-dim {
  background: var(--ink-dimmer);
}
</style>
