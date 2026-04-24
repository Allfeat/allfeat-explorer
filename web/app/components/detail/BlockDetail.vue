<script setup lang="ts">
// Block detail — KV overview on the left, Summary panel on the right,
// tabs below with the block's extrinsics / flat event list.
//
// Pure presentational: the page shell owns the fetches and the 404
// state. We trust `block`/`extrinsics` to be non-null when this component
// mounts.

import type { Block, BlockEvent, Extrinsic, NetworkSpec } from '@bindings'

const props = defineProps<{
  block: Block
  extrinsics: readonly Extrinsic[]
  events: readonly BlockEvent[]
  network: NetworkSpec | null
}>()

const route = useRoute()
const { to, blockHref, accountHref } = useNetworkLink()
const addrLabel = useAddrLabel()

const num = computed<bigint>(() => BigInt(props.block.number))
const prevNum = computed<bigint>(() => num.value > 0n ? num.value - 1n : 0n)
const nextNum = computed<bigint>(() => num.value + 1n)

// Flat event list for the Events tab. The "Event ID" column uses
// <block>-<runningIndex> to match the Substrate convention (events are
// indexed globally within a block). The phase column surfaces
// Initialization / ApplyExtrinsic / Finalization so on-finalize effects
// like `grandpa.NewAuthorities` stay visible instead of being dropped.
interface EventRow {
  id: string
  extrinsicId: string | null
  module: string
  name: string
  phaseLabel: string
}

function extrinsicIdFor(blockNumber: bigint | string, extrinsicIndex: number): string {
  return `${blockNumber}-${extrinsicIndex}`
}

const eventRows = computed<EventRow[]>(() => {
  return props.events.map((ev) => {
    const phase = ev.phase
    let extrinsicId: string | null = null
    let phaseLabel = ''
    if (phase.kind === 'apply_extrinsic') {
      extrinsicId = extrinsicIdFor(props.block.number, phase.index)
      phaseLabel = 'ApplyExtrinsic'
    }
    else if (phase.kind === 'initialization') {
      phaseLabel = 'Initialization'
    }
    else if (phase.kind === 'finalization') {
      phaseLabel = 'Finalization'
    }
    return {
      id: `${props.block.number}-${ev.index}`,
      extrinsicId,
      module: ev.module,
      name: ev.name,
      phaseLabel,
    }
  })
})

const activeTab = computed<string>(() => {
  const q = route.query.tab
  return typeof q === 'string' ? q : 'extrinsics'
})

function openExtrinsic(id: string) {
  navigateTo(to(`/extrinsics/${id}`))
}
</script>

