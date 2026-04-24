<script setup lang="ts">
// Nav icon set — stroke-based, 15×15 default. Paths ported verbatim from
// the mockup's NavIcon component so the visual language matches exactly.

interface Props {
  name: 'overview' | 'ats' | 'token' | 'blocks' | 'extrinsics' | 'accounts' | 'events' | 'runtime' | 'more'
  size?: number
}

const { size = 15 } = defineProps<Props>()
</script>

<template>
  <svg
    :width="size" :height="size" viewBox="0 0 24 24"
    fill="none" stroke="currentColor" stroke-width="1.5"
    stroke-linecap="round" stroke-linejoin="round"
    aria-hidden="true"
  >
    <template v-if="name === 'overview'">
      <rect x="3.5" y="3.5" width="7" height="7" rx="1.2" />
      <rect x="13.5" y="3.5" width="7" height="7" rx="1.2" />
      <rect x="3.5" y="13.5" width="7" height="7" rx="1.2" />
      <rect x="13.5" y="13.5" width="7" height="7" rx="1.2" />
    </template>
    <template v-else-if="name === 'ats'">
      <rect x="4.5" y="10.5" width="15" height="10" rx="2" />
      <path d="M7.5 10.5V7a4.5 4.5 0 0 1 9 0v3.5" />
      <circle cx="12" cy="15" r="1.1" fill="currentColor" stroke="none" />
    </template>
    <template v-else-if="name === 'blocks'">
      <path d="M12 3 3.5 7.5 12 12l8.5-4.5L12 3z" />
      <path d="M3.5 7.5V16L12 20.5V12" />
      <path d="M20.5 7.5V16L12 20.5" />
    </template>
    <template v-else-if="name === 'token'">
      <circle cx="12" cy="12" r="8" />
      <path d="M12 7.5v9" />
      <path d="M9.5 9.5h3.5a1.5 1.5 0 0 1 0 3H10a1.5 1.5 0 0 0 0 3h3.5" />
    </template>
    <template v-else-if="name === 'extrinsics'">
      <path d="M7.5 4v14" />
      <path d="M4.5 7.5 7.5 4.5l3 3" />
      <path d="M16.5 20V6" />
      <path d="M13.5 16.5 16.5 19.5l3-3" />
    </template>
    <template v-else-if="name === 'accounts'">
      <circle cx="12" cy="8" r="3.5" />
      <path d="M5 20c0-3.6 3.1-6 7-6s7 2.4 7 6" />
    </template>
    <template v-else-if="name === 'events'">
      <path d="M6.5 16.5V11a5.5 5.5 0 0 1 11 0v5.5" />
      <path d="M4.5 16.5h15" />
      <path d="M10 19.5a2 2 0 0 0 4 0" />
    </template>
    <template v-else-if="name === 'runtime'">
      <rect x="6" y="6" width="12" height="12" rx="1.5" />
      <rect x="9" y="9" width="6" height="6" rx="0.5" />
      <path d="M10 3v3M14 3v3M10 18v3M14 18v3M3 10h3M3 14h3M18 10h3M18 14h3" />
    </template>
    <template v-else-if="name === 'more'">
      <circle cx="5" cy="12" r="1.3" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1.3" fill="currentColor" stroke="none" />
      <circle cx="19" cy="12" r="1.3" fill="currentColor" stroke="none" />
    </template>
  </svg>
</template>
