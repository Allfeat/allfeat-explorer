<script setup lang="ts">
// Deterministic circular swatch derived from the seed (usually an SS58
// address). `large` applies the .identicon-lg modifier (56px); arbitrary
// sizes use the `size` prop which emits inline width/height overrides.

const props = withDefaults(defineProps<{
  seed: string
  size?: number | null
  large?: boolean
}>(), {
  size: null,
  large: false,
})

const gradient = computed(() => identiconGradient(props.seed))

const inlineStyle = computed(() => {
  const style: Record<string, string> = { background: gradient.value }
  if (props.size != null) {
    style.width = `${props.size}px`
    style.height = `${props.size}px`
  }
  return style
})
</script>

<template>
  <span
    class="identicon"
    :class="{ 'identicon-lg': large }"
    :style="inlineStyle"
    aria-hidden="true"
  />
</template>
