<script setup lang="ts">
// Dashboard panel — latest ATS timestamps. Purely presentational;
// the parent seeds the store (useLiveAtsFeed) and passes the slice.

import type { AtsFeedItem } from '@bindings'

defineProps<{
  items: readonly AtsFeedItem[]
}>()

const { queryString, to } = useNetworkLink()
const addrLabel = useAddrLabel()

function atsRowLabel(a: AtsFeedItem): string {
  if (a.version_count <= 1) return 'registration'
  if (a.is_initial) return 'initial registration'
  return a.is_latest ? 'new version' : 'revision'
}
</script>

<template>
  <div class="panel">
    <div class="panel-head">
      <h3>Latest ATS</h3>
      <span class="tag">ats.register · ats.add_version</span>
      <div class="spacer" />
      <NuxtLink :to="`/ats${queryString}`" class="ui-label">View all →</NuxtLink>
    </div>

    <div v-if="items.length > 0" class="ats-list">
      <NuxtLink
        v-for="a in items.slice(0, 6)"
        :key="`${a.ats_id}-${a.version_index}`"
        :to="to(`/ats/${a.ats_id}`)"
        class="ats-list-row row-fade-in"
      >
        <div class="ats-list-main">
          <AtsWave :commitment="a.commitment" :bars="18" :height="22" />
          <div class="ats-list-text">
            <div class="ats-list-top">
              <span class="hash ats-list-hash">{{ shortHash(a.commitment, 8, 8) }}</span>
              <span
                v-if="a.version_count > 1"
                class="version-badge"
                :class="{ latest: a.is_latest }"
              >
                v{{ a.version_index + 1 }}/{{ a.version_count }}
              </span>
            </div>
            <div class="mono text-xs dim ats-list-sub">
              {{ atsRowLabel(a) }} · by {{ addrLabel(a.signer, null, 5, 5) }} · #{{ fmtInt(a.block_number) }}
            </div>
          </div>
        </div>
        <span class="mono text-xs dim ats-list-time">
          <TimeAgo :timestamp="a.timestamp_ms" />
        </span>
      </NuxtLink>
    </div>
    <SkeletonRows v-else :rows="6" :columns="['1fr', '100px']" />
  </div>
</template>

<style scoped lang="scss">
.ats-list {
  padding: 4px 0;
}

.ats-list-row {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 16px;
  padding: 12px 22px;
  border-bottom: 1px solid var(--line);
  transition: background 0.2s ease;
  color: inherit;
  text-decoration: none;

  &:last-child {
    border-bottom: none;
  }

  &:hover,
  &:focus-visible {
    background: var(--hover);
  }

  &:focus-visible {
    outline: 2px solid var(--teal-500);
    outline-offset: -2px;
  }
}

.ats-list-main {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

.ats-list-text {
  min-width: 0;
  flex: 1;
}

.ats-list-top {
  display: flex;
  align-items: center;
  gap: 6px;
}

.ats-list-hash {
  font-size: 13px;
  font-weight: 600;
}

.ats-list-sub {
  margin-top: 2px;
}

.ats-list-time {
  white-space: nowrap;
  align-self: center;
}

.version-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 7px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--ink-dim);
  background: var(--chip-bg);
  border: 1px solid var(--chip-bd);

  &.latest {
    color: var(--teal-500);
    background: rgba(0, 177, 140, 0.1);
    border-color: rgba(0, 177, 140, 0.2);
  }
}

@media (max-width: 480px) {
  .ats-list-row {
    padding: 12px 16px;
    gap: 10px;
  }

  .ats-list-main {
    gap: 10px;
  }

  // Wave can eat the whole width before the hash on very narrow screens;
  // hide it and let the commitment hash + meta carry the visual identity.
  .ats-list-main > :first-child { display: none; }

  .ats-list-hash { font-size: 12px; }
  .ats-list-time { font-size: 10.5px; }
}
</style>
