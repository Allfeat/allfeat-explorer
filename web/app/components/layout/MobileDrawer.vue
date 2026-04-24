<script setup lang="ts">
// Mobile navigation drawer — slides in from the right at <900px.
// Contains the full nav list (ALL items, no More dropdown needed here),
// a NetworkSwitch, and a ThemeSwitch. The parent (AppHeader) owns the
// open state and toggles it from the hamburger button.

import { computed, nextTick, ref, watch } from 'vue'
import { onKeyStroke, useScrollLock } from '@vueuse/core'
import type { NavItem } from '~/types/nav'

const props = defineProps<{
  open: boolean
  items: readonly NavItem[]
}>()

const emit = defineEmits<{
  close: []
}>()

const route = useRoute()
const panel = ref<HTMLElement | null>(null)

// Lock body scroll while the drawer is visible so the user isn't
// accidentally scrolling the page behind the backdrop.
const locked = useScrollLock(typeof document !== 'undefined' ? document.body : null)
watch(() => props.open, (open) => {
  locked.value = open
  // Focus the panel after it opens so Escape / Tab have a starting point
  // inside the drawer. Small nextTick lets the transition mount finish.
  if (open) nextTick(() => panel.value?.focus())
})

// Close on route change — the user tapped a nav item, no need to also
// click the backdrop.
watch(() => route.fullPath, () => {
  if (props.open) emit('close')
})

onKeyStroke('Escape', () => {
  if (props.open) emit('close')
})

function isActive(path: string): boolean {
  if (path === '/') return route.path === '/'
  return route.path === path || route.path.startsWith(`${path}/`)
}

function onItemClick(it: NavItem, e: MouseEvent) {
  if (it.disabled) {
    e.preventDefault()
    return
  }
  // Route-change watch handles the close, but this keeps the drawer
  // feeling responsive in the edge case where the target route equals
  // the current one (no fullPath change fires).
  emit('close')
}

const YEAR = computed(() => new Date().getFullYear())
</script>

<template>
  <Teleport to="body">
    <Transition name="drawer">
      <div v-if="open" class="drawer-root" role="dialog" aria-modal="true" aria-label="Navigation">
        <button
          type="button"
          class="drawer-backdrop"
          aria-label="Close navigation"
          @click="emit('close')"
        />

        <aside
          ref="panel"
          class="drawer-panel"
          tabindex="-1"
        >
          <div class="drawer-head">
            <NuxtLink to="/" class="drawer-brand" @click="emit('close')">
              <BrandLogo class="drawer-brand-mark" />
              <span class="drawer-brand-suffix">Explorer</span>
            </NuxtLink>
            <button
              type="button"
              class="drawer-close icon-btn"
              aria-label="Close navigation"
              @click="emit('close')"
            >
              <svg
                width="18" height="18" viewBox="0 0 24 24"
                fill="none" stroke="currentColor" stroke-width="2"
                stroke-linecap="round" stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="m6 6 12 12M6 18 18 6" />
              </svg>
            </button>
          </div>

          <nav class="drawer-nav" aria-label="Primary">
            <NuxtLink
              v-for="it in items"
              :key="it.id"
              :to="it.disabled ? '' : it.path"
              :class="{ 'drawer-nav-link': true, active: isActive(it.path), disabled: it.disabled }"
              @click="onItemClick(it, $event)"
            >
              <NavIcon :name="it.id" />
              <span>{{ it.label }}</span>
              <span v-if="it.disabled" class="drawer-nav-badge">soon</span>
              <svg
                v-else
                class="drawer-nav-arrow"
                width="14" height="14" viewBox="0 0 24 24"
                fill="none" stroke="currentColor" stroke-width="1.8"
                stroke-linecap="round" stroke-linejoin="round"
                aria-hidden="true"
              >
                <polyline points="9 18 15 12 9 6" />
              </svg>
            </NuxtLink>
          </nav>

          <div class="drawer-divider" />

          <div class="drawer-tools">
            <div class="drawer-tool-label">Network</div>
            <NetworkSwitch class="drawer-network" />

            <div class="drawer-tool-label">Appearance</div>
            <ClientOnly>
              <div class="drawer-theme-row">
                <ThemeSwitch />
                <span class="drawer-tool-hint">Toggle dark / light</span>
              </div>
            </ClientOnly>
          </div>

          <div class="drawer-foot">
            <span class="mono">© {{ YEAR }} ALLFEAT</span>
          </div>
        </aside>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped lang="scss">
.drawer-root {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  justify-content: flex-end;
  pointer-events: none;

  > * { pointer-events: auto; }
}

