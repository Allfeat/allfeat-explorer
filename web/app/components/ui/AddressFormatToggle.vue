<script setup lang="ts">
// Inline ↔ button that flips an `'mainnet' | 'generic'` v-model. Used on
// the account-detail title row so a viewer can re-encode the displayed
// SS58 between the network's native prefix and the generic Substrate
// prefix (42) without leaving the page.

export type AddressFormat = 'mainnet' | 'generic'

const model = defineModel<AddressFormat>({ required: true })
const props = withDefaults(defineProps<{ size?: number }>(), { size: 13 })

const tooltip = computed(() =>
  model.value === 'mainnet'
    ? 'Switch to Generic format (prefix 42)'
    : 'Switch to network format',
)

function toggle(event: MouseEvent) {
  event.stopPropagation()
  model.value = model.value === 'mainnet' ? 'generic' : 'mainnet'
}
</script>

<template>
  <button
    type="button"
    class="fmt-btn"
    :title="tooltip"
    :aria-label="tooltip"
    @click="toggle"
  >
    <svg
      :width="props.size"
      :height="props.size"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <polyline points="17 1 21 5 17 9" />
      <path d="M3 11V9a4 4 0 0 1 4-4h14" />
      <polyline points="7 23 3 19 7 15" />
      <path d="M21 13v2a4 4 0 0 1-4 4H3" />
    </svg>
  </button>
</template>
