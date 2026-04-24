<script setup lang="ts">
// Trail above page titles. Items render as NuxtLinks when `to` is
// provided, otherwise as plain text (typically the current page's
// label). Separators are inserted between items but not after the last.

interface BreadcrumbItem {
  label: string
  to?: string | null
}

defineProps<{
  items: readonly BreadcrumbItem[]
}>()
</script>

<template>
  <div class="breadcrumb">
    <template v-for="(it, i) in items" :key="i">
      <NuxtLink v-if="it.to" :to="it.to">{{ it.label }}</NuxtLink>
      <span v-else>{{ it.label }}</span>
      <span v-if="i < items.length - 1" class="sep">/</span>
    </template>
  </div>
</template>
