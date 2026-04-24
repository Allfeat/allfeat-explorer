<script setup lang="ts">
// Deterministic "visual fingerprint" derived from an ATS commitment hash.
// Each bar's height is drawn from consecutive hex bytes — same commitment
// always renders the same waveform, so the shape becomes an at-a-glance
// identity cue without being cryptographic.
//
// Ported from the reference maquette's ATSWave React component so the
// exact bar geometry/opacity rules stay identical.

const props = withDefaults(defineProps<{
  commitment: string
  bars?: number
  height?: number
  color?: string
  opacity?: number
}>(), {
  bars: 48,
  height: 28,
  color: 'var(--teal-500)',
  opacity: 1,
})

const values = computed<number[]>(() => {
  const hex = props.commitment.startsWith('0x')
    ? props.commitment.slice(2)
    : props.commitment
  const out: number[] = []
  if (hex.length < 2) return out
  for (let i = 0; i < props.bars; i++) {
    const a = hex[(i * 3) % hex.length] ?? '0'
    const b = hex[(i * 3 + 1) % hex.length] ?? '0'
    const byte = Number.parseInt(a + b, 16)
    out.push(0.15 + (byte / 255) * 0.85)
  }
  return out
})
</script>

<template>
  <div
    class="ats-wave"
    :style="{ height: `${height}px`, opacity: String(opacity) }"
    aria-hidden="true"
  >
    <span
      v-for="(v, i) in values"
      :key="i"
      class="ats-wave__bar"
      :style="{
        height: `${Math.max(2, v * height)}px`,
        background: color,
        opacity: 0.4 + v * 0.6,
      }"
    />
  </div>
</template>

<style scoped>
.ats-wave {
  display: flex;
  align-items: center;
  gap: 2px;
}

.ats-wave__bar {
  display: inline-block;
  width: 2px;
  border-radius: 1px;
}
</style>
