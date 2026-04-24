<script setup lang="ts">
// Mini 3-bar waveform used as the "live" indicator across the app.
// Purely presentational; colour is driven by `currentColor` on the
// parent so the same markup reads teal/neutral by context.

withDefaults(defineProps<{
  size?: number
}>(), {
  size: 10,
})
</script>

<template>
  <span
    class="live-dot"
    :style="{ '--live-dot-h': `${size}px` }"
    aria-hidden="true"
  >
    <i /><i /><i />
  </span>
</template>
