<script setup lang="ts">
// ATS detail → Versions tab. Lists every registered version newest-first
// and lets the user pick one — clicking a row drives the selected
// version back through `?v=<1-based>` and hops to the Overview tab so
// the picked version's full record is visible immediately.

import type { AtsRecord, AtsVersion } from '@bindings'
import { fmtInt, shortHash } from '~/utils/format'

const props = defineProps<{
  record: AtsRecord
  selectedIndex: number
}>()

const route = useRoute()
const { query: netQuery, extrinsicHref } = useNetworkLink()
const addrLabel = useAddrLabel()

const versionCount = computed(() => props.record.version_count)
// Newest version on top — the default-selected row for first-time viewers.
const orderedVersions = computed(() => [...props.record.versions].reverse())

function versionChipClass(v: AtsVersion): string {
  const i = v.version_index
  if (i === versionCount.value - 1 && versionCount.value > 1) return 'chip module'
  return 'chip'
}

function versionChipLabel(v: AtsVersion): string {
  const i = v.version_index
  if (i === 0) return 'initial'
  if (i === versionCount.value - 1 && versionCount.value > 1) return 'latest'
  return 'revision'
}

async function selectVersion(v: AtsVersion) {
  const query = {
    ...netQuery.value,
    tab: 'overview',
    v: String(v.version_index + 1),
  }
  await navigateTo({ query }, { replace: true })
}
</script>

<template>
  <div class="panel">
    <div class="panel-head">
      <h3>Version history</h3>
      <span class="tag">
        {{ versionCount }} {{ versionCount > 1 ? 'versions' : 'version' }} · same registry ID
      </span>
    </div>
    <div class="version-list">
      <div
        v-for="v in orderedVersions"
        :key="v.version_index"
        class="version-row"
        :class="{ selected: v.version_index === selectedIndex }"
        @click="selectVersion(v)"
      >
        <div class="version-row__tag">
          <span class="version-row__num">v{{ v.version_index + 1 }}</span>
          <span v-if="v.version_index === versionCount - 1" class="version-row__dot" />
        </div>
        <AtsWave
          :commitment="v.commitment"
          :bars="18"
          :height="18"
          :opacity="v.version_index === selectedIndex ? 1 : 0.55"
        />
        <div style="min-width: 0;">
          <div class="hash version-row__hash">{{ shortHash(v.commitment, 10, 8) }}</div>
          <div class="mono text-xs dim version-row__sub">
            by {{ addrLabel(v.signer, null, 5, 5) }} ·
            #{{ fmtInt(v.block_number) }} ·
            <TimeAgo :timestamp="v.created_at_ms" />
          </div>
        </div>
        <span :class="versionChipClass(v)" style="font-size: 10px;">
          {{ versionChipLabel(v) }}
        </span>
        <NuxtLink
          class="hash version-row__ext"
          :to="extrinsicHref(v.extrinsic_id)"
          @click.stop
        >
          {{ v.extrinsic_id }}
        </NuxtLink>
      </div>
    </div>
  </div>
</template>

<style scoped>
.version-list {
  padding: 4px 0;
}

.version-row {
  display: grid;
  grid-template-columns: 56px 90px 1fr auto auto;
  gap: 16px;
  padding: 14px 22px;
  align-items: center;
  cursor: pointer;
  border-bottom: 1px solid var(--line);
  border-left: 2px solid transparent;
  transition: background 0.15s ease, border-color 0.15s ease;
}

.version-row:last-child {
  border-bottom: none;
}

.version-row:hover {
  background: var(--hover);
}

.version-row.selected {
  background: var(--hover);
  border-left-color: var(--teal-500);
}

.version-row__tag {
  display: flex;
  align-items: center;
  gap: 6px;
}

.version-row__num {
  font-family: var(--font-mono);
  font-size: 13px;
  font-weight: 700;
  color: var(--ink);
}

.version-row.selected .version-row__num {
  color: var(--teal-500);
}

.version-row__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--teal-500);
}

.version-row__hash {
  font-size: 12px;
  font-weight: 600;
}

.version-row__sub {
  margin-top: 2px;
}

.version-row__ext {
  font-size: 11px;
}

@media (max-width: 760px) {
  .version-row {
    grid-template-columns: 52px 1fr auto;
  }

  .version-row > :nth-child(2),
  .version-row__ext {
    display: none;
  }
}

@media (max-width: 640px) {
  .version-row {
    padding: 12px 14px;
    gap: 10px;
  }
}
</style>
