<script setup lang="ts">
// ATS detail → Overview tab. Renders the on-chain KV record on the
// left, plus a "what this proves" blurb and the signature waveform on
// the right.

import type { AtsRecord, AtsVersion } from '@bindings'
import { fmtAFT, fmtInt, fmtUtcTime } from '~/utils/format'

const props = defineProps<{
  record: AtsRecord
  selected: AtsVersion
}>()

const { blockHref, extrinsicHref, accountHref } = useNetworkLink()

const versionCount = computed(() => props.record.version_count)
const multiVersion = computed(() => versionCount.value > 1)
const isInitial = computed(() => props.selected.version_index === 0)
const isLatest = computed(() => props.selected.version_index === versionCount.value - 1)
const callName = computed(() => (isInitial.value ? 'ats.register' : 'ats.add_version'))
const totalDepositors = computed(() => props.record.deposits.length)
</script>

<template>
  <div class="ats-grid">
    <div class="panel">
      <div class="panel-head">
        <h3>On-chain record</h3>
        <span class="tag">
          <template v-if="multiVersion">
            Version {{ selected.version_index + 1 }} of {{ versionCount }}
          </template>
          <template v-else>Immutable</template>
          · block {{ fmtInt(selected.block_number) }}
        </span>
      </div>
      <Kv>
        <KvRow label="Registry ID">
          <span class="mono">ats_id <span class="dim">=</span>
            <strong style="color: var(--teal-500);">#{{ record.ats_id }}</strong>
            <span class="dim"> · shared across all versions</span>
          </span>
        </KvRow>
        <KvRow label="Version">
          <span class="mono" style="display: inline-flex; align-items: center; gap: 8px;">
            <strong>v{{ selected.version_index + 1 }}</strong>
            <span class="dim">of {{ versionCount }}</span>
            <Chip v-if="isInitial">initial</Chip>
            <Chip v-if="isLatest && multiVersion" variant="module">latest</Chip>
            <Chip v-if="!isInitial && !isLatest">revision</Chip>
          </span>
        </KvRow>
        <KvRow label="Commitment">
          <div style="display: flex; gap: 6px; align-items: center; min-width: 0;">
            <span class="hash" style="word-break: break-all;">{{ selected.commitment }}</span>
            <CopyButton :text="selected.commitment" />
          </div>
        </KvRow>
        <KvRow label="Protocol version">
          <span class="mono">v{{ selected.protocol_version }}</span>
          <span class="dim"> (SHA-256, Merkle d=5)</span>
        </KvRow>
        <KvRow label="Owner">
          <Addr :text="record.owner" :to="accountHref(record.owner)" />
        </KvRow>
        <KvRow label="Submitted by">
          <span style="display: inline-flex; align-items: center; gap: 8px;">
            <Addr :text="selected.signer" :to="accountHref(selected.signer)" />
            <Chip v-if="selected.signer !== record.owner">co-depositor</Chip>
          </span>
        </KvRow>
        <KvRow label="Block">
          <NuxtLink class="hash" :to="blockHref(selected.block_number)">
            #{{ fmtInt(selected.block_number) }}
          </NuxtLink>
        </KvRow>
        <KvRow label="Extrinsic">
          <NuxtLink class="hash" :to="extrinsicHref(selected.extrinsic_id)">
            {{ selected.extrinsic_id }}
          </NuxtLink>
          <span class="dim"> · {{ callName }}</span>
        </KvRow>
        <KvRow label="Timestamp">
          <span class="mono">{{ fmtUtcTime(selected.created_at_ms) }}</span>
        </KvRow>
        <KvRow label="Fee paid">
          <span class="mono">{{ fmtAFT(selected.fee, 12, 6) }} AFT</span>
        </KvRow>
        <KvRow label="Total deposits bonded">
          <span class="mono">{{ fmtAFT(record.total_deposit, 12, 2) }} AFT</span>
          <span class="dim"> · {{ totalDepositors }} depositor{{ totalDepositors !== 1 ? 's' : '' }}</span>
        </KvRow>
        <KvRow label="Status">
          <span class="status-active">
            <LiveDot />Active
          </span>
        </KvRow>
      </Kv>
    </div>

    <div class="ats-side">
      <div class="panel">
        <div class="panel-head">
          <h3>What this proves</h3>
        </div>
        <div class="prose">
          <p>
            This ATS is a cryptographic commitment to a musical work — its media,
            title, and creators — submitted to Allfeat at block
            <strong>#{{ fmtInt(selected.block_number) }}</strong>.
          </p>
          <p v-if="multiVersion">
            The registry ID <strong>#{{ record.ats_id }}</strong> carries {{ versionCount }} versions —
            each a new commitment replacing the previous one (e.g. remaster, credit correction, new
            co-writer). Every version is independently verifiable.
          </p>
          <p v-else>
            Any holder of the original inputs can later prove they existed at the registration time,
            without revealing them publicly. Individual creators can prove inclusion via Merkle path.
          </p>
        </div>
      </div>

      <div class="panel">
        <div class="panel-head">
          <h3>Signature</h3>
          <span class="tag">v{{ selected.version_index + 1 }} fingerprint</span>
        </div>
        <div class="signature-body">
          <AtsWave :commitment="selected.commitment" :bars="56" :height="50" />
          <div class="mono text-xs dim signature-cap">
            unique visual fingerprint · not cryptographic
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ats-grid {
  display: grid;
  grid-template-columns: 1.5fr 1fr;
  gap: 20px;
}

@media (max-width: 960px) {
  .ats-grid {
    grid-template-columns: 1fr;
  }
}

.ats-side {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.prose {
  padding: 18px 22px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--ink-dim);
}

.prose p {
  margin: 0;
}

.prose p + p {
  margin-top: 10px;
}

.prose strong {
  color: var(--ink);
}

.signature-body {
  padding: 22px 22px 26px;
}

@media (max-width: 640px) {
  .signature-body {
    padding: 16px 14px;
  }
}

.signature-cap {
  margin-top: 12px;
  text-align: center;
}

.status-active {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 600;
  color: var(--teal-500);
  background: rgba(0, 177, 140, 0.1);
  border: 1px solid rgba(0, 177, 140, 0.2);
}
</style>
