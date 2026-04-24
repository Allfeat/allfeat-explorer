<script setup lang="ts">
// Flat stroke-style icon per envelope id — gives each genesis pocket a
// visual handle so users can scan the grid without reading every label.
// Envelope ids are stable (see `EnvelopeId` union in @bindings); any new
// envelope falls through to the generic token glyph.

import type { EnvelopeId } from '@bindings'

withDefaults(defineProps<{
  id: EnvelopeId
  size?: number
}>(), {
  size: 22,
})
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.6"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    class="env-icon"
  >
    <!-- Teams: stylised group / handshake -->
    <template v-if="id === 'teams'">
      <circle cx="8" cy="9" r="2.5" />
      <circle cx="16" cy="9" r="2.5" />
      <path d="M3 19c.8-2.6 2.8-4 5-4s4.2 1.4 5 4" />
      <path d="M13 19c.8-2.6 2.8-4 5-4" />
    </template>

    <!-- KOL: megaphone -->
    <template v-else-if="id === 'kol'">
      <path d="M4 10v4a1 1 0 0 0 1 1h3l6 4V5L8 9H5a1 1 0 0 0-1 1Z" />
      <path d="M17 9a4 4 0 0 1 0 6" />
    </template>

    <!-- Private sales: vault / lock -->
    <template v-else-if="id === 'private1' || id === 'private2'">
      <rect x="4" y="5" width="16" height="14" rx="1.5" />
      <circle cx="12" cy="12" r="2.5" />
      <path d="M12 14.5V17" />
      <path d="M7 9v-.5M7 15v.5" />
    </template>

    <!-- Public sales: storefront / market -->
    <template v-else-if="id === 'public1' || id === 'public2' || id === 'public3' || id === 'public4'">
      <path d="M3 9h18l-1.5-4h-15Z" />
      <path d="M5 9v10h14V9" />
      <path d="M10 19v-5h4v5" />
    </template>

    <!-- Airdrop: parachute / gift drop -->
    <template v-else-if="id === 'airdrop'">
      <path d="M3 10a9 9 0 0 1 18 0" />
      <path d="M3 10 12 20 21 10" />
      <path d="M9 10v10M15 10v10M12 10v10" />
    </template>

    <!-- Community rewards: heart -->
    <template v-else-if="id === 'community-rewards'">
      <path d="M12 20s-7-4.5-7-10a4.5 4.5 0 0 1 7-3.5A4.5 4.5 0 0 1 19 10c0 5.5-7 10-7 10Z" />
    </template>

    <!-- Listing: chart / upward trend -->
    <template v-else-if="id === 'listing'">
      <path d="M3 17 9 11l4 4 8-9" />
      <path d="M14 6h7v7" />
    </template>

    <!-- R&D: beaker / flask -->
    <template v-else-if="id === 'research-development'">
      <path d="M9 4h6" />
      <path d="M10 4v6l-5 8a2 2 0 0 0 1.8 3h10.4A2 2 0 0 0 19 18l-5-8V4" />
      <path d="M7 15h10" />
    </template>

    <!-- Reserve: shield -->
    <template v-else-if="id === 'reserve'">
      <path d="M12 3 4 6v6c0 4.4 3.4 8.2 8 9 4.6-.8 8-4.6 8-9V6Z" />
    </template>

    <!-- Fallback: abstract token -->
    <template v-else>
      <circle cx="12" cy="12" r="8" />
      <path d="M12 8v8M8 12h8" />
    </template>
  </svg>
</template>

<style scoped>
.env-icon {
  color: var(--teal-500);
  flex-shrink: 0;
}
</style>
