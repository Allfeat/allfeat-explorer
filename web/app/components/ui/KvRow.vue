<script setup lang="ts">
// Single row inside <Kv/>. Multi-root SFC emits the two grid cells as
// siblings so the parent's CSS grid flows them correctly.
//   <KvRow label="Block">#12345</KvRow>
// For complex labels (badges, tooltips) use the `label` slot instead.

defineProps<{
  label?: string
}>()
</script>

<template>
  <div class="k">
    <slot name="label">{{ label }}</slot>
  </div>
  <div class="v">
    <slot />
  </div>
</template>
