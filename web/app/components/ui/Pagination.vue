<script setup lang="ts">
// URL-driven pagination. The current page lives in `?page=<n>` and
// clicking a number pushes a new history entry (unlike tabs) — users
// expect the browser back button to walk back through their pages.

const props = withDefaults(defineProps<{
  total: number
  queryKey?: string
  defaultPage?: number
}>(), {
  queryKey: 'page',
  defaultPage: 1,
})

const emit = defineEmits<{
  change: [page: number]
}>()

const route = useRoute()

const currentPage = computed<number>(() => {
  const q = route.query[props.queryKey]
  if (typeof q !== 'string') return props.defaultPage
  const n = Number.parseInt(q, 10)
  if (!Number.isFinite(n) || n < 1 || n > props.total) return props.defaultPage
  return n
})

const slots = computed(() => paginationPages(currentPage.value, props.total))
const prevDisabled = computed(() => currentPage.value <= 1)
const nextDisabled = computed(() => currentPage.value >= props.total)

async function go(page: number) {
  if (page < 1 || page > props.total || page === currentPage.value) return
  await navigateTo({
    query: { ...route.query, [props.queryKey]: String(page) },
  })
  emit('change', page)
}

defineExpose({ currentPage })
</script>

<template>
  <nav v-if="total > 1" class="pagination" aria-label="Pagination">
    <button
      type="button"
      :disabled="prevDisabled"
      aria-label="Previous page"
      @click="go(currentPage - 1)"
    >
      ‹
    </button>
    <template v-for="(slot, i) in slots" :key="`${i}-${slot}`">
      <span v-if="slot === 'ellipsis'" class="ellipsis">…</span>
      <button
        v-else
        type="button"
        :class="{ current: slot === currentPage }"
        :aria-current="slot === currentPage ? 'page' : undefined"
        @click="go(slot)"
      >
        {{ slot }}
      </button>
    </template>
    <button
      type="button"
      :disabled="nextDisabled"
      aria-label="Next page"
      @click="go(currentPage + 1)"
    >
      ›
    </button>
  </nav>
</template>
