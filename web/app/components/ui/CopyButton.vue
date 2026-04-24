<script setup lang="ts">
// Copy-to-clipboard button. Flashes a check icon for ~900ms on success.
// Click is stop-propagated so the button works inside clickable rows
// without triggering row navigation.

const props = withDefaults(defineProps<{
  text: string
  size?: number
}>(), {
  size: 13,
})

const copied = ref(false)
let flashTimer: ReturnType<typeof setTimeout> | null = null

async function onClick(event: MouseEvent) {
  event.stopPropagation()
  if (typeof navigator === 'undefined' || !navigator.clipboard) return
  try {
    await navigator.clipboard.writeText(props.text)
    copied.value = true
    if (flashTimer) clearTimeout(flashTimer)
    flashTimer = setTimeout(() => {
      copied.value = false
    }, 900)
  } catch {
    // Clipboard write can reject under locked-down perms; stay silent.
  }
}

onBeforeUnmount(() => {
  if (flashTimer) clearTimeout(flashTimer)
})
</script>

<template>
  <button
    type="button"
    class="copy-btn"
    :title="copied ? 'Copied' : 'Copy'"
    :aria-label="copied ? 'Copied' : 'Copy'"
    @click="onClick"
  >
    <svg
      v-if="copied"
      :width="size"
      :height="size"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="m5 12 5 5 9-11" />
    </svg>
    <svg
      v-else
      :width="size"
      :height="size"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V6a2 2 0 0 1 2-2h9" />
    </svg>
  </button>
</template>
