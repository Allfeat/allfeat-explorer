<script setup lang="ts">
// Thin wrapper that maps extrinsic/block status semantics to a Pill
// variant + label. Ported from the maquette's `StatusPill`:
//   success | finalized=true  → success / 'Finalized' or 'Success'
//   failed                    → fail / 'Failed'
//   finalized=false           → pending / 'In block'
//   otherwise                 → neutral / passthrough or 'Pending'

type Result = 'success' | 'failed' | null

const props = withDefaults(defineProps<{
  result?: Result
  finalized?: boolean | null
}>(), {
  result: null,
  finalized: null,
})

const resolved = computed<{ variant: 'success' | 'fail' | 'pending' | 'neutral', label: string }>(() => {
  if (props.result === 'success' || props.finalized === true) {
    return { variant: 'success', label: props.finalized ? 'Finalized' : 'Success' }
  }
  if (props.result === 'failed') {
    return { variant: 'fail', label: 'Failed' }
  }
  if (props.finalized === false) {
    return { variant: 'pending', label: 'In block' }
  }
  return { variant: 'neutral', label: props.result ?? 'Pending' }
})
</script>

<template>
  <Pill :variant="resolved.variant">{{ resolved.label }}</Pill>
</template>
