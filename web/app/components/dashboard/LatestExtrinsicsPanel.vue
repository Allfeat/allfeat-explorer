<script setup lang="ts">
// Dashboard panel — toggles between the latest extrinsics and the
// latest events (chain-wide, including Initialization/Finalization
// phases). Parents own both feeds (see useLiveExtrinsics / useLiveEvents);
// this component is purely presentational.

import type { BlockEvent, Extrinsic } from '@bindings'

defineProps<{
  extrinsics: readonly Extrinsic[]
  events: readonly BlockEvent[]
}>()

type Mode = 'extrinsics' | 'events'
const mode = ref<Mode>('extrinsics')

const { queryString, to } = useNetworkLink()

// Events only link to an extrinsic for `ApplyExtrinsic`-phase rows;
// Initialization / Finalization events belong to the block as a whole,
// so the row navigates to the block instead.
function eventHref(ev: BlockEvent): string {
  if (ev.phase.kind === 'apply_extrinsic') {
    return `/extrinsics/${ev.block_number}-${ev.phase.index}`
  }
  return `/blocks/${ev.block_number}`
}

// Short label under the event's call chip so the user can tell an
// extrinsic-attached event apart from an on_initialize/on_finalize one.
function eventSubLabel(ev: BlockEvent): string {
  switch (ev.phase.kind) {
    case 'apply_extrinsic':
      return `extrinsic ${ev.block_number}-${ev.phase.index}`
    case 'initialization':
      return 'on_initialize'
    case 'finalization':
      return 'on_finalize'
  }
}
</script>

<template>
  <div class="panel">
    <div class="panel-head">
      <h3>{{ mode === 'extrinsics' ? 'Latest extrinsics' : 'Latest events' }}</h3>
      <div class="seg panel-seg">
        <button
          type="button"
          :class="{ active: mode === 'extrinsics' }"
          @click="mode = 'extrinsics'"
        >
          Extrinsics
        </button>
        <button
          type="button"
          :class="{ active: mode === 'events' }"
          @click="mode = 'events'"
        >
          Events
        </button>
      </div>
      <div class="spacer" />
      <NuxtLink :to="`/extrinsics${queryString}`" class="ui-label">View all →</NuxtLink>
    </div>

    <table v-if="mode === 'extrinsics' && extrinsics.length > 0" class="table compact">
      <tbody>
        <tr
          v-for="x in extrinsics.slice(0, 8)"
          :key="x.id"
          class="row-fade-in clickable-row"
          role="link"
          tabindex="0"
          @click="navigateTo(to(`/extrinsics/${x.id}`))"
          @keydown.enter.space.prevent="navigateTo(to(`/extrinsics/${x.id}`))"
        >
          <td class="col-id">
            <div class="ui-label col-id-kicker">Extrinsic</div>
            <span class="hash col-id-value">{{ x.id }}</span>
          </td>
          <td>
            <div class="row-mid">
              <div class="call-line">
                <Chip variant="module">{{ x.module }}</Chip>
                <Chip variant="call" class="call-chip">{{ x.call }}</Chip>
              </div>
              <span class="text-xs dim">
                {{ x.signed && x.signer ? 'signed' : 'unsigned' }}
                <span v-if="x.events.length > 0">· {{ x.events.length }} evt</span>
              </span>
            </div>
          </td>
          <td class="right col-meta">
            <div class="col-meta-stack">
              <StatusPill :result="x.result === 'Success' ? 'success' : 'failed'" />
              <span class="mono text-xs dim"><TimeAgo :timestamp="x.timestamp_ms" /></span>
            </div>
          </td>
        </tr>
      </tbody>
    </table>

    <table v-else-if="mode === 'events' && events.length > 0" class="table compact">
      <tbody>
        <tr
          v-for="ev in events.slice(0, 8)"
          :key="`${ev.block_number}-${ev.index}`"
          class="row-fade-in clickable-row"
          role="link"
          tabindex="0"
          @click="navigateTo(to(eventHref(ev)))"
          @keydown.enter.space.prevent="navigateTo(to(eventHref(ev)))"
        >
          <td class="col-id">
            <div class="ui-label col-id-kicker">Block</div>
            <span class="hash col-id-value">#{{ fmtInt(ev.block_number) }}</span>
          </td>
          <td>
            <div class="row-mid">
              <div class="call-line">
                <Chip variant="module">{{ ev.module }}</Chip>
                <Chip variant="call" class="call-chip">{{ ev.name }}</Chip>
              </div>
              <span class="text-xs dim">{{ eventSubLabel(ev) }}</span>
            </div>
          </td>
          <td class="right col-meta">
            <span class="mono text-xs dim"><TimeAgo :timestamp="ev.timestamp_ms" /></span>
          </td>
        </tr>
      </tbody>
    </table>

    <SkeletonRows v-else :rows="5" :columns="['120px', '1fr', '140px']" />
  </div>
</template>

<style scoped lang="scss">
.panel-seg {
  margin-left: 8px;
}

.clickable-row {
  cursor: pointer;

  &:focus-visible {
    outline: 2px solid var(--teal-500);
    outline-offset: -2px;
  }
}

.col-id {
  width: 120px;
}

.col-id-kicker {
  margin-bottom: 2px;
}

.col-id-value {
  font-size: 14px;
  font-weight: 600;
}

.row-mid {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.call-line {
  display: flex;
  align-items: center;
  gap: 2px;
}

.call-chip {
  margin-left: 4px;
}

.col-meta {
  width: 140px;
}

.col-meta-stack {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
}
</style>
