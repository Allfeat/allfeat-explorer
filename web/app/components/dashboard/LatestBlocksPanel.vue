<script setup lang="ts">
// Dashboard panel — latest blocks with a live indicator. Consumes the
// store via the parent (storeToRefs) and renders the top N rows.

import type { Block } from '@bindings'

defineProps<{
  blocks: readonly Block[]
}>()

const { queryString, to } = useNetworkLink()
const addrLabel = useAddrLabel()
</script>

<template>
  <div class="panel">
    <div class="panel-head">
      <h3>Latest blocks</h3>
      <LiveDot class="live-dot-accent" />
      <span class="ui-label">live</span>
      <div class="spacer" />
      <NuxtLink :to="`/blocks${queryString}`" class="ui-label">View all →</NuxtLink>
    </div>

    <table v-if="blocks.length > 0" class="table compact">
      <tbody>
        <tr
          v-for="b in blocks.slice(0, 8)"
          :key="b.number"
          class="row-fade-in clickable-row"
          role="link"
          tabindex="0"
          @click="navigateTo(to(`/blocks/${b.number}`))"
          @keydown.enter.space.prevent="navigateTo(to(`/blocks/${b.number}`))"
        >
          <td class="col-id">
            <div class="ui-label col-id-kicker">Block</div>
            <span class="hash col-id-value">{{ fmtInt(b.number) }}</span>
          </td>
          <td>
            <div class="row-mid">
              <Chip>{{ b.extrinsic_count }} ext</Chip>
              <Chip>{{ b.event_count }} evt</Chip>
              <span class="mono text-xs dim">by {{ addrLabel(b.author, b.author_name) }}</span>
            </div>
          </td>
          <td class="right col-meta">
            <div class="mono text-xs dim"><TimeAgo :timestamp="b.timestamp_ms" /></div>
            <StatusPill :finalized="b.finalized" />
          </td>
        </tr>
      </tbody>
    </table>
    <SkeletonRows v-else :rows="5" :columns="['120px', '1fr', '160px']" />
  </div>
</template>

<style scoped lang="scss">
.live-dot-accent {
  color: var(--teal-500);
  margin-left: 4px;
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
  gap: 12px;
  align-items: center;
}

.col-meta {
  width: 160px;
}
</style>
