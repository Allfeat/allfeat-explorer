<script setup lang="ts">
// Runtime "View at" toolbar — segmented control (Latest / At block…),
// inline block-number form, dismissable `?at=N` badge, and a
// context-dependent deployment banner on the right-hand side.
//
// Owns navigation directly: submitting / clearing the form pushes the
// new `?at=` query onto the route. The parent only needs to observe
// `?at=` via its own `atParam` computed.

import { fmtInt } from '~/utils/format'

defineProps<{
  atParam: number | null
  deploymentAgo: string | null
  deploymentBlockHref: string | null
  deploymentBlockLabel: string | null
  deploymentUnknown: boolean
}>()

const route = useRoute()
const router = useRouter()

const atFormOpen = ref(false)
const atFormValue = ref('')

// Opening the form pre-fills with the current `?at=` value (or nothing
// when we're already on the latest snapshot), so editing a single
// digit doesn't force the user to re-type the whole number.
function openAtForm(atParam: number | null) {
  atFormValue.value = atParam != null ? String(atParam) : ''
  atFormOpen.value = true
  // Let Vue commit the v-show/display change before focusing the input.
  nextTick(() => {
    const el = document.getElementById('rt-at-input') as HTMLInputElement | null
    el?.focus()
    el?.select()
  })
}

function cancelAtForm() {
  atFormOpen.value = false
}

async function submitAtForm() {
  const trimmed = atFormValue.value.trim()
  if (trimmed === '') {
    // Empty input is "go back to latest"; removing the query string is
    // what the parent's `atParam` reads as `null`, which flips the fetch
    // key back to `:latest`.
    await router.push({ query: { ...route.query, at: undefined } })
    atFormOpen.value = false
    return
  }
  const n = Number(trimmed)
  if (!Number.isFinite(n) || n < 0 || Math.floor(n) !== n) {
    // Keep the form open on a bad input rather than silently dropping
    // the submission. The input visually carries the invalid value so
    // the user can see what they typed.
    return
  }
  await router.push({ query: { ...route.query, at: String(Math.floor(n)) } })
  atFormOpen.value = false
}

async function resetToLatest() {
  await router.push({ query: { ...route.query, at: undefined } })
  atFormOpen.value = false
}

// Sliding-pill indicator for the segmented control. "Latest" and
// "At block…" have different widths, so the pill is measured from the
// live button geometry rather than being hardcoded — same approach as
// `FilterSegment.vue`. `.ready` gates the opacity so the pill doesn't
// flash at (0,0) on first paint before measurement runs.
const segRoot = ref<HTMLElement | null>(null)
const segIndicator = ref({ left: 0, width: 0, ready: false })

function updateSegIndicator() {
  if (!segRoot.value) return
  const btn = segRoot.value.querySelector<HTMLButtonElement>('button.active')
  if (!btn) {
    segIndicator.value = { left: 0, width: 0, ready: false }
    return
  }
  segIndicator.value = { left: btn.offsetLeft, width: btn.offsetWidth, ready: true }
}

onMounted(() => {
  updateSegIndicator()
  if (typeof document !== 'undefined' && document.fonts?.ready) {
    // `Space Grotesk` + `JetBrains Mono` load async — measuring before
    // they land would pin the pill at the fallback-font width. Running
    // once more after the font promise resolves is cheap and invisible.
    document.fonts.ready.then(updateSegIndicator).catch(() => {})
  }
  window.addEventListener('resize', updateSegIndicator, { passive: true })
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', updateSegIndicator)
})

// Re-measure when the active segment or form visibility flips — the
// pill needs to slide to the new button's geometry.
watch(() => [route.query.at, atFormOpen.value], () => nextTick(updateSegIndicator))

const { blockHref } = useNetworkLink()
</script>

