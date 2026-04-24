<script setup lang="ts">
// Dark/light theme toggle. Backed by `@nuxtjs/color-mode`, which sets
// `data-theme="dark"|"light"` on <html> (configured in nuxt.config.ts).
// The `prefers-color-scheme` media rule in `_tokens.scss` already covers
// first-load system detection; this button lets the user commit to an
// explicit preference, persisted in localStorage by the module.
//
// Wrapped in <ClientOnly> at the call site: `colorMode.value` is resolved
// from the injected head script / localStorage and may not match the
// server-rendered default on hydration.

const colorMode = useColorMode()

function toggle() {
  colorMode.preference = colorMode.value === 'dark' ? 'light' : 'dark'
}
</script>

<template>
  <button
    type="button"
    class="theme-switch icon-btn"
    :aria-label="colorMode.value === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'"
    :title="colorMode.value === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'"
    @click="toggle"
  >
    <svg
      v-if="colorMode.value === 'dark'"
      width="16" height="16" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="1.5"
      stroke-linecap="round" stroke-linejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2" />
      <path d="M12 20v2" />
      <path d="m4.93 4.93 1.41 1.41" />
      <path d="m17.66 17.66 1.41 1.41" />
      <path d="M2 12h2" />
      <path d="M20 12h2" />
      <path d="m6.34 17.66-1.41 1.41" />
      <path d="m19.07 4.93-1.41 1.41" />
    </svg>
    <svg
      v-else
      width="16" height="16" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="1.5"
      stroke-linecap="round" stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  </button>
</template>
