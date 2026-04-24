<script setup lang="ts">
// Status pill variants (success / fail / pending / neutral). The `dot`
// marker is on by default because every pill in the maquette carries it;
// turn it off for compact contexts where the glyph adds too much noise.

type PillVariant = 'success' | 'fail' | 'pending' | 'neutral'

withDefaults(defineProps<{
  variant?: PillVariant
  dot?: boolean
}>(), {
  variant: 'neutral',
  dot: true,
})
</script>

<template>
  <span class="pill" :class="variant">
    <span v-if="dot" class="dot" />
    <slot />
  </span>
</template>
