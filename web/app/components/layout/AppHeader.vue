<script setup lang="ts">
// Top navigation shell. Mirrors the mockup .header layout:
//   Brand | Nav (+ More dropdown) | spacer | NetworkSwitch
//
// Nav items are rendered twice — once in the primary `.nav` and once in
// the `.nav-more-menu`. CSS handles which items are visible at each
// breakpoint via `data-collapse-at` / `data-show-at` (ported verbatim
// from the proto). We don't measure widths in JS.
//
// On mobile (<900px) the entire `.nav` is hidden and replaced by a
// hamburger button that opens <MobileDrawer/> — the drawer gets the full
// nav list without any collapse logic, since it has room to show them
// all vertically.

import { computed, ref } from 'vue'
import { onClickOutside, useMediaQuery } from '@vueuse/core'
import type { NavItem } from '~/types/nav'

const ALL_NAV_ITEMS: readonly NavItem[] = [
  { id: 'overview', label: 'Overview', path: '/', collapseAt: null },
  { id: 'ats', label: 'ATS', path: '/ats', collapseAt: null },
  { id: 'token', label: 'Token', path: '/token', collapseAt: null, mainnetOnly: true },
  { id: 'blocks', label: 'Blocks', path: '/blocks', collapseAt: null },
  { id: 'extrinsics', label: 'Extrinsics', path: '/extrinsics', collapseAt: 'xxl' },
  { id: 'accounts', label: 'Accounts', path: '/accounts', collapseAt: 'xl' },
  { id: 'events', label: 'Events', path: '/events', collapseAt: 'xxl' },
  { id: 'runtime', label: 'Runtime', path: '/runtime', collapseAt: 'xxl' },
] as const

// Mainnet detection (`isMainnet`) also gates `mainnetOnly` entries. Defaults
// to `true` during the SSR window before the active network resolves, keeping
// the server and client markup stable.
const { isMainnet } = useActiveNetwork()

const NAV_ITEMS = computed(() =>
  ALL_NAV_ITEMS.filter(it => !it.mainnetOnly || isMainnet.value),
)

const drawerOpen = ref(false)
function toggleDrawer() {
  drawerOpen.value = !drawerOpen.value
}

// Items rendered inside the More dropdown — anything with a collapse rule.
const MORE_ITEMS = computed(() =>
  NAV_ITEMS.value.filter(it => it.collapseAt !== null),
)

const route = useRoute()

function isActive(path: string): boolean {
  if (path === '/') return route.path === '/'
  return route.path === path || route.path.startsWith(`${path}/`)
}

// Mirror the CSS breakpoints so we only treat an item as "in More" when it's
// actually hidden from the main nav at the current viewport.
const isXl = useMediaQuery('(max-width: 1280px)')

function isInMore(it: NavItem): boolean {
  if (it.collapseAt === 'xxl') return true
  if (it.collapseAt === 'xl') return isXl.value
  return false
}

const moreActive = computed(() =>
  MORE_ITEMS.value.some(it => isInMore(it) && isActive(it.path)),
)

const moreOpen = ref(false)
const moreWrap = ref<HTMLElement | null>(null)
onClickOutside(moreWrap, () => {
  moreOpen.value = false
})

function toggleMore(e: MouseEvent) {
  e.preventDefault()
  moreOpen.value = !moreOpen.value
}

function onNavClick(it: NavItem, e: MouseEvent) {
  if (it.disabled) {
    e.preventDefault()
    return
  }
  moreOpen.value = false
}
</script>

<template>
  <header class="header">
    <div class="container header-inner">
      <button
        type="button"
        class="nav-burger icon-btn"
        :aria-expanded="drawerOpen"
        aria-label="Open navigation"
        aria-controls="mobile-drawer"
        @click="toggleDrawer"
      >
        <svg
          width="20" height="20" viewBox="0 0 24 24"
          fill="none" stroke="currentColor" stroke-width="1.8"
          stroke-linecap="round" stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      </button>

      <NuxtLink to="/" class="brand" aria-label="Allfeat Explorer">
        <BrandLogo class="brand-mark" />
        <span class="brand-suffix">Explorer</span>
      </NuxtLink>

      <nav class="nav">
        <NuxtLink
          v-for="it in NAV_ITEMS"
          :key="it.id"
          :to="it.disabled ? '' : it.path"
          :data-collapse-at="it.collapseAt || undefined"
          :class="{ active: isActive(it.path) }"
          :style="it.disabled ? { opacity: 0.45, cursor: 'default' } : undefined"
          @click="onNavClick(it, $event)"
        >
          <NavIcon :name="it.id" />{{ it.label }}
        </NuxtLink>

        <div ref="moreWrap" class="nav-more-wrap" style="position: relative;">
          <a
            href="#"
            :class="{ active: moreActive }"
            :aria-expanded="moreOpen"
            aria-haspopup="menu"
            @click="toggleMore"
          >
            <NavIcon name="more" />More
            <svg
              width="10" height="10" viewBox="0 0 24 24"
              fill="none" stroke="currentColor" stroke-width="2"
              stroke-linecap="round" stroke-linejoin="round"
              :style="{ marginLeft: '-2px', opacity: 0.6, transform: moreOpen ? 'rotate(180deg)' : 'none', transition: 'transform 0.2s' }"
              aria-hidden="true"
            >
              <polyline points="6 9 12 15 18 9" />
            </svg>
          </a>
          <div v-if="moreOpen" class="nav-more-menu" role="menu">
            <NuxtLink
              v-for="it in MORE_ITEMS"
              :key="it.id"
              :to="it.disabled ? '' : it.path"
              :data-show-at="it.collapseAt || undefined"
              :class="{ active: isActive(it.path) }"
              :style="it.disabled ? { opacity: 0.45, cursor: 'default' } : undefined"
              @click="onNavClick(it, $event)"
            >
              <NavIcon :name="it.id" />{{ it.label }}
            </NuxtLink>
          </div>
        </div>
      </nav>

      <SearchBar />

      <ClientOnly>
        <ThemeSwitch />
      </ClientOnly>

      <NetworkSwitch class="header-network" />
    </div>

    <MobileDrawer
      :open="drawerOpen"
      :items="NAV_ITEMS"
      @close="drawerOpen = false"
    />
  </header>
</template>
