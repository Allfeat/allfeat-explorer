<script setup lang="ts">
// ATS index page — Allfeat Timestamp registry. Top: hero copy + stats
// strip. Below: cursor-paginated version feed. The server returns
// `Page<AtsFeedItem>` so there's no more offset arithmetic; Next/Prev
// follow the cursor chain and `page_info.total` still drives the
// numeric counter in the hero strip when available.

import type { AtsFeedItem, AtsStats } from '@bindings'

definePageMeta({ name: 'ats-list' })
useSeoMeta({
  title: 'ATS · Allfeat Explorer',
  description: 'Allfeat Timestamp registry — every song on Allfeat gets a cryptographic heartbeat.',
  ogTitle: 'ATS · Allfeat',
  ogDescription: 'A tiny on-chain timestamp that says: this track existed, right here, right now.',
  ogType: 'website',
})

const { data: stats } = useNetworkFetch<AtsStats | null>(
  net => `ats-stats:${net}`,
  net => `/networks/${net}/ats/stats`,
  { default: () => null },
)

const {
  items,
  pending,
  error,
  hasMore,
  canGoBack,
  goBack,
  goNext,
  refresh,
} = usePaginatedList<AtsFeedItem>({
  keyPrefix: 'ats-feed',
  path: '/ats/feed',
  pageSize: 25,
})

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'ATS' },
]

async function handleRefresh() {
  await refresh()
}
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div class="ats-hero">
      <h1 class="ats-h1">
        Every song gets<br>
        a <span style="color: var(--teal-500); font-style: italic;">heartbeat</span>.
      </h1>
      <p class="ats-lede">
        A tiny on-chain timestamp that says <em>this track existed, right here, right now</em> —
        forever. No paperwork, no middlemen.
      </p>

      <div v-if="stats" style="margin-top: 28px;">
        <AtsStatsStrip :stats="stats" />
      </div>
    </div>

    <div class="page-title" style="margin-top: 32px;">
      <div>
        <h1>All timestamps</h1>
      </div>
      <div class="navs">
        <button
          type="button"
          class="icon-square"
          aria-label="Refresh feed"
          :disabled="pending"
          @click="handleRefresh"
        >
          ↻
        </button>
      </div>
    </div>

    <AtsTimeline v-if="items.length > 0" :items="items" />
    <div v-else-if="pending" style="margin-top: 20px;">
      <SkeletonRows :rows="8" :columns="['120px', '1fr', '60px', '80px']" />
    </div>
    <p v-else-if="error" class="dim" style="padding: 40px; text-align: center;">
      Failed to load ATS feed.
    </p>
    <p v-else class="dim" style="padding: 40px; text-align: center;">
      No ATS registrations found.
    </p>

    <nav
      v-if="canGoBack || hasMore"
      class="cursor-nav"
      aria-label="ATS feed pagination"
    >
      <button
        type="button"
        :disabled="!canGoBack"
        aria-label="Previous page"
        @click="goBack"
      >
        ‹ Previous
      </button>
      <button
        type="button"
        :disabled="!hasMore"
        aria-label="Next page"
        @click="goNext"
      >
        Next ›
      </button>
    </nav>
  </section>
</template>

<style scoped>
.ats-hero {
  padding-bottom: 28px;
  border-bottom: 1px solid var(--line);
}

.ats-h1 {
  font-family: var(--font-display);
  font-size: clamp(44px, 6vw, 72px);
  line-height: 0.95;
  letter-spacing: -0.04em;
  font-weight: 700;
  text-wrap: balance;
  max-width: 900px;
}

.ats-lede {
  color: var(--ink-dim);
  font-size: 16px;
  max-width: 560px;
  margin-top: 18px;
  line-height: 1.5;
}

.cursor-nav {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}
.cursor-nav button {
  appearance: none;
  background: var(--panel-2, var(--panel));
  color: var(--text);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 6px 12px;
  cursor: pointer;
  font-size: 13px;
}
.cursor-nav button:disabled {
  opacity: 0.4;
  cursor: default;
}
.cursor-nav button:not(:disabled):hover {
  border-color: var(--accent, var(--text));
}
</style>
