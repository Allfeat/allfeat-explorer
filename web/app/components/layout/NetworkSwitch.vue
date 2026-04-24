<script setup lang="ts">
// Network selector. Bound to `useNetworksStore` + `useActiveNetwork`.
// Clicking an item pushes ?network=<id> via `navigateTo({ replace: true })`
// so back/forward don't thread through every network swap. Other query
// params (tab, filter, page) are preserved across the swap.
//
// Markup mirrors the mockup one-for-one: `.network-switch` trigger with
// `.network-dot` (+ `.testnet` modifier), `.network-name`, `.network-type`.
// The dropdown is a scoped companion — it doesn't reuse global classes
// since no equivalent exists in the mockup proto outside inline styles.

import { ref } from 'vue'
import { onClickOutside } from '@vueuse/core'
import { useNetworksStore } from '~/stores/networks'
import { useActiveNetwork } from '~/composables/useActiveNetwork'

const store = useNetworksStore()
const { id: activeId, spec: activeSpec } = useActiveNetwork()

const open = ref(false)
const root = ref<HTMLElement | null>(null)

onClickOutside(root, () => {
  open.value = false
})

const route = useRoute()

function select(id: string) {
  open.value = false
  if (id === activeId.value) return
  const query = { ...route.query, network: id }
  navigateTo({ path: route.path, query }, { replace: true })
}

function toggle() {
  open.value = !open.value
}
</script>

<template>
  <div ref="root" class="network-switch-wrap">
    <button
      type="button"
      class="network-switch"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="toggle"
    >
      <span
        class="network-dot"
        :class="{ testnet: activeSpec?.testnet }"
      />
      <span class="network-name">{{ activeSpec?.name ?? '—' }}</span>
      <span class="network-type">{{ activeSpec?.kind ?? '' }}</span>
      <svg
        width="12" height="12" viewBox="0 0 24 24"
        fill="none" stroke="currentColor" stroke-width="2"
        stroke-linecap="round" stroke-linejoin="round"
        aria-hidden="true"
      >
        <polyline points="6 9 12 15 18 9" />
      </svg>
    </button>

    <Transition name="net-menu">
      <div v-if="open" class="network-menu" role="listbox">
        <button
          v-for="n in store.items"
          :key="n.id"
          type="button"
          class="network-menu__item"
          :class="{ active: n.id === activeId }"
          role="option"
          :aria-selected="n.id === activeId"
          @click="select(n.id)"
        >
          <span class="network-dot" :class="{ testnet: n.testnet }" />
          <span class="network-menu__text">
            <span class="network-menu__name">{{ n.name }}</span>
            <span class="ui-label network-menu__sub">{{ n.kind }} · {{ n.token }}</span>
          </span>
          <svg
            v-if="n.id === activeId"
            class="network-menu__check"
            width="14" height="14" viewBox="0 0 24 24"
            fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round"
            aria-hidden="true"
          >
            <polyline points="20 6 9 17 4 12" />
          </svg>
        </button>

        <div v-if="activeSpec" class="network-menu__footer">
          <div class="ui-label">Endpoint</div>
          <div class="mono network-menu__endpoint">{{ activeSpec.endpoint }}</div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped lang="scss">
.network-switch-wrap {
  position: relative;
}

.network-switch {
  svg {
    transition: transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  &[aria-expanded="true"] svg {
    transform: rotate(180deg);
  }
}

.network-menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  min-width: 240px;
  padding: 6px;
  background: var(--bg-elev);
  border: 1px solid var(--line-2);
  border-radius: 10px;
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.4);
  z-index: 60;
  display: flex;
  flex-direction: column;
  gap: 2px;
  transform-origin: top right;

  &__item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 10px 12px;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    cursor: pointer;
    color: var(--ink-dim);
    transition: background 0.18s ease, color 0.18s ease;

    &:hover {
      background: var(--hover);
      color: var(--ink);
    }

    &.active {
      background: var(--hover);
      color: var(--ink);
    }
  }

  &__text {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
  }

  &__name {
    font-family: var(--font-display);
    font-size: 13px;
    font-weight: 600;
  }

  &__sub {
    font-size: 10px;
  }

  &__check {
    margin-left: auto;
    color: var(--teal-500);
  }

  &__footer {
    padding: 8px 12px;
    border-top: 1px solid var(--line);
    margin-top: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  &__endpoint {
    font-size: 11px;
    color: var(--ink-dimmer);
    word-break: break-all;
  }
}

[data-theme="light"] .network-menu__check {
  color: var(--teal-700);
}

.net-menu-enter-active,
.net-menu-leave-active {
  transition:
    opacity 0.2s cubic-bezier(0.2, 0.8, 0.2, 1),
    transform 0.22s cubic-bezier(0.2, 0.8, 0.2, 1);
}

.net-menu-enter-from,
.net-menu-leave-to {
  opacity: 0;
  transform: translateY(-6px) scale(0.98);
}

@media (prefers-reduced-motion: reduce) {
  .net-menu-enter-active,
  .net-menu-leave-active {
    transition: opacity 0.15s linear;
  }
  .net-menu-enter-from,
  .net-menu-leave-to {
    transform: none;
  }
  .network-switch svg {
    transition: none;
  }
}
</style>
