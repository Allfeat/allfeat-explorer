<script setup lang="ts">
// Pure presentation — consumes a connection state string and renders the
// `.connection-pill.connection-pill--<state>` chip. Wired to the live
// store in Phase 8 via `useConnectionState()`.

export type ConnState = 'connecting' | 'connected' | 'reconnecting' | 'offline'

const { state } = defineProps<{
  state: ConnState
}>()

const LABELS: Record<ConnState, string> = {
  connecting: 'Connecting',
  connected: 'Live',
  reconnecting: 'Reconnecting',
  offline: 'Offline',
}
</script>

<template>
  <span
    class="connection-pill"
    :class="[`connection-pill--${state}`]"
    role="status"
    :aria-label="`Live feed: ${LABELS[state]}`"
  >
    <span v-if="state === 'connected'" class="bars" aria-hidden="true">
      <i /><i /><i />
    </span>
    <span v-else class="dot" />
    <span class="label">{{ LABELS[state] }}</span>
  </span>
</template>