<template>
  <div class="rt-toolbar">
    <span class="tool-label">View runtime at</span>
    <div ref="segRoot" class="seg rt-seg">
      <span
        class="seg__indicator"
        :class="{ ready: segIndicator.ready }"
        :style="{
          transform: `translateX(${segIndicator.left}px)`,
          width: `${segIndicator.width}px`,
        }"
      />
      <button
        type="button"
        :class="{ active: atParam == null }"
        @click="resetToLatest"
      >
        Latest
      </button>
      <button
        type="button"
        :class="{ active: atParam != null }"
        @click="openAtForm(atParam)"
      >
        At block…
      </button>
    </div>

    <!-- Selected-block badge. Shows the current `?at=N` as a small
         mono tag with a dismiss affordance — keeps the segmented
         control's label static ("At block…") while still giving the
         user a visible handle on *which* block they're viewing, and
         a one-click way back to latest. Hidden while the input form
         is open (the form itself carries the value). -->
    <span v-if="atParam != null && !atFormOpen" class="rt-at-active">
      <NuxtLink class="mono rt-at-active__block" :to="blockHref(atParam)">
        #{{ fmtInt(atParam) }}
      </NuxtLink>
      <button
        type="button"
        class="rt-at-active__clear"
        title="Back to latest"
        @click="resetToLatest"
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M6 6l12 12" />
          <path d="M18 6L6 18" />
        </svg>
      </button>
    </span>

    <form
      v-if="atFormOpen"
      class="rt-at-form"
      @submit.prevent="submitAtForm"
    >
      <input
        id="rt-at-input"
        v-model="atFormValue"
        type="text"
        inputmode="numeric"
        pattern="[0-9]*"
        placeholder="block #"
        autocomplete="off"
      />
      <button type="submit" class="btn sm">Go</button>
      <button type="button" class="btn sm ghost" @click="cancelAtForm">Cancel</button>
    </form>

    <div class="rt-toolbar__spacer" />

    <!-- Right side: the runtime's deployment info when viewing
         "Latest", or a "Viewing at" restatement when the user has
         pinned `?at=N`. Two separate templates read more clearly
         than a ternary inside one span. -->
    <span v-if="atParam != null" class="rt-deploy rt-deploy--viewing">
      <span class="tool-label">Viewing</span>
      <span class="mono rt-deploy__when">runtime at</span>
      <span class="sep">·</span>
      <NuxtLink class="hash" :to="blockHref(atParam)">
        block #{{ fmtInt(atParam) }}
      </NuxtLink>
    </span>
    <span v-else-if="deploymentBlockHref" class="rt-deploy">
      <span class="tool-label">Deployed</span>
      <span class="mono rt-deploy__when">{{ deploymentAgo }}</span>
      <span class="sep">·</span>
      <NuxtLink class="hash" :to="deploymentBlockHref">
        block {{ deploymentBlockLabel }}
      </NuxtLink>
    </span>
    <span v-else-if="deploymentUnknown" class="tool-label rt-deploy rt-deploy--unknown">
      Deployment block not indexed
    </span>
  </div>
</template>

<style scoped lang="scss">
.rt-toolbar {
  display: flex;
  gap: 12px;
  align-items: center;
  padding: 16px 0;
  border-bottom: 1px solid var(--line);
  flex-wrap: wrap;

  .tool-label {
    font-family: var(--font-mono);
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-dimmer);
  }

  &__spacer {
    flex: 1;
  }
}

// The segmented control borrows the app-wide `.seg` base styles
// (sliding pill, mono labels); indicator geometry is measured
// reactively in `updateSegIndicator`, so no hardcoded offsets live
// here anymore. The `min-width` keeps both buttons legible when one
// is Inactive and typography is narrower than the other.
.rt-seg {
  button {
    min-width: 72px;
    text-align: center;
  }
}

// Dismissable badge shown when `?at=N` is active. Styled as a
// borderless mono chip sized to the segmented control's own height so
// the two controls line up visually — the X button is a minimal
// icon-square that slots next to the block link without adding
// another border.
.rt-at-active {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  border: 1px solid var(--line-2);
  border-radius: 100px;
  font-size: 11px;
  color: var(--ink);

  &__block {
    font-family: var(--font-mono);
    font-size: 11.5px;
    letter-spacing: 0.02em;
    color: var(--ink);
    text-decoration: none;

    &:hover {
      color: var(--teal-500);
    }
  }

  &__clear {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border: 0;
    background: transparent;
    color: var(--ink-dimmer);
    cursor: pointer;
    padding: 0;
    border-radius: 50%;
    transition:
      color 0.18s ease,
      background 0.18s ease;

    &:hover {
      color: var(--ink);
      background: var(--hover);
    }
  }
}

[data-theme="light"] .rt-at-active__block:hover {
  color: var(--teal-700);
}

// Inline "At block #…" input that slides in to the right of the
// segmented control when the second segment is clicked.
.rt-at-form {
  display: inline-flex;
  align-items: center;
  gap: 6px;

  input {
    width: 140px;
    padding: 6px 10px;
    font-family: var(--font-mono);
    font-size: 13px;
    border: 1px solid var(--line-2);
    background: var(--bg-1);
    color: var(--ink);
    outline: none;

    &:focus {
      border-color: var(--teal-500);
    }
  }
}

[data-theme="light"] .rt-at-form input:focus {
  border-color: var(--teal-700);
}

.rt-deploy {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: var(--ink);

  &__when {
    color: var(--ink);
  }

  .sep {
    color: var(--ink-dimmer);
  }

  &--unknown {
    color: var(--ink-dimmer);
    font-style: italic;
    text-transform: none;
    letter-spacing: normal;
  }
}
</style>
