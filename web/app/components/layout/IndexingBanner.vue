<script setup lang="ts">
// Top-of-page banner that surfaces indexer health for the active
// network. Polls /api/v1/indexing/status every 5s on the client; the
// initial render is seeded by useFetch so SSR produces the correct
// markup for catching-up / backfill states without a post-hydrate jump.
//
// Hidden when the active network is Healthy (the common case) so the
// banner doesn't eat real estate when nothing is wrong.

import { computed } from 'vue'
import { useIntervalFn } from '@vueuse/core'
import type { IndexerStatus } from '@bindings'
import { useActiveNetwork } from '~/composables/useActiveNetwork'
import { apiBaseUrl } from '~/composables/useApi'

const { id: activeId } = useActiveNetwork()

const { data, refresh } = await useFetch<IndexerStatus[]>('/indexing/status', {
  key: 'indexing-status',
  baseURL: apiBaseUrl(),
  default: (): IndexerStatus[] => [],
})

// Client-side poll. useIntervalFn auto-cleans on unmount and is a no-op
// server-side (guards internally on `window`). 5s matches the plan.
if (import.meta.client) {
  useIntervalFn(refresh, 5_000)
}

const status = computed<IndexerStatus | null>(() => {
  const list = data.value
  if (!list || list.length === 0) return null
  const id = activeId.value
  if (!id) return list[0] ?? null
  return list.find(s => s.network_id === id) ?? null
})

const visible = computed(() => {
  const s = status.value
  return s !== null && s.state !== 'Healthy'
})

const message = computed(() => {
  const s = status.value
  if (!s) return ''
  switch (s.state) {
    case 'Backfilling':
      return `Backfilling historical data · ${fmtPct(s.backfill_pct)}`
    case 'CatchingUp':
      return s.live_lag_blocks
        ? `Catching up · ${s.live_lag_blocks} blocks behind`
        : 'Catching up with the chain'
    case 'Offline':
      return 'Indexer offline — data may be stale'
    default:
      return ''
  }
})

const variant = computed(() => {
  const s = status.value
  if (!s) return 'info'
  switch (s.state) {
    case 'Backfilling': return 'info'
    case 'CatchingUp': return 'warn'
    case 'Offline': return 'error'
    default: return 'info'
  }
})
</script>

<template>
  <div v-if="visible" class="indexing-banner" :class="[`indexing-banner--${variant}`]" role="status">
    <div class="container indexing-banner__inner">
      <span class="indexing-banner__dot" />
      <span class="indexing-banner__text">{{ message }}</span>
      <span
        v-if="status?.state === 'Backfilling'"
        class="indexing-banner__bar"
        :style="{ '--pct': `${Math.min(100, Math.max(0, status.backfill_pct))}%` }"
      />
    </div>
  </div>
</template>

<style scoped lang="scss">
.indexing-banner {
  border-bottom: 1px solid var(--line);
  background: var(--bg-1);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-dim);

  &__inner {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 36px;
    padding: 6px 0;
    flex-wrap: wrap;
  }

  &__dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: currentColor;
    flex-shrink: 0;
  }

  &__text {
    letter-spacing: 0.02em;
  }

  &__bar {
    flex: 1;
    min-width: 120px;
    height: 3px;
    background: var(--line);
    border-radius: 2px;
    overflow: hidden;
    position: relative;

    &::after {
      content: '';
      position: absolute;
      inset: 0;
      width: var(--pct, 0%);
      background: currentColor;
      transition: width 0.8s ease;
    }
  }

  &--info {
    color: var(--teal-500);
    background: rgba(0, 177, 140, 0.06);
  }

  &--warn {
    color: var(--cream-200);
    background: rgba(196, 192, 177, 0.06);
  }

  &--error {
    color: var(--red-500);
    background: rgba(255, 74, 95, 0.06);
  }
}

[data-theme="light"] .indexing-banner--info { color: var(--teal-700); }
[data-theme="light"] .indexing-banner--error { color: var(--red-700); }
</style>
