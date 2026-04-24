<script setup lang="ts">
// Events table. Each row represents one emitted event, keyed by
// `<block>-<eventIndex>` (Substrate's canonical event id). Rows navigate
// sideways: `ApplyExtrinsic`-phase events open the owning extrinsic,
// `Initialization` / `Finalization` events open the block, since they
// don't belong to any single call.
//
// "Event ID" and "Extrinsic" carry different indices: the event id uses
// the running event index within the block, whereas the extrinsic id uses
// the extrinsic index (from the `ApplyExtrinsic` phase). They are not
// interchangeable — a block with one extrinsic and six events will have
// event ids `…-0` through `…-5` but only extrinsic id `…-0`.

import type { BlockEvent } from '@bindings'

defineProps<{
  events: readonly BlockEvent[]
}>()

const { to } = useNetworkLink()

function phaseLabel(ev: BlockEvent): string {
  switch (ev.phase.kind) {
    case 'apply_extrinsic': return 'ApplyExtrinsic'
    case 'initialization': return 'Initialization'
    case 'finalization': return 'Finalization'
  }
}

function extrinsicIdFor(ev: BlockEvent): string | null {
  if (ev.phase.kind !== 'apply_extrinsic') return null
  return `${ev.block_number}-${ev.phase.index}`
}

function openRow(ev: BlockEvent) {
  const extId = extrinsicIdFor(ev)
  if (extId) navigateTo(to(`/extrinsics/${extId}`))
  else navigateTo(to(`/blocks/${ev.block_number}`))
}

function openBlock(number: string, event: Event) {
  event.stopPropagation()
  navigateTo(to(`/blocks/${number}`))
}

function openExtrinsic(id: string, event: Event) {
  event.stopPropagation()
  navigateTo(to(`/extrinsics/${id}`))
}
</script>

<template>
  <table class="table">
    <thead>
      <tr>
        <th>Event ID</th>
        <th>Block</th>
        <th>Extrinsic</th>
        <th>Event</th>
        <th>Phase</th>
        <th>Data</th>
        <th class="right">Time</th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="ev in events"
        :key="`${ev.block_number}-${ev.index}`"
        class="clickable-row"
        role="link"
        tabindex="0"
        :aria-label="`Open event ${ev.block_number}-${ev.index}`"
        @click="openRow(ev)"
        @keydown.enter.space.prevent="openRow(ev)"
      >
        <td><span class="hash event-id">{{ ev.block_number }}-{{ ev.index }}</span></td>
        <td data-label="Block">
          <button
            type="button"
            class="hash link-button"
            :aria-label="`Open block ${ev.block_number}`"
            @click="openBlock(ev.block_number, $event)"
            @keydown.enter.space.prevent="openBlock(ev.block_number, $event)"
          >
            #{{ fmtInt(ev.block_number) }}
          </button>
        </td>
        <td data-label="Extrinsic">
          <button
            v-if="extrinsicIdFor(ev)"
            type="button"
            class="hash link-button"
            :aria-label="`Open extrinsic ${extrinsicIdFor(ev)}`"
            @click="openExtrinsic(extrinsicIdFor(ev)!, $event)"
            @keydown.enter.space.prevent="openExtrinsic(extrinsicIdFor(ev)!, $event)"
          >
            {{ extrinsicIdFor(ev) }}
          </button>
          <span v-else class="dim text-xs">—</span>
        </td>
        <td data-label="Event">
          <Chip variant="module">{{ ev.module }}</Chip>
          <Chip variant="call" class="call-chip">{{ ev.name }}</Chip>
        </td>
        <td data-label="Phase">
          <span class="mono text-xs dim">{{ phaseLabel(ev) }}</span>
        </td>
        <td data-label="Data">
          <span v-if="ev.fields.length === 0" class="mono text-xs dim">—</span>
          <div v-else class="fields">
            <span
              v-for="(f, j) in ev.fields"
              :key="j"
              class="mono text-xs field-row"
            >
              <span class="dim">{{ f.name ?? `#${j}` }}:</span>
              <span class="field-val">{{ f.value }}</span>
            </span>
          </div>
        </td>
        <td data-label="Time" class="right">
          <span class="mono text-xs dim"><TimeAgo :timestamp="ev.timestamp_ms" /></span>
        </td>
      </tr>
    </tbody>
  </table>
</template>

<style scoped lang="scss">
.clickable-row {
  cursor: pointer;

  &:focus-visible {
    outline: 2px solid var(--teal-500);
    outline-offset: -2px;
  }
}

.event-id {
  font-weight: 600;
}

.call-chip {
  margin-left: 4px;
}

.fields {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-width: 420px;
}

.field-row {
  display: block;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.field-val {
  margin-left: 4px;
}

.link-button {
  background: none;
  border: none;
  padding: 0;
  font: inherit;
  color: inherit;
  cursor: pointer;

  &:hover,
  &:focus-visible {
    color: var(--teal-500);
  }

  &:focus-visible {
    outline: 2px solid var(--teal-500);
    outline-offset: 2px;
    border-radius: 2px;
  }
}
</style>
