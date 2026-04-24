<script setup lang="ts">
// ATS detail. Renders the registry record + a selectable version. Layout
// ports the reference maquette: subtitle + badge, commitment-as-H1, hero
// waveform strip, then three tabs (Overview / Versions / Technical).
//
// Selected version lives in `?v=<1-based>` so deep links survive reload;
// `?tab=<id>` drives the tab strip (handled by <Tabs/>). The per-tab
// bodies live in `components/detail/ats/` so each tab owns its own
// markup + scoped styles — this file only handles version selection and
// tab routing.

import type { AtsRecord, AtsVersion } from '@bindings'
import { fmtUtcTime } from '~/utils/format'

const props = defineProps<{
  record: AtsRecord
}>()

const route = useRoute()

const versionCount = computed(() => props.record.version_count)
const multiVersion = computed(() => versionCount.value > 1)

const selectedIndex = computed<number>(() => {
  const q = route.query.v
  if (typeof q === 'string') {
    const n = Number.parseInt(q, 10)
    if (Number.isFinite(n) && n >= 1 && n <= versionCount.value) {
      return n - 1
    }
  }
  return versionCount.value - 1
})

const selected = computed<AtsVersion>(
  () => props.record.versions[selectedIndex.value] ?? props.record.versions[props.record.versions.length - 1]!,
)

const isInitial = computed(() => selected.value.version_index === 0)
const isLatest = computed(() => selected.value.version_index === versionCount.value - 1)

const tabs = computed(() => {
  const out: { id: string, label: string, count?: number }[] = [
    { id: 'overview', label: 'Overview' },
  ]
  if (multiVersion.value) {
    out.push({ id: 'versions', label: 'Versions', count: versionCount.value })
  }
  out.push({ id: 'technical', label: 'Technical' })
  return out
})

const activeTab = computed<string>(() => {
  const q = route.query.tab
  if (typeof q === 'string' && tabs.value.some(t => t.id === q)) return q
  return 'overview'
})
</script>

<template>
  <div>
    <div class="page-title ats-head">
      <div style="min-width: 0;">
        <h1 class="commitment-h1 mono">{{ selected.commitment }}</h1>
        <div v-if="multiVersion" class="mono text-xs dim" style="margin-top: 8px;">
          Showing version
          <span style="color: var(--teal-500); font-weight: 600;">v{{ selected.version_index + 1 }}</span>
          of {{ versionCount }}
          · {{ isLatest ? 'latest' : isInitial ? 'initial' : 'historical' }}
        </div>
      </div>
    </div>

    <div class="panel ats-hero-strip">
      <div class="ats-hero-strip__inner">
        <AtsWave :commitment="selected.commitment" :bars="72" :height="40" />
        <div class="ats-hero-strip__spacer" />
        <div class="ats-hero-strip__meta">
          <div class="ui-label">Registered</div>
          <div class="ats-hero-strip__ts">{{ fmtUtcTime(selected.created_at_ms) }}</div>
          <div class="mono text-xs dim" style="margin-top: 2px;">
            <TimeAgo :timestamp="selected.created_at_ms" />
          </div>
        </div>
      </div>
    </div>

    <div style="margin-top: 24px;">
      <Tabs :items="tabs" />
    </div>

    <div style="margin-top: 16px;">
      <AtsOverviewTab
        v-if="activeTab === 'overview'"
        :record="record"
        :selected="selected"
      />
      <AtsVersionsTab
        v-else-if="activeTab === 'versions'"
        :record="record"
        :selected-index="selectedIndex"
      />
      <AtsTechnicalTab
        v-else-if="activeTab === 'technical'"
        :selected="selected"
      />
    </div>
  </div>
</template>

<style scoped>
.ats-head {
  padding-bottom: 20px;
}

.commitment-h1 {
  font-family: var(--font-mono);
  font-size: 22px;
  font-weight: 600;
  letter-spacing: -0.01em;
  word-break: break-all;
  max-width: 900px;
  margin-top: 8px;
  line-height: 1.25;
}

.ats-hero-strip {
  margin-top: 4px;
  padding: 22px 24px;
  background:
    linear-gradient(90deg, rgba(0, 177, 140, 0.08) 0%, transparent 70%),
    var(--bg-elev);
}

.ats-hero-strip__inner {
  display: flex;
  align-items: center;
  gap: 20px;
}

.ats-hero-strip__spacer {
  flex: 1;
}

.ats-hero-strip__meta {
  text-align: right;
}

.ats-hero-strip__ts {
  font-family: var(--font-display);
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.01em;
  margin-top: 4px;
}

@media (max-width: 960px) {
  .commitment-h1 {
    font-size: 16px;
  }
}

@media (max-width: 640px) {
  .commitment-h1 {
    font-size: 13px;
    margin-top: 6px;
  }

  .ats-hero-strip {
    padding: 16px 18px;
  }

  .ats-hero-strip__inner {
    flex-direction: column;
    align-items: stretch;
    gap: 14px;
  }

  .ats-hero-strip__spacer {
    display: none;
  }

  .ats-hero-strip__meta {
    text-align: left;
  }

  .ats-hero-strip__ts {
    font-size: 16px;
  }
}
</style>
