<script setup lang="ts">
// Layout-level seed for the live store: the footer renders inside this
// layout (above the <NuxtPage/> slot), so page-level useLiveBlocks calls
// would arrive too late for the SSR pass of `<FooterHeadChip/>`. Running
// the seed here makes BLOCK / FINALIZED render with real numbers on
// the first byte delivered to the browser.
//
// useFetch deduplicates across same-URL callers, so pages that also call
// useLiveBlocks share this cache entry rather than re-hitting the API.

import { useLiveBlocks } from '~/composables/useLiveBlocks'
import { useLiveWaveform } from '~/composables/useLiveWaveform'

// Non-blocking: the composable registers a useFetch that Nuxt waits on
// during SSR rendering, so the server payload still carries the seed,
// but on client-side navigation the setup returns synchronously and the
// page transition isn't held up by the round-trip.
useLiveBlocks({ count: 25 })
// Hero waveform needs a 72-block window; lean payload shaved by the
// dedicated /waveform endpoint. Seeded here for the same SSR-readiness
// reason — the dashboard hero is above the fold.
useLiveWaveform({ count: 72 })
</script>

<template>
  <div class="app-shell">
    <NuxtLoadingIndicator color="var(--teal-500)" :height="2" />
    <AppHeader />
    <IndexingBanner />

    <main class="app-shell__main">
      <slot />
    </main>

    <AppFooter />
  </div>
</template>

<style scoped lang="scss">
.app-shell {
  display: flex;
  flex-direction: column;
  min-height: 100vh;

  &__main {
    flex: 1;
  }
}
</style>
