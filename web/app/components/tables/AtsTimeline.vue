<script setup lang="ts">
// Timeline view of the ATS version feed. Items are grouped by UTC day
// (YYYY-MM-DD) and rendered as cards stacked under a date marker on a
// shared vertical rail. Replaces the flat table view from Phase 10 —
// matches the maquette's ATSTimeline (see project/Allfeat Explorer.html).

import type { AtsFeedItem } from '@bindings'

const props = defineProps<{
  items: readonly AtsFeedItem[]
}>()

const { to } = useNetworkLink()
const addrLabel = useAddrLabel()

interface DateGroup {
  date: string
  items: readonly AtsFeedItem[]
}

const groups = computed<DateGroup[]>(() => {
  const buckets = new Map<string, AtsFeedItem[]>()
  for (const a of props.items) {
    // Use UTC day so grouping is stable across viewers — the backend
    // emits timestamps in ms since epoch and the maquette slices the ISO
    // string the same way.
    const key = new Date(a.timestamp_ms).toISOString().slice(0, 10)
    const bucket = buckets.get(key)
    if (bucket) bucket.push(a)
    else buckets.set(key, [a])
  }
  return [...buckets.entries()]
    .sort((a, b) => (a[0] < b[0] ? 1 : a[0] > b[0] ? -1 : 0))
    .map(([date, items]) => ({ date, items }))
})

function openAts(a: AtsFeedItem) {
  // Carry the version through so a v1 row doesn't silently land on the
  // default-latest view (the AtsDetail page falls back to versionCount-1
  // when `?v=` is absent). Without this, every click on a non-latest
  // version of the same ats_id sends the user to the latest version's
  // data — including its extrinsic link.
  navigateTo(to(`/ats/${a.ats_id}`, { v: String(a.version_index + 1) }))
}

function rowLabel(a: AtsFeedItem): string {
  if (a.version_count <= 1) return 'registered'
  return a.is_initial ? 'registered' : 'new version'
}

function fmtClock(ms: number): string {
  // 24h HH:MM:SS, locale-free so SSR and client agree byte-for-byte.
  const d = new Date(ms)
  const hh = String(d.getUTCHours()).padStart(2, '0')
  const mm = String(d.getUTCMinutes()).padStart(2, '0')
  const ss = String(d.getUTCSeconds()).padStart(2, '0')
  return `${hh}:${mm}:${ss}`
}
</script>

<template>
  <div class="ats-timeline">
    <div class="ats-timeline-rail" aria-hidden="true" />

    <div
      v-for="g in groups"
      :key="g.date"
      class="ats-timeline-group"
    >
      <div class="ats-timeline-head">
        <div class="ats-timeline-marker" aria-hidden="true" />
        <span class="ui-label ats-timeline-date">{{ g.date }}</span>
        <span class="mono text-xs dim">{{ g.items.length }} event{{ g.items.length > 1 ? 's' : '' }}</span>
      </div>

      <div class="ats-timeline-rows">
        <div
          v-for="a in g.items"
          :key="`${a.ats_id}-${a.version_index}`"
          class="panel interactive ats-timeline-row"
          role="link"
          tabindex="0"
          :aria-label="`Open ATS registration ${a.ats_id}`"
          @click="openAts(a)"
          @keydown.enter.space.prevent="openAts(a)"
        >
          <AtsWave :commitment="a.commitment" :bars="22" :height="22" />

          <div class="ats-timeline-text">
            <div class="ats-timeline-top">
              <span class="hash" style="font-size: 13px; font-weight: 600;">
                {{ shortHash(a.commitment, 12, 10) }}
              </span>
              <span v-if="a.version_count > 1" class="version-badge" :class="{ 'latest': a.is_latest }">
                v{{ a.version_index + 1 }}/{{ a.version_count }}
              </span>
            </div>
            <div class="mono text-xs dim ats-timeline-sub">
              {{ rowLabel(a) }} by {{ addrLabel(a.signer, null, 5, 5) }} · #{{ fmtInt(a.block_number) }} · ats#{{ a.ats_id }}
            </div>
          </div>

          <span class="chip ats-timeline-proto">v{{ a.protocol_version }}</span>
          <span class="mono text-xs dim ats-timeline-time">{{ fmtClock(a.timestamp_ms) }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ats-timeline {
  position: relative;
  margin-top: 20px;
}

.ats-timeline-rail {
  position: absolute;
  left: 12px;
  top: 8px;
  bottom: 8px;
  width: 1px;
  background: var(--line);
}

.ats-timeline-group {
  position: relative;
  margin-bottom: 24px;
}

.ats-timeline-group:last-child {
  margin-bottom: 0;
}

.ats-timeline-head {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 12px;
  padding-left: 32px;
}

.ats-timeline-marker {
  position: absolute;
  left: 6px;
  top: 6px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--bg);
  border: 2px solid var(--teal-500);
}

.ats-timeline-date {
  font-size: 12px;
  font-weight: 600;
  color: var(--ink);
}

.ats-timeline-rows {
  padding-left: 32px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.ats-timeline-row {
  padding: 14px 18px;
  display: grid;
  grid-template-columns: 120px 1fr auto auto;
  gap: 16px;
  align-items: center;
  cursor: pointer;
}

.ats-timeline-row:focus-visible {
  outline: 2px solid var(--teal-500);
  outline-offset: -2px;
}

.ats-timeline-text {
  min-width: 0;
}

.ats-timeline-top {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ats-timeline-sub {
  margin-top: 3px;
}

.ats-timeline-proto {
  font-family: var(--font-mono);
}

.ats-timeline-time {
  white-space: nowrap;
}

.version-badge {
  display: inline-flex;
  align-items: center;
  padding: 1px 6px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 9px;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--ink-dim);
  background: var(--chip-bg);
  border: 1px solid var(--chip-bd);
}

.version-badge.latest {
  color: var(--teal-500);
  background: rgba(0, 177, 140, 0.1);
  border-color: rgba(0, 177, 140, 0.2);
}

@media (max-width: 720px) {
  .ats-timeline-row {
    grid-template-columns: 72px 1fr auto;
  }

  .ats-timeline-time {
    display: none;
  }
}

@media (max-width: 480px) {
  .ats-timeline-rail {
    left: 8px;
  }

  .ats-timeline-head {
    padding-left: 24px;
    gap: 10px;
  }

  .ats-timeline-marker {
    left: 2px;
  }

  .ats-timeline-rows {
    padding-left: 24px;
  }

  .ats-timeline-row {
    grid-template-columns: 1fr auto;
    grid-template-rows: auto auto;
    gap: 6px 10px;
    padding: 12px 14px;
  }

  /* The wave becomes a full-width "preview strip" above the text so the
     hash + meta get a proper column instead of being squeezed by a fixed
     72px wave column on the left. */
  .ats-timeline-row > :first-child {
    grid-column: 1 / -1;
    order: -1;
    opacity: 0.55;
  }
}
</style>
