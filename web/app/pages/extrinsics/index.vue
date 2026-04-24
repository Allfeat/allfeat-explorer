<script setup lang="ts">
// Extrinsics list page.
//
// Cursor-based pagination (see docs/api-pagination-plan.md). The server
// returns `{ items, page_info }`; cursors are opaque "<block>-<idx>"
// strings the client round-trips verbatim. `total` isn't populated for
// this endpoint (row counts at 10⁷+ scale are too expensive), so the UI
// just shows Next/Prev without a page count.
//
// Server-side filters (`pallet`, `signed`) live in the URL query so the
// state is bookmarkable; changing either drops the cursor because a
// cursor computed under a different predicate doesn't line up with the
// new filtered set.

import type { Extrinsic } from '@bindings'

definePageMeta({ name: 'extrinsics-list' })
useSeoMeta({
  title: 'Extrinsics · Allfeat Explorer',
  description: 'Latest signed and unsigned extrinsics on the Allfeat chain.',
  ogTitle: 'Extrinsics · Allfeat',
  ogDescription: 'Browse the most recent transactions on Allfeat.',
  ogType: 'website',
})

const route = useRoute()

// Active filters, derived from the URL. `pallet` is an opaque string
// (validated against the dropdown options on change); `signed` is
// normalised to `"true"`, `"false"`, or `null` — anything else is
// treated as "no filter" so a stray query param doesn't freeze an
// empty list.
const pallet = computed<string>(() => {
  const q = route.query.pallet
  return typeof q === 'string' ? q : ''
})

// Defaults to `'true'` (signed) so the list opens on the useful
// default — signed user transactions — without drowning in inherents.
// Any other value maps back to `'true'` so a stray / missing param
// can't break the filter.
const signed = computed<'true' | 'false'>(() => {
  const q = route.query.signed
  return q === 'false' ? 'false' : 'true'
})

const { pallets: palletOptions } = usePallets()

const {
  items: extrinsics,
  pending,
  error,
  hasMore,
  canGoBack,
  goBack,
  goNext,
} = usePaginatedList<Extrinsic>({
  keyPrefix: 'extrinsics-list',
  path: '/extrinsics',
  pageSize: 25,
  filters: () => ({
    pallet: pallet.value || null,
    signed: signed.value || null,
  }),
})

async function onPalletChange(event: Event) {
  const next = (event.target as HTMLSelectElement).value
  const { cursor: _cursor, pallet: _pallet, ...rest } = route.query
  await navigateTo({
    query: { ...rest, ...(next ? { pallet: next } : {}) },
  })
}

async function setSigned(next: 'true' | 'false') {
  if (next === signed.value) return
  const { cursor: _cursor, signed: _signed, ...rest } = route.query
  // Omit the param for the 'true' default so the URL stays clean.
  await navigateTo({
    query: { ...rest, ...(next === 'false' ? { signed: 'false' } : {}) },
  })
}

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'Extrinsics' },
]

const skeletonCols = ['100px', '100px', '1fr', '200px', '180px', '120px', '100px', '100px']
</script>

<template>
  <section class="container" style="padding-bottom: 64px;">
    <Breadcrumb :items="breadcrumb" />

    <div class="page-title">
      <div>
        <h1>Extrinsics</h1>
      </div>
    </div>

    <div class="filter-bar" role="group" aria-label="Extrinsic filters">
      <label class="filter-field">
        <span class="filter-label">Pallet</span>
        <select
          class="filter-select"
          :value="pallet"
          @change="onPalletChange"
        >
          <option value="">All pallets</option>
          <option
            v-for="name in palletOptions"
            :key="name"
            :value="name"
          >
            {{ name }}
          </option>
        </select>
      </label>

      <div class="filter-field">
        <span class="filter-label">Signer</span>
        <div class="filter-toggle" role="radiogroup">
          <button
            type="button"
            role="radio"
            :aria-checked="signed === 'true'"
            :class="{ active: signed === 'true' }"
            @click="setSigned('true')"
          >
            Signed
          </button>
          <button
            type="button"
            role="radio"
            :aria-checked="signed === 'false'"
            :class="{ active: signed === 'false' }"
            @click="setSigned('false')"
          >
            Unsigned
          </button>
        </div>
      </div>
    </div>

    <div class="panel" style="margin-top: 20px;">
      <ExtrinsicsTable v-if="extrinsics.length > 0" :extrinsics="extrinsics" />
      <SkeletonRows v-else-if="pending" :rows="10" :columns="skeletonCols" />
      <p v-else-if="error" class="dim" style="padding: 40px; text-align: center;">
        Failed to load extrinsics.
      </p>
      <p v-else class="dim" style="padding: 40px; text-align: center;">
        No extrinsics to display.
      </p>

      <nav
        v-if="canGoBack || hasMore"
        class="cursor-nav"
        aria-label="Extrinsic pagination"
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
.filter-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: 16px;
  margin-top: 16px;
}
.filter-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.filter-label {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--ink-dim);
}
.filter-select {
  appearance: none;
  background: var(--panel-2, var(--panel));
  color: var(--text);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 6px 28px 6px 10px;
  font: inherit;
  font-size: 13px;
  cursor: pointer;
  min-width: 180px;
  background-image: linear-gradient(45deg, transparent 50%, var(--ink-dim) 50%),
                    linear-gradient(135deg, var(--ink-dim) 50%, transparent 50%);
  background-position: calc(100% - 14px) 50%, calc(100% - 10px) 50%;
  background-size: 4px 4px, 4px 4px;
  background-repeat: no-repeat;
}
.filter-select:focus-visible {
  outline: 2px solid var(--teal-500);
  outline-offset: 1px;
}

.filter-toggle {
  display: inline-flex;
  border: 1px solid var(--line);
  border-radius: 4px;
  overflow: hidden;
}
.filter-toggle button {
  appearance: none;
  background: var(--panel-2, var(--panel));
  color: var(--ink-dim);
  border: 0;
  border-left: 1px solid var(--line);
  padding: 6px 12px;
  font: inherit;
  font-size: 13px;
  cursor: pointer;
}
.filter-toggle button:first-child {
  border-left: 0;
}
.filter-toggle button:hover:not(.active) {
  color: var(--text);
}
.filter-toggle button.active {
  background: var(--ink);
  color: var(--bg);
}
.filter-toggle button:focus-visible {
  outline: 2px solid var(--teal-500);
  outline-offset: -2px;
}

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
