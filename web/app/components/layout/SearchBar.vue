<script setup lang="ts">
// Navbar search. Combines the typing-classifier composable
// (`useSearch`) with a headless combobox UI: text input + grouped
// dropdown + keyboard navigation + global ⌘K / Ctrl+K shortcut.
//
// Layout: the wrapper takes the flex slot between nav cluster and
// right-hand utilities inside `.header-inner`. The dropdown is
// absolute-positioned off the wrapper so it tracks the input's exact
// width when flex grows/shrinks on window resize.
//
// Selection model: results are grouped but keyboard nav runs against a
// flat array (`flat`), so ↑/↓ crosses group boundaries naturally. The
// active index is reset whenever a new query lands, and clamped when
// the result count shrinks so a stale index never points past the end.

import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { onClickOutside, onKeyStroke, useEventListener, useMediaQuery } from '@vueuse/core'
import type { SearchHit } from '~/composables/useSearch'
import type { SearchKind } from '~/utils/searchPatterns'

type IconKey = 'blocks' | 'extrinsics' | 'accounts' | 'ats'

interface Section {
  kind: SearchKind
  label: string
  icon: IconKey
  items: SearchHit[]
}

const { query, loading, results, flat, total, reset } = useSearch()
const { to: makeTo } = useNetworkLink()

const root = ref<HTMLElement | null>(null)
const input = ref<HTMLInputElement | null>(null)
const open = ref(false)
const activeIdx = ref(0)
const isMac = ref(false)

onMounted(() => {
  isMac.value = typeof navigator !== 'undefined'
    && /Mac|iPhone|iPad|iPod/i.test(navigator.platform || '')
})
const shortcutHint = computed(() => isMac.value ? '⌘K' : 'Ctrl K')

// Placeholder collapses below 640px — the long help string doesn't fit
// in a mobile header and mostly just shows the first few words truncated.
const isNarrow = useMediaQuery('(max-width: 640px)')
const placeholder = computed(() =>
  isNarrow.value
    ? 'Search…'
    : 'Search blocks, extrinsics, accounts (name or SS58), ATS…',
)

function isTypingElsewhere(): boolean {
  const el = document.activeElement as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || el.isContentEditable
}

useEventListener('keydown', (e: KeyboardEvent) => {
  if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'k') {
    e.preventDefault()
    input.value?.focus()
    input.value?.select()
    open.value = true
    return
  }
  if (e.key === '/' && !isTypingElsewhere()) {
    e.preventDefault()
    input.value?.focus()
    open.value = true
  }
})

onClickOutside(root, () => {
  open.value = false
})

onKeyStroke('Escape', () => {
  if (!open.value && query.value === '') return
  open.value = false
  input.value?.blur()
})

watch(total, (n) => {
  if (activeIdx.value >= n) activeIdx.value = Math.max(0, n - 1)
})

watch(query, () => {
  open.value = query.value.trim().length > 0
  activeIdx.value = 0
})

const sections = computed<Section[]>(() => {
  const r = results.value
  const out: Section[] = []
  if (r.blocks.length) out.push({ kind: 'block-number', label: 'Blocks', icon: 'blocks', items: r.blocks })
  if (r.extrinsics.length) out.push({ kind: 'extrinsic-id', label: 'Extrinsics', icon: 'extrinsics', items: r.extrinsics })
  if (r.accounts.length) out.push({ kind: 'account', label: 'Accounts', icon: 'accounts', items: r.accounts })
  if (r.ats.length) out.push({ kind: 'ats-id', label: 'ATS', icon: 'ats', items: r.ats })
  return out
})

function globalIndex(section: number, item: number): number {
  let idx = 0
  for (let s = 0; s < section; s++) idx += sections.value[s]!.items.length
  return idx + item
}

function onFocus() {
  if (query.value.trim()) open.value = true
}