<template>
  <div>
    <div class="page-title">
      <div>
        <h1 style="display: flex; align-items: center; gap: 14px;">
          #{{ fmtInt(block.number) }}
          <StatusPill :finalized="block.finalized" />
        </h1>
      </div>
      <div class="navs">
        <NuxtLink
          class="icon-square"
          :to="blockHref(prevNum)"
          :aria-disabled="num === 0n"
          aria-label="Previous block"
        >
          ‹
        </NuxtLink>
        <NuxtLink
          class="icon-square"
          :to="blockHref(nextNum)"
          aria-label="Next block"
        >
          ›
        </NuxtLink>
      </div>
    </div>

    <div class="block-grid">
      <div class="panel">
        <Kv>
          <KvRow label="Timestamp">
            <span class="mono">{{ fmtUtcTime(block.timestamp_ms) }}</span>
            <span class="dim mono text-xs"> · <TimeAgo :timestamp="block.timestamp_ms" /></span>
          </KvRow>
          <KvRow label="Status">
            <StatusPill :finalized="block.finalized" />
          </KvRow>
          <KvRow label="Hash">
            <Hash :text="block.hash" :head="12" :tail="12" />
          </KvRow>
          <KvRow label="Parent hash">
            <Hash :text="block.parent_hash" :to="blockHref(prevNum)" :head="12" :tail="12" />
          </KvRow>
          <KvRow label="State root">
            <Hash :text="block.state_root" :head="12" :tail="12" :copy="false" dim />
          </KvRow>
          <KvRow label="Extrinsics root">
            <Hash :text="block.extrinsics_root" :head="12" :tail="12" :copy="false" dim />
          </KvRow>
          <KvRow label="Author">
            <div style="display: flex; align-items: center; gap: 8px;">
              <Identicon :seed="block.author" :size="20" />
              <NuxtLink class="author-label mono" :to="accountHref(block.author)">
                {{ addrLabel(block.author, block.author_name) }}
              </NuxtLink>
              <Hash :text="block.author" :to="accountHref(block.author)" :head="10" :tail="10" />
            </div>
          </KvRow>
          <KvRow label="Ref time">
            <span class="mono">{{ fmtInt(block.ref_time) }}</span>
            <span class="dim"> ({{ block.ref_time_pct }}%)</span>
          </KvRow>
          <KvRow label="Proof size">
            <span class="mono">{{ fmtInt(block.proof_size) }}</span>
          </KvRow>
          <KvRow label="Spec version">
            <span class="mono">{{ block.spec_version }}</span>
          </KvRow>
          <KvRow label="Size">
            <span class="mono">{{ (block.size_bytes / 1024).toFixed(2) }} KB</span>
          </KvRow>
        </Kv>
      </div>

      <div class="panel" style="align-self: start;">
        <div class="panel-head">
          <h3>Summary</h3>
        </div>
        <div style="padding: 20px; display: flex; flex-direction: column; gap: 18px;">
          <div>
            <div class="ui-label" style="margin-bottom: 4px;">Extrinsics</div>
            <div class="big-num">{{ fmtInt(block.extrinsic_count) }}</div>
          </div>
          <div>
            <div class="ui-label" style="margin-bottom: 4px;">Events</div>
            <div class="big-num">{{ fmtInt(block.event_count) }}</div>
          </div>
          <div>
            <div class="ui-label" style="margin-bottom: 6px;">Weight used</div>
            <div class="bar" style="height: 6px;">
              <i :style="{ width: `${block.ref_time_pct}%` }" />
            </div>
            <div class="mono text-xs dim" style="margin-top: 4px;">
              {{ block.ref_time_pct }}% of block limit
            </div>
          </div>
          <div v-if="network" style="border-top: 1px solid var(--line); padding-top: 14px;">
            <div class="ui-label" style="margin-bottom: 6px;">Block time</div>
            <div class="mono">{{ network.block_time_secs }}s target</div>
          </div>
        </div>
      </div>
    </div>

    <div class="panel" style="margin-top: 24px;">
      <Tabs
        :items="[
          { id: 'extrinsics', label: 'Extrinsics', count: block.extrinsic_count },
          { id: 'events', label: 'Events', count: block.event_count },
        ]"
      />

      <table v-if="activeTab === 'extrinsics'" class="table">
        <thead>
          <tr>
            <th>Extrinsic ID</th>
            <th>Hash</th>
            <th>Action</th>
            <th>Signer</th>
            <th>Result</th>
            <th class="right">Time</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="e in extrinsics"
            :key="e.id"
            class="clickable-row"
            role="link"
            tabindex="0"
            :aria-label="`Open extrinsic ${e.id}`"
            @click="openExtrinsic(e.id)"
            @keydown.enter.space.prevent="openExtrinsic(e.id)"
          >
            <td><span class="hash" style="font-weight: 600;">{{ e.id }}</span></td>
            <td data-label="Hash"><Hash :text="e.hash" :head="8" :tail="6" /></td>
            <td data-label="Action">
              <Chip variant="module">{{ e.module }}</Chip>
              <Chip variant="call" style="margin-left: 4px;">{{ e.call }}</Chip>
            </td>
            <td data-label="Signer">
              <Addr v-if="e.signer" :text="e.signer" :to="accountHref(e.signer)" />
              <span v-else class="dim text-xs">—</span>
            </td>
            <td data-label="Result"><StatusPill :result="e.result === 'Success' ? 'success' : 'failed'" /></td>
            <td data-label="Time" class="right">
              <span class="mono text-xs dim"><TimeAgo :timestamp="e.timestamp_ms" /></span>
            </td>
          </tr>
        </tbody>
      </table>

      <table v-else-if="activeTab === 'events'" class="table">
        <thead>
          <tr>
            <th>Event ID</th>
            <th>Extrinsic</th>
            <th>Event</th>
            <th>Phase</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="ev in eventRows" :key="ev.id">
            <td><span class="hash">{{ ev.id }}</span></td>
            <td data-label="Extrinsic">
              <button
                v-if="ev.extrinsicId"
                type="button"
                class="hash link-button"
                :aria-label="`Open extrinsic ${ev.extrinsicId}`"
                @click="openExtrinsic(ev.extrinsicId)"
                @keydown.enter.space.prevent="openExtrinsic(ev.extrinsicId)"
              >
                {{ ev.extrinsicId }}
              </button>
              <span v-else class="dim text-xs">—</span>
            </td>
            <td data-label="Event">
              <Chip variant="module">{{ ev.module }}</Chip>
              <Chip variant="call" style="margin-left: 4px;">{{ ev.name }}</Chip>
            </td>
            <td data-label="Phase"><span class="mono text-xs dim">{{ ev.phaseLabel }}</span></td>
          </tr>
          <tr v-if="eventRows.length === 0">
            <td colspan="4" style="padding: 40px; text-align: center;" class="dim">
              No events recorded for this block.
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped lang="scss">
.block-grid {
  display: grid;
  grid-template-columns: 1fr 320px;
  gap: 24px;
  margin-top: 24px;
}

@media (max-width: 900px) {
  .block-grid {
    grid-template-columns: 1fr;
  }
}

.big-num {
  font-family: var(--font-display);
  font-size: 32px;
  font-weight: 700;
  letter-spacing: -0.02em;
}

.navs .icon-square {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  text-decoration: none;
}

.clickable-row {
  cursor: pointer;

  &:focus-visible {
    outline: 2px solid var(--teal-500);
    outline-offset: -2px;
  }
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

.author-label {
  font-weight: 600;
  font-size: 12px;
  color: inherit;
  text-decoration: none;

  &:hover {
    color: var(--teal-500);
  }
}
</style>
