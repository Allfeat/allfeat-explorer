<script setup lang="ts">
// URL-driven tab strip: the active tab lives in `?tab=<id>` (query key
// configurable). Clicking a tab replaces history so the browser's back
// button doesn't accumulate one entry per toggle.
//
// Consumers read `route.query[queryKey]` directly to render the tab
// body; an @change event is also emitted for cases where a local ref is
// more convenient.

interface TabItem {
  id: string
  label: string
  count?: number | null
}

const props = withDefaults(defineProps<{
  items: readonly TabItem[]
  queryKey?: string
  defaultId?: string | null
}>(), {
  queryKey: 'tab',
  defaultId: null,
})

const emit = defineEmits<{
  change: [id: string]
}>()

const route = useRoute()

const activeId = computed<string>(() => {
  const q = route.query[props.queryKey]
  if (typeof q === 'string' && props.items.some(it => it.id === q)) return q
  if (props.defaultId && props.items.some(it => it.id === props.defaultId)) {
    return props.defaultId
  }
  return props.items[0]?.id ?? ''
})

async function select(id: string) {
  if (id === activeId.value) return
  await navigateTo(
    { query: { ...route.query, [props.queryKey]: id } },
    { replace: true },
  )
  emit('change', id)
}

// Sliding indicator — measures the active button's layout and mirrors
// its position onto an absolute bar. Avoids per-button border-bottom
// animations (which can't transition across DOM nodes).
const root = ref<HTMLElement | null>(null)
const indicator = ref({ left: 0, width: 0, ready: false })

function updateIndicator() {
  if (!root.value) return
  const btn = root.value.querySelector<HTMLButtonElement>('.tab.active')
  if (!btn) {
    indicator.value = { left: 0, width: 0, ready: false }
    return
  }
  indicator.value = { left: btn.offsetLeft, width: btn.offsetWidth, ready: true }
}

onMounted(() => {
  updateIndicator()
  // Fonts load asynchronously and change button widths — re-measure once.
  if (typeof document !== 'undefined' && document.fonts?.ready) {
    document.fonts.ready.then(updateIndicator).catch(() => {})
  }
  window.addEventListener('resize', updateIndicator, { passive: true })
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', updateIndicator)
})

watch([activeId, () => props.items.length], () => nextTick(updateIndicator))

defineExpose({ activeId })
</script>

<template>
  <div ref="root" class="tabs">
    <button
      v-for="it in items"
      :key="it.id"
      type="button"
      class="tab"
      :class="{ active: it.id === activeId }"
      @click="select(it.id)"
    >
      {{ it.label }}
      <span v-if="it.count != null" class="count">{{ it.count }}</span>
    </button>
    <span
      class="tabs__indicator"
      :class="{ ready: indicator.ready }"
      :style="{
        transform: `translateX(${indicator.left}px)`,
        width: `${indicator.width}px`,
      }"
    />
  </div>
</template>
