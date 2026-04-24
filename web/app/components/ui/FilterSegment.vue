<script setup lang="ts">
// URL-driven segmented control (`?filter=<id>` by default). Same
// mechanics as <Tabs/> but rendered with the .seg visual language.

interface SegItem {
  id: string
  label: string
}

const props = withDefaults(defineProps<{
  items: readonly SegItem[]
  queryKey?: string
  defaultId?: string | null
}>(), {
  queryKey: 'filter',
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

// Sliding pill — same pattern as <Tabs/>. Measures the active button
// and translates a shared background element to its position.
const root = ref<HTMLElement | null>(null)
const indicator = ref({ left: 0, width: 0, ready: false })

function updateIndicator() {
  if (!root.value) return
  const btn = root.value.querySelector<HTMLButtonElement>('button.active')
  if (!btn) {
    indicator.value = { left: 0, width: 0, ready: false }
    return
  }
  indicator.value = { left: btn.offsetLeft, width: btn.offsetWidth, ready: true }
}

onMounted(() => {
  updateIndicator()
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
  <div ref="root" class="seg">
    <span
      class="seg__indicator"
      :class="{ ready: indicator.ready }"
      :style="{
        transform: `translateX(${indicator.left}px)`,
        width: `${indicator.width}px`,
      }"
    />
    <button
      v-for="it in items"
      :key="it.id"
      type="button"
      :class="{ active: it.id === activeId }"
      @click="select(it.id)"
    >
      {{ it.label }}
    </button>
  </div>
</template>
