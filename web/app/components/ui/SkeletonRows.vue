<script setup lang="ts">
// Placeholder rows shown while a table is fetching. `columns` controls
// how many shimmer cells each row contains — pass a number for equal
// widths, or a list of grid-template-columns tracks (e.g. `['30%',
// '1fr', '80px']`) to match a real table's layout so hydration doesn't
// shift content.

const props = withDefaults(defineProps<{
  rows?: number
  columns?: number | readonly string[]
}>(), {
  rows: 5,
  columns: 4,
})

const columnCount = computed<number>(() =>
  typeof props.columns === 'number' ? props.columns : props.columns.length,
)

const gridStyle = computed<Record<string, string>>(() => ({
  gridTemplateColumns:
    typeof props.columns === 'number'
      ? `repeat(${props.columns}, 1fr)`
      : props.columns.join(' '),
}))
</script>

<template>
  <div>
    <div
      v-for="i in rows"
      :key="i"
      class="skeleton-row"
      :style="gridStyle"
    >
      <span v-for="j in columnCount" :key="j" />
    </div>
  </div>
</template>