function move(delta: number) {
  if (flat.value.length === 0) return
  const n = flat.value.length
  activeIdx.value = (activeIdx.value + delta + n) % n
  nextTick(() => {
    const el = root.value?.querySelector<HTMLElement>('[data-active="true"]')
    el?.scrollIntoView({ block: 'nearest' })
  })
}

function onEnter() {
  const hit = flat.value[activeIdx.value]
  if (hit) choose(hit)
}

function choose(hit: SearchHit) {
  open.value = false
  input.value?.blur()
  const target = makeTo(hit.to)
  reset()
  navigateTo(target)
}
</script>

<template>
  <div ref="root" class="search-wrap">
    <div class="search-input-wrap" :class="{ 'has-value': query.length > 0, 'is-open': open }">
      <svg
        class="search-ico"
        width="15" height="15" viewBox="0 0 24 24"
        fill="none" stroke="currentColor" stroke-width="1.6"
        stroke-linecap="round" stroke-linejoin="round"
        aria-hidden="true"
      >
        <circle cx="11" cy="11" r="7" />
        <path d="m21 21-4.3-4.3" />
      </svg>
      <input
        ref="input"
        v-model="query"
        type="search"
        class="search-input"
        :placeholder="placeholder"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
        role="combobox"
        aria-autocomplete="list"
        aria-label="Search"
        :aria-expanded="open"
        aria-controls="search-listbox"
        :aria-activedescendant="open && flat.length > 0 ? `search-item-${activeIdx}` : undefined"
        @focus="onFocus"
        @keydown.down.prevent="move(1); open = true"
        @keydown.up.prevent="move(-1); open = true"
        @keydown.enter.prevent="onEnter"
        @keydown.tab="open = false"
      >
      <button
        v-if="query"
        type="button"
        class="search-clear"
        aria-label="Clear search"
        tabindex="-1"
        @click="reset(); input?.focus()"
      >
        <svg
          width="14" height="14" viewBox="0 0 24 24"
          fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="m6 6 12 12M6 18 18 6" />
        </svg>
      </button>
      <span v-else class="search-kbd" aria-hidden="true">{{ shortcutHint }}</span>
    </div>

    <Transition name="search-menu">
      <div
        v-if="open && query.trim()"
        id="search-listbox"
        class="search-menu"
        role="listbox"
      >
        <div v-if="loading && total === 0" class="search-state">
          <span class="search-spinner" />
          <span>Searching…</span>
        </div>

        <div v-else-if="total === 0" class="search-state search-empty">
          <span class="search-empty-title">No matches</span>
          <span class="search-empty-sub">Try a block number, account name or SS58, extrinsic id (e.g. <span class="mono">1234-0</span>), 0x-hash, or ATS id.</span>
        </div>

        <template v-else>
          <div
            v-for="(section, sIdx) in sections"
            :key="section.kind"
            class="search-section"
          >
            <div class="search-section-label">
              <NavIcon :name="section.icon" :size="12" />
              {{ section.label }}
            </div>
            <button
              v-for="(hit, iIdx) in section.items"
              :id="`search-item-${globalIndex(sIdx, iIdx)}`"
              :key="hit.to"
              type="button"
              class="search-item"
              role="option"
              :data-active="activeIdx === globalIndex(sIdx, iIdx)"
              :aria-selected="activeIdx === globalIndex(sIdx, iIdx)"
              @mouseenter="activeIdx = globalIndex(sIdx, iIdx)"
              @click="choose(hit)"
            >
              <span class="search-item-main mono">{{ hit.title }}</span>
              <span class="search-item-sub">{{ hit.subtitle }}</span>
              <svg
                class="search-item-arrow"
                width="14" height="14" viewBox="0 0 24 24"
                fill="none" stroke="currentColor" stroke-width="1.8"
                stroke-linecap="round" stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M7 17 17 7" />
                <path d="M8 7h9v9" />
              </svg>
            </button>
          </div>
          <div v-if="loading" class="search-state search-refreshing">
            <span class="search-spinner" />
            <span>Refining…</span>
          </div>
        </template>
      </div>
    </Transition>
  </div>
</template>
