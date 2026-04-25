<script setup lang="ts">
// Per-address waveform fingerprint — see utils/identicon.ts for the
// hashing + parameter derivation. `large` applies the .identicon-lg
// modifier (56px); arbitrary sizes use the `size` prop which emits
// inline width/height overrides. The disc carries a coloured glow via
// the `--identicon-color` custom property the script writes.

const props = withDefaults(defineProps<{
  seed: string
  size?: number | null
  large?: boolean
}>(), {
  size: null,
  large: false,
})

const VIEW_BOX = 100
const MARGIN = 10

const params = computed(() => identiconParams(props.seed))
const color = computed(() => identiconColor(params.value))
const path = computed(() => identiconPath(params.value, VIEW_BOX, MARGIN))

const inlineStyle = computed<Record<string, string>>(() => {
  const style: Record<string, string> = { '--identicon-color': color.value }
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
  >
    <svg
      :viewBox="`0 0 ${VIEW_BOX} ${VIEW_BOX}`"
      class="identicon-svg"
      preserveAspectRatio="xMidYMid meet"
    >
      <path :d="path" class="identicon-glow" :stroke="color" />
      <path :d="path" class="identicon-wave" :stroke="color" />
    </svg>
  </span>
</template>
