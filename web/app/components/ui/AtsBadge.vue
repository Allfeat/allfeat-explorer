<script setup lang="ts">
// Teal-tinted "ATS" badge with a shield-check glyph. Used in ATS detail
// subtitles and anywhere the Allfeat Timestamp pallet needs a visual
// marker. Ported from the reference ATSBadge.

withDefaults(defineProps<{
  size?: number
}>(), {
  size: 14,
})
</script>

<template>
  <span class="ats-badge">
    <svg
      :width="size"
      :height="size"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="M12 3 3 7v6c0 4.5 3.5 7.5 9 8 5.5-.5 9-3.5 9-8V7z" />
      <path d="m8.5 12 2.5 2.5L16 10" />
    </svg>
    ATS
  </span>
</template>

<style scoped>
.ats-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 8px;
  border-radius: 4px;
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--teal-500);
  background: rgba(0, 177, 140, 0.1);
  border: 1px solid rgba(0, 177, 140, 0.2);
}
</style>