.drawer-backdrop {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(3px);
  -webkit-backdrop-filter: blur(3px);
  border: 0;
  padding: 0;
  cursor: pointer;
}

[data-theme="light"] .drawer-backdrop {
  background: rgba(21, 21, 21, 0.35);
}

.drawer-panel {
  position: relative;
  width: min(340px, 88vw);
  max-width: 100%;
  height: 100%;
  background: var(--bg-elev-solid);
  border-left: 1px solid var(--line-2);
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  overscroll-behavior: contain;

  &:focus {
    outline: none;
  }
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 18px 20px;
  border-bottom: 1px solid var(--line);
  position: sticky;
  top: 0;
  background: var(--bg-elev-solid);
  z-index: 1;
}

.drawer-brand {
  display: flex;
  align-items: center;
  gap: 10px;
  text-decoration: none;
  color: inherit;
}

.drawer-brand-mark {
  height: 20px;
  width: auto;
  color: var(--ink);
  display: block;
}

.drawer-brand-suffix {
  padding-left: 10px;
  border-left: 1px solid var(--line);
  font-family: var(--font-mono);
  font-size: 10.5px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--ink-dimmer);
}

.drawer-close {
  width: 40px;
  height: 40px;
}

.drawer-nav {
  display: flex;
  flex-direction: column;
  padding: 8px 10px;
  gap: 2px;
}

.drawer-nav-link {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 12px;
  border-radius: 8px;
  font-family: var(--font-display);
  font-size: 14.5px;
  font-weight: 500;
  color: var(--ink-dim);
  text-decoration: none;
  min-height: 48px;
  transition: background 0.18s ease, color 0.18s ease;

  svg {
    flex-shrink: 0;
    opacity: 0.7;
  }

  &:hover {
    background: var(--hover);
    color: var(--ink);
  }

  &.active {
    color: var(--ink);
    background: var(--hover);
  }

  &.active svg {
    opacity: 1;
    color: var(--teal-500);
  }

  &.disabled {
    opacity: 0.45;
    cursor: default;
  }

  > span:first-of-type {
    flex: 1;
  }
}

.drawer-nav-arrow {
  color: var(--ink-dimmer);
  opacity: 0.5;
}

.drawer-nav-link:hover .drawer-nav-arrow,
.drawer-nav-link.active .drawer-nav-arrow {
  opacity: 0.9;
  color: var(--ink-dim);
}

.drawer-nav-badge {
  padding: 2px 7px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 9.5px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--ink-dimmer);
  background: var(--chip-bg);
  border: 1px solid var(--chip-bd);
}

.drawer-divider {
  height: 1px;
  background: var(--line);
  margin: 6px 18px;
}

.drawer-tools {
  padding: 10px 18px 18px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.drawer-tool-label {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-dimmer);
  margin-top: 6px;
}

// NetworkSwitch trigger already gets its own styling; here we just want
// it to stretch full-width so it reads as a drawer row, not a chip.
.drawer-network :deep(.network-switch) {
  width: 100%;
  justify-content: flex-start;
  height: 44px;
  padding: 8px 12px;
}

.drawer-network :deep(.network-switch svg:last-of-type) {
  margin-left: auto;
}

.drawer-network :deep(.network-menu) {
  // Keep the dropdown from overflowing the drawer on narrow screens.
  left: 0;
  right: 0;
  min-width: 0;
}

.drawer-theme-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.drawer-tool-hint {
  font-size: 12.5px;
  color: var(--ink-dim);
}

.drawer-foot {
  margin-top: auto;
  padding: 14px 20px 20px;
  border-top: 1px solid var(--line);
  font-size: 10.5px;
  letter-spacing: 0.06em;
  color: var(--ink-muted);
}

// ——— transitions ———

.drawer-enter-active .drawer-panel,
.drawer-leave-active .drawer-panel {
  transition: transform 0.28s cubic-bezier(0.2, 0.8, 0.2, 1);
}
.drawer-enter-active .drawer-backdrop,
.drawer-leave-active .drawer-backdrop {
  transition: opacity 0.22s ease;
}

.drawer-enter-from .drawer-panel,
.drawer-leave-to .drawer-panel {
  transform: translateX(100%);
}
.drawer-enter-from .drawer-backdrop,
.drawer-leave-to .drawer-backdrop {
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .drawer-enter-active .drawer-panel,
  .drawer-leave-active .drawer-panel {
    transition: none;
  }
  .drawer-enter-active .drawer-backdrop,
  .drawer-leave-active .drawer-backdrop {
    transition: opacity 0.1s linear;
  }
}
</style>
