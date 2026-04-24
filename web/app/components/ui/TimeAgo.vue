<script setup lang="ts">
// Relative-time label that re-renders every second. Fed by useWallClock
// (singleton 1-Hz tick) so dozens of TimeAgo instances on a page share
// one timer. The wrapper is a bare <span> so callers can apply their
// own typography classes (usually `mono text-xs dim`).

const props = defineProps<{
  timestamp: number
}>()

const now = useWallClock()
const label = computed(() => timeAgo(props.timestamp, now.value))
</script>

<template>
  <span>{{ label }}</span>
</template>
