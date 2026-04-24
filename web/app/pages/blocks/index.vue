<script setup lang="ts">
// Blocks list page.
//
// Cursor-based pagination (see docs/api-pagination-plan.md). The server
// returns `{ items, page_info: { total, next_cursor, has_more } }`; the
// page walks cursors forward with a single round-trip, no head lookup
// and no BigInt math on block numbers. `?cursor=<block>` is the URL
// shape — bookmarkable and reversible via the browser's back button.
//
// We don't consume the live store here: list pages let the user walk
// back through history, so mutating their rows under them would be
// jarring. Live animation lives on the dashboard (pages/index.vue).

import type { Block } from '@bindings'

definePageMeta({ name: 'blocks-list' })
useSeoMeta({
  title: 'Blocks · Allfeat Explorer',
  description: 'Paginated history of blocks produced by the Allfeat chain.',
  ogTitle: 'Blocks · Allfeat',
  ogDescription: 'Browse every block produced on Allfeat.',
  ogType: 'website',
})

const {
  items: blocks,
  pending,
  error,
  total,
  hasMore,
  canGoBack,
  goBack,
  goNext,
} = usePaginatedList<Block>({
  keyPrefix: 'blocks-list',
  path: '/blocks',
  pageSize: 25,
})

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'Blocks' },
]

const skeletonCols = ['90px', '100px', '1fr', '80px', '80px', '180px', '90px', '100px']
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div class="page-title">
      <div>
        <h1>Blocks</h1>
        <p v-if="total" class="dim" style="margin-top: 4px;">
          {{ Number(total).toLocaleString('en-US') }} total
        </p>
      </div>
    </div>

    <div class="panel" style="margin-top: 20px;">
      <BlocksTable v-if="blocks.length > 0" :blocks="blocks" />
      <SkeletonRows v-else-if="pending" :rows="10" :columns="skeletonCols" />
      <p v-else-if="error" class="dim" style="padding: 40px; text-align: center;">
        Failed to load blocks.
      </p>
      <p v-else class="dim" style="padding: 40px; text-align: center;">
        No blocks to display.
      </p>

      <nav
        v-if="canGoBack || hasMore"
        class="cursor-nav"
        aria-label="Block pagination"
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
