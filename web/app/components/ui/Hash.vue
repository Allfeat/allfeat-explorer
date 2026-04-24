<script setup lang="ts">
// Monospace hash display: head…tail with an optional copy button and an
// optional link wrapper. `dim` swaps to the muted color used for less
// significant hashes (e.g. parent hash in block detail views).

const props = withDefaults(defineProps<{
  text: string
  to?: string | null
  head?: number
  tail?: number
  copy?: boolean
  dim?: boolean
}>(), {
  to: null,
  head: 6,
  tail: 4,
  copy: true,
  dim: false,
})

const short = computed(() => shortHash(props.text, props.head, props.tail))
const hashClass = computed(() => (props.dim ? ['hash', 'hash-dim'] : ['hash']))
</script>

<template>
  <span class="hash-wrap">
    <NuxtLink v-if="to" :to="to" :class="hashClass">{{ short }}</NuxtLink>
    <span v-else :class="hashClass">{{ short }}</span>
    <CopyButton v-if="copy" :text="text" />
  </span>
</template>

<style scoped>
.hash-wrap {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
</style>
