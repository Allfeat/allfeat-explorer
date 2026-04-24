<script setup lang="ts">
// Small mono-labelled tag. Default = bare chip; `module` = teal-tinted
// variant used for Substrate pallet names; `call` = brighter text variant
// used for call names next to their module.

type ChipVariant = 'default' | 'module' | 'call'

withDefaults(defineProps<{
  variant?: ChipVariant
}>(), {
  variant: 'default',
})
</script>

<template>
  <span class="chip" :class="variant !== 'default' ? variant : null">
    <slot />
  </span>
</template>
