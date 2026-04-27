<script setup lang="ts">
// Trail above page titles. Items render as NuxtLinks when `to` is
// provided, otherwise as plain text (typically the current page's
// label). Separators are inserted between items but not after the last.
//
// Link targets are routed through `useNetworkLink().to()` so the active
// `?network=` query is preserved across breadcrumb hops — pages pass raw
// paths and don't need to thread the network query themselves.

interface BreadcrumbItem {
  label: string
  to?: string | null
}

defineProps<{
  items: readonly BreadcrumbItem[]
}>()

const { to: linkTo } = useNetworkLink()
</script>

<template>
  <div class="breadcrumb">
    <template v-for="(it, i) in items" :key="i">
      <NuxtLink v-if="it.to" :to="linkTo(it.to)">{{ it.label }}</NuxtLink>
      <span v-else>{{ it.label }}</span>
      <span v-if="i < items.length - 1" class="sep">/</span>
    </template>
  </div>
</template>
