<script setup lang="ts">
// Upgrade history timeline — one row per indexed `system.set_code` event,
// plus a graceful empty state when no upgrades have been observed yet.
// The "current" row is highlighted; every other row is rendered as a
// neutral historical entry.
//
// `upgradeBlockNumber` normalises `first_block` (string | null on the
// wire): `null` means the backend couldn't determine a deployment block
// (RPC-only fallback), `"0"` is the legit genesis block, everything else
// is a real upgrade height.

import type { NetworkSpec, RuntimeUpgrade } from '@bindings'
import { fmtInt, fmtSpecVersionDotted } from '~/utils/format'
import { timeAgo } from '~/utils/time'

defineProps<{
  upgrades: RuntimeUpgrade[]
  spec: NetworkSpec | null
  now: number
}>()

const { blockHref } = useNetworkLink()
const PLACEHOLDER = '—'

function upgradeBlockNumber(u: RuntimeUpgrade): number | null {
  if (u.first_block == null) return null
  const n = Number(u.first_block)
  return Number.isFinite(n) && n >= 0 ? n : null
}

function upgradeWhen(u: RuntimeUpgrade, now: number): string {
  const n = upgradeBlockNumber(u)
  if (n == null) return PLACEHOLDER
  if (n === 0 || u.first_block_timestamp_ms == null) return 'genesis'
  return timeAgo(u.first_block_timestamp_ms, now)
}

function upgradeBlockLabel(u: RuntimeUpgrade): string {
  const n = upgradeBlockNumber(u)
  if (n == null) return PLACEHOLDER
  return n === 0 ? '#0' : `#${fmtInt(n)}`
}

function upgradeBlockLink(u: RuntimeUpgrade): string | null {
  const n = upgradeBlockNumber(u)
  return n != null ? blockHref(n) : null
}
</script>

<template>
  <div class="panel">
    <div class="panel-head">
      <h3>Upgrade history</h3>
      <span class="tag">system.set_code</span>
    </div>
    <div class="upg-timeline">
      <div
        v-for="u in upgrades"
        :key="`${u.spec_version}-${u.first_block ?? 'unknown'}`"
        class="upg-row"
        :class="{ current: u.is_current }"
      >
        <div class="when">{{ upgradeWhen(u, now) }}</div>
        <div class="dotcol">
          <span class="dot" />
        </div>
        <div class="body">
          <div class="title">
            <span class="spec">{{ fmtSpecVersionDotted(u.spec_version, PLACEHOLDER) }}</span>
            <span v-if="u.is_current">Active on {{ spec?.name ?? 'the chain' }}</span>
            <span v-else>Historical spec</span>
          </div>
        </div>
        <NuxtLink
          v-if="upgradeBlockLink(u)"
          class="block-link"
          :to="upgradeBlockLink(u)!"
        >
          {{ upgradeBlockLabel(u) }}
        </NuxtLink>
        <span v-else class="block-link dim">{{ PLACEHOLDER }}</span>
      </div>
      <div v-if="!upgrades || upgrades.length === 0" class="upg-history-note">
        The indexer has not yet observed any runtime upgrades for this
        network. The row will populate automatically once a
        <span class="mono">system.CodeUpdated</span> event is indexed.
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.upg-timeline {
  padding: 6px 22px 18px;
  position: relative;
}

.upg-row {
  display: grid;
  grid-template-columns: 92px 16px 1fr auto;
  align-items: center;
  padding: 12px 0;
  gap: 16px;
  border-bottom: 1px dashed var(--line);

  &:last-of-type {
    border-bottom: 0;
  }

  .when {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-dimmer);
    letter-spacing: 0.04em;
    text-align: right;
  }

  .dotcol {
    position: relative;
    height: 100%;
    display: grid;
    place-items: center;

    // Vertical connector between the dots. The `::before` draws a
    // full-height 1px line; the first and last rows trim it at the
    // dot so the timeline doesn't float out of the panel.
    &::before {
      content: '';
      position: absolute;
      top: -12px;
      bottom: -12px;
      left: 50%;
      width: 1px;
      background: var(--line);
    }
  }

  &:first-child .dotcol::before {
    top: 50%;
  }

  &:last-child .dotcol::before {
    bottom: 50%;
  }

  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--bg-1);
    border: 2px solid var(--ink-dim);
    position: relative;
    z-index: 1;
  }

  &.current .dot {
    background: var(--teal-500);
    border-color: var(--teal-500);
    box-shadow: 0 0 0 4px rgba(0, 177, 140, 0.15);
  }

  .body .title {
    font-family: var(--font-display);
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    display: flex;
    align-items: baseline;
    gap: 10px;
    flex-wrap: wrap;

    .spec {
      color: var(--teal-500);
      font-family: var(--font-mono);
      font-size: 12px;
      font-weight: 500;
    }
  }

  .block-link {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
}

[data-theme="light"] {
  .upg-row.current .dot {
    background: var(--teal-700);
    border-color: var(--teal-700);
    box-shadow: 0 0 0 4px rgba(5, 138, 110, 0.18);
  }

  .upg-row .body .title .spec {
    color: var(--teal-700);
  }
}

.upg-history-note {
  margin-top: 12px;
  padding: 12px 14px;
  border: 1px dashed var(--line);
  font-size: 12px;
  color: var(--ink-dim);
  line-height: 1.5;

  .mono {
    color: var(--ink);
  }
}
</style>
