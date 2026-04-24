<script setup lang="ts">
// Events list page.
//
// Cursor-based pagination on the chain-wide event feed. The server
// returns `{ items, page_info: { next_cursor, has_more } }`; the page
// walks cursors forward without a double round-trip (no more separate
// head fetch + per-block fan-out) and no BigInt arithmetic. Client-side
// filters are gone — Phase 3 will add `pallet` / `variant` query params.

import type { BlockEvent } from '@bindings'

definePageMeta({ name: 'events-list' })
useSeoMeta({
  title: 'Events · Allfeat Explorer',
  description: 'Paginated feed of runtime events emitted on the Allfeat chain.',
  ogTitle: 'Events · Allfeat',
  ogDescription: 'Browse runtime events on Allfeat.',
  ogType: 'website',
})

const {
  items: events,
  pending,
  error,
  hasMore,
  canGoBack,
  goBack,
  goNext,
} = usePaginatedList<BlockEvent>({
  keyPrefix: 'events-list',
  path: '/events',
  pageSize: 50,
})

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'Events' },
]

const skeletonCols = ['120px', '100px', '120px', '1fr', '140px', '1fr', '100px']
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div class="page-title">
      <div>
        <h1>Events</h1>
      </div>
    </div>

    <div class="panel" style="margin-top: 20px;">
      <EventsTable v-if="events.length > 0" :events="events" />
      <SkeletonRows v-else-if="pending" :rows="10" :columns="skeletonCols" />
      <p v-else-if="error" class="dim" style="padding: 40px; text-align: center;">
        Failed to load events.
      </p>
      <p v-else class="dim" style="padding: 40px; text-align: center;">
        No events to display.
      </p>

      <nav
        v-if="canGoBack || hasMore"
        class="cursor-nav"
        aria-label="Event pagination"
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
    </div>
  </section>
</template>

<style scoped>
.cursor-nav {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 14px 18px;
  border-top: 1px solid var(--line);
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
