<script setup lang="ts">
// Latest-blocks table. Pure presentational: takes a Block[] prop and
// renders the rows. Row click/Enter navigates to /blocks/:number,
// preserving the current network via ?network=.

import type { Block } from '@bindings'

const props = defineProps<{
  blocks: readonly Block[]
  rowFadeIn?: boolean
}>()

const { to } = useNetworkLink()
const addrLabel = useAddrLabel()

function openBlock(number: string) {
  navigateTo(to(`/blocks/${number}`))
}
</script>

<template>
  <table class="table">
    <thead>
      <tr>
        <th>Block</th>
        <th>Status</th>
        <th>Hash</th>
        <th>Extrinsics</th>
        <th>Events</th>
        <th>Author</th>
        <th class="right">Size</th>
        <th class="right">Time</th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="b in props.blocks"
        :key="b.number"
        class="clickable-row"
        :class="{ 'row-fade-in': rowFadeIn }"
        role="link"
        tabindex="0"
        :aria-label="`Open block ${b.number}`"
        @click="openBlock(b.number)"
        @keydown.enter.space.prevent="openBlock(b.number)"
      >
        <td>
          <span class="hash block-id">#{{ fmtInt(b.number) }}</span>
        </td>
        <td data-label="Status">
          <StatusPill :finalized="b.finalized" />
        </td>
        <td data-label="Hash">
          <Hash :text="b.hash" :head="8" :tail="6" />
        </td>
        <td data-label="Extrinsics">
          <Chip>{{ b.extrinsic_count }}</Chip>
        </td>
        <td data-label="Events">
          <Chip>{{ b.event_count }}</Chip>
        </td>
        <td data-label="Author">
          <div class="author-cell">
            <Identicon :seed="b.author" :size="18" />
            <span class="mono text-xs">{{ addrLabel(b.author, b.author_name) }}</span>
          </div>
        </td>
        <td data-label="Size" class="right mono text-xs">{{ (b.size_bytes / 1024).toFixed(2) }} KB</td>
        <td data-label="Time" class="right">
          <span class="mono text-xs dim"><TimeAgo :timestamp="b.timestamp_ms" /></span>
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

.block-id {
  font-size: 13.5px;
  font-weight: 600;
}

.author-cell {
  display: flex;
  gap: 8px;
  align-items: center;
}
</style>
