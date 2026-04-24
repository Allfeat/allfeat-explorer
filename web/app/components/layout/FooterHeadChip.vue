<script setup lang="ts">
// Compact head/finalized readout for the footer. Takes the latest
// numbers as props so the caller owns the data flow (live store in
// Phase 8, static fallback otherwise). Renders `—` when unknown.

defineProps<{
  head?: string | number | null
  finalized?: string | number | null
}>()

function fmt(n: string | number | null | undefined): string {
  if (n === null || n === undefined) return '—'
  const num = typeof n === 'string' ? n : String(n)
  // Thousands separator — mockup uses narrow non-breaking space.
  return num.replace(/\B(?=(\d{3})+(?!\d))/g, '\u202F')
}
</script>

<template>
  <span class="footer-head-chip mono">
    <span>BLOCK {{ fmt(head) }}</span>
    <span class="sep">·</span>
    <span>FINALIZED {{ fmt(finalized) }}</span>
  </span>
</template>

<style scoped lang="scss">
.footer-head-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--ink-muted);
  letter-spacing: 0.04em;

  .sep {
    color: var(--ink-dimmer);
  }
}
</style>
