<script setup lang="ts">
// Runtime overview. Renders the currently-active runtime (or a
// historical snapshot when `?at=N` is set) across four blocks:
// hero + WASM card, toolbar + "deployed X ago" line, 6-tile stats
// strip, identity + upgrade timeline panels.
//
// Every field on the page is fed by one of three endpoints:
//   * `useActiveNetwork().spec`     — NetworkSpec (token, SS58, …)
//   * `useRuntimeDetails(atParam)`  — RuntimeDetails (identity + WASM)
//   * `useRuntimeUpgrades()`        — RuntimeUpgrade[] (history)
// `at` flows through the URL (`?at=N`) so a historical view survives a
// refresh / deep-link.

import type { RuntimeUpgrade } from '@bindings'
import { fmtBytesCompact, fmtInt, fmtSpecVersionDotted, shortHash } from '~/utils/format'
import { timeAgo } from '~/utils/time'

definePageMeta({ name: 'runtime-overview' })
useSeoMeta({
  title: 'Runtime · Allfeat Explorer',
  description: 'Spec version, identity, and upgrade history of the active runtime.',
  ogType: 'website',
})

const route = useRoute()

// `?at=N` → the historical-block snapshot. Clamp to a non-negative
// integer so a garbage value in the query string doesn't break the
// fetch key (`NaN` would otherwise rehash every render).
const atParam = computed<number | null>(() => {
  const raw = route.query.at
  const n = typeof raw === 'string' ? Number(raw) : Number.NaN
  return Number.isFinite(n) && n >= 0 ? Math.floor(n) : null
})

const { id: activeId, spec } = useActiveNetwork()
const { runtime } = useRuntimeDetails(atParam)
const { upgrades } = useRuntimeUpgrades()
const { blockHref } = useNetworkLink()
const now = useWallClock()

// `.wasm` / raw-metadata download URLs. The HREFs embed the
// `apiBaseUrl()` prefix so they work identically in dev (Nuxt proxy →
// backend) and prod (same-origin). `download` on the <a> hints the
// browser to save rather than navigate; the server responds with
// `Content-Disposition: attachment` too for defensive-measure
// duplication. Both URLs are omitted when the active network isn't
// resolved yet — a naked `/api/v1/networks//runtime/wasm` would 404.
const apiBase = apiBaseUrl()
const wasmDownloadHref = computed(() => {
  const id = activeId.value
  if (!id) return null
  const base = `${apiBase}/networks/${id}/runtime/wasm`
  return atParam.value != null ? `${base}?at=${atParam.value}` : base
})
const wasmDownloadFilename = computed(() => {
  const id = activeId.value ?? 'runtime'
  const v = runtime.value?.identity.spec_version
  return v != null ? `${id}-spec-${v}.wasm` : `${id}-runtime.wasm`
})
const metadataDownloadHref = computed(() => {
  const id = activeId.value
  return id ? `${apiBase}/networks/${id}/runtime/metadata` : null
})
const metadataDownloadFilename = computed(() => {
  const id = activeId.value ?? 'runtime'
  return `${id}-metadata.scale`
})

const breadcrumb = [
  { label: 'Home', to: '/' },
  { label: 'Runtime' },
]

const PLACEHOLDER = '—'

// Every supported Allfeat / Melodie runtime pins token decimals at 12
// (Substrate convention for AFT). The value is stable enough that we
// keep it as a front-end constant rather than threading it through the
// network spec.
const TOKEN_DECIMALS = 12

const identity = computed(() => runtime.value?.identity ?? null)
const code = computed(() => runtime.value?.code ?? null)

const specTitle = computed(
  () => identity.value?.spec_name ?? spec.value?.spec_name ?? 'runtime',
)
const specVersion = computed(() => identity.value?.spec_version ?? spec.value?.spec_version)
const specVersionDotted = computed(() => fmtSpecVersionDotted(specVersion.value, PLACEHOLDER))
const specVersionInt = computed(() =>
  specVersion.value == null ? PLACEHOLDER : fmtInt(specVersion.value),
)

const networkKindLabel = computed(() => {
  const s = spec.value
  if (!s) return null
  return s.testnet ? 'Testnet' : 'Live'
})

// Aura + GRANDPA is the standard pair for every Substrate chain this
// deployment targets; the cadence comes from Timestamp::MinimumPeriod
// (see RpcClient::block_time_secs).
const consensusBlockTime = computed(() => {
  const s = spec.value
  return s ? `${s.block_time_secs}s blocks` : PLACEHOLDER
})

// ── WASM card ─────────────────────────────────────────────────────────
// `size_bytes` is the raw blob length read off `:code` (zstd-wrapped
// when the runtime is compressed). The card shows both the
// human-readable "X.YZ MB" and the exact byte count, mirroring the
// mock — the two forms together make it obvious whether the blob is
// the compressed wrapper or the decompressed WASM.
const wasmSizeCompact = computed(() => fmtBytesCompact(code.value?.size_bytes, PLACEHOLDER))
const wasmSizeExact = computed(() =>
  code.value ? `${fmtInt(code.value.size_bytes)} bytes` : PLACEHOLDER,
)
const wasmHash = computed(() => code.value?.hash ?? null)

// ── Identity panel helpers ────────────────────────────────────────────

const implNameLabel = computed(() => identity.value?.impl_name ?? PLACEHOLDER)

// ── Stats strip subtitle copy ─────────────────────────────────────────

// Reuse the RuntimeIdentity shape from the upgrade-history entry
// immediately below the current one so the subtitle can say
// "was 1,001,003" instead of a bland descriptor.
const previousSpecVersion = computed<number | null>(() => {
  const list = upgrades.value
  if (!list || list.length < 2) return null
  const currentIdx = list.findIndex((u: RuntimeUpgrade) => u.is_current)
  if (currentIdx < 0) return null
  const prev = list[currentIdx + 1]
  return prev?.spec_version ?? null
})
const specSubtitle = computed(() => {
  const prev = previousSpecVersion.value
  return prev != null ? `was ${fmtInt(prev)}` : 'sp_version · u32'
})

// `state_version` encodes the trie-node layout — 0 = in-place (legacy),
// 1 = value-split (blake2 redirect). The subtitle mirrors the mockup
// so readers who know Substrate recognise the distinction without a
// glossary lookup; anything beyond the two well-known values falls back
// to the raw number.
const stateVersionLabel = computed(() => {
  const v = identity.value?.state_version
  if (v == null) return PLACEHOLDER
  return String(v)
})
const stateVersionSubtitle = computed(() => {
  const v = identity.value?.state_version
  if (v == null) return 'not exposed'
  if (v === 0) return 'standard trie'
  if (v === 1) return 'blake2 trie'
  return `state v${v}`
})

const metadataVerLabel = computed(() =>
  runtime.value ? `V${runtime.value.metadata_version}` : PLACEHOLDER,
)

// ── Deployment context (toolbar right-hand-side) ──────────────────────
//
// "Deployed X ago · block #N" is sourced from the upgrade-history row
// flagged `is_current`. `first_block = 0` on the RPC fallback is a
// sentinel meaning "we don't have a deployment block for the active
// runtime" — the block link is hidden in that case.

const currentUpgrade = computed<RuntimeUpgrade | null>(
  () => upgrades.value?.find((u: RuntimeUpgrade) => u.is_current) ?? null,
)
// `first_block` is `string | null` on the wire:
//   * `null` → backend couldn't determine a deployment block (RPC-only
//     fallback, or indexed path without a matching spec_version row).
//   * `"0"`  → deployed at genesis (legit block number, not a sentinel).
//   * other  → real upgrade block.
// Parsing happens in one place so the toolbar / hero / timeline agree.
const currentBlock = computed<number | null>(() => {
  const u = currentUpgrade.value
  if (!u || u.first_block == null) return null
  const n = Number(u.first_block)
  return Number.isFinite(n) && n >= 0 ? n : null
})
const deploymentUnknown = computed(() => currentBlock.value == null)
const deploymentBlockLabel = computed(() => {
  const n = currentBlock.value
  if (n == null) return null
  return n === 0 ? '#0 (genesis)' : `#${fmtInt(n)}`
})
const deploymentBlockHref = computed(() => {
  const n = currentBlock.value
  return n != null ? blockHref(n) : null
})
const deploymentAgo = computed(() => {
  const c = currentUpgrade.value
  const n = currentBlock.value
  // Genesis has no `timestamp.set` inherent, so the DB records `0` for
  // the row's timestamp — the backend flips that to `null` and we
  // surface "genesis" here rather than "56 years ago".
  if (!c) return null
  if (n === 0 || c.first_block_timestamp_ms == null) return 'genesis'
  return timeAgo(c.first_block_timestamp_ms, now.value)
})
</script>

<template>
  <section class="container rt-page">
    <Breadcrumb :items="breadcrumb" />

    <!-- HERO + WASM CARD -->
    <section class="rt-hero">
      <div>
        <div class="rt-eyebrow">
          <span>RUNTIME · WASM BLOB</span>
          <span class="sep">·</span>
          <span>METADATA {{ metadataVerLabel }}</span>
          <span class="sep">·</span>
          <span class="rt-eyebrow__active">
            <LiveDot :size="8" />
            ACTIVE
          </span>
        </div>

        <h1 class="rt-title">
          <span>{{ specTitle }}<span class="rt-title__suffix">-runtime</span></span>
          <span class="rt-title__v">spec {{ specVersionDotted }}</span>
        </h1>

        <p class="rt-lede">
          The authoring runtime currently producing blocks on
          <span class="mono" style="color: var(--ink)">
            {{ spec?.name ?? PLACEHOLDER }}<span v-if="networkKindLabel"> · {{ networkKindLabel }}</span>
          </span>.
          <template v-if="deploymentBlockHref">
            Deployed at block
            <NuxtLink class="hash" :to="deploymentBlockHref">{{ deploymentBlockLabel }}</NuxtLink>
            via <span class="mono" style="color: var(--ink)">system.set_code</span>.
          </template>
        </p>
      </div>

      <aside class="rt-wasm">
        <div class="glyph" aria-hidden="true">
          <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="4" y="4" width="16" height="16" rx="1.5" />
            <rect x="8" y="8" width="8" height="8" rx="0.5" />
            <path d="M10 2v2M14 2v2M10 20v2M14 20v2M2 10h2M2 14h2M20 10h2M20 14h2" />
          </svg>
        </div>
        <div class="rt-wasm__label">Compact WASM</div>
        <div class="rt-wasm__size">
          {{ wasmSizeCompact }}
          <span class="mono dim rt-wasm__size-exact">· {{ wasmSizeExact }}</span>
        </div>
        <div class="rt-wasm__kv">
          <div class="k">Blake2</div>
          <div class="v">
            <template v-if="wasmHash">
              <Hash :text="wasmHash" :head="8" :tail="6" dim />
            </template>
            <template v-else>{{ PLACEHOLDER }}</template>
          </div>
        </div>
        <div class="rt-wasm__bar"><i /></div>
        <!-- Two raw-blob downloads: the WASM itself (`:code`, varies with
             the at=N snapshot) and the compile-time SCALE metadata the
             same runtime decodes against. Plain <a> with `download` so
             the browser saves the bytes instead of trying to render
             them; the URLs point at dedicated backend endpoints that
             stream from the RPC client (WASM) or the bundled artifact
             (metadata). -->
        <div class="rt-wasm__actions">
          <a
            v-if="wasmDownloadHref"
            class="btn sm"
            :href="wasmDownloadHref"
            :download="wasmDownloadFilename"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12" />
              <polyline points="7 10 12 15 17 10" />
              <path d="M5 21h14" />
            </svg>
            Download .wasm
          </a>
          <a
            v-if="metadataDownloadHref"
            class="btn sm ghost"
            :href="metadataDownloadHref"
            :download="metadataDownloadFilename"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="8 18 2 12 8 6" />
              <polyline points="16 6 22 12 16 18" />
            </svg>
            Raw metadata
          </a>
        </div>
      </aside>
    </section>

    <RuntimeAtToolbar
      :at-param="atParam"
      :deployment-ago="deploymentAgo"
      :deployment-block-href="deploymentBlockHref"
      :deployment-block-label="deploymentBlockLabel"
      :deployment-unknown="deploymentUnknown"
    />

    <!-- STATS STRIP -->
    <div class="rt-stats">
      <div class="stat rt-stat">
        <div class="label">Spec version</div>
        <div class="value rt-stat__value--accent">{{ specVersionInt }}</div>
        <div class="sub">{{ specSubtitle }}</div>
      </div>
      <div class="stat rt-stat">
        <div class="label">Impl version</div>
        <div class="value">{{ identity ? fmtInt(identity.impl_version) : PLACEHOLDER }}</div>
        <div class="sub">semantics unchanged</div>
      </div>
      <div class="stat rt-stat">
        <div class="label">Authoring ver</div>
        <div class="value">{{ identity ? fmtInt(identity.authoring_version) : PLACEHOLDER }}</div>
        <div class="sub">block-author compat</div>
      </div>
      <div class="stat rt-stat">
        <div class="label">Tx version</div>
        <div class="value">{{ identity ? fmtInt(identity.transaction_version) : PLACEHOLDER }}</div>
        <div class="sub">signed-ext id</div>
      </div>
      <div class="stat rt-stat">
        <div class="label">State version</div>
        <div class="value">{{ stateVersionLabel }}</div>
        <div class="sub">{{ stateVersionSubtitle }}</div>
      </div>
      <div class="stat rt-stat">
        <div class="label">Metadata ver</div>
        <div class="value">{{ metadataVerLabel }}</div>
        <div class="sub">RFC-78 · typed</div>
      </div>
    </div>

    <!-- IDENTITY + UPGRADE HISTORY -->
    <div class="sec-head">
      <h2>Runtime info</h2>
      <span class="tag">spec · identity · upgrades</span>
      <div class="divider" />
    </div>

    <div class="rt-col-2">
      <div class="panel">
        <div class="panel-head">
          <h3>Identity</h3>
          <span class="tag">sp_version::RuntimeVersion</span>
        </div>
        <Kv>
          <KvRow label="Spec name">
            <span class="mono">{{ identity?.spec_name ?? PLACEHOLDER }}</span>
          </KvRow>
          <KvRow label="Impl name">
            <span class="mono">{{ implNameLabel }}</span>
          </KvRow>
          <KvRow label="Chain">
            <span>{{ spec?.name ?? PLACEHOLDER }}</span>
            <Chip v-if="networkKindLabel" style="margin-left: 8px">
              {{ networkKindLabel }}
            </Chip>
          </KvRow>
          <KvRow label="Token">
            <span class="mono">{{ spec?.token ?? PLACEHOLDER }}</span>
            <span v-if="spec" class="dim">
              · {{ TOKEN_DECIMALS }} decimals · SS58 {{ spec.ss58_prefix }}
            </span>
          </KvRow>
          <KvRow label="Consensus">
            <span class="mono">Aura</span>
            <span class="dim"> + </span>
            <span class="mono">GRANDPA</span>
            <span class="dim"> · {{ consensusBlockTime }}</span>
          </KvRow>
          <KvRow label="Genesis hash">
            <template v-if="runtime">
              <Hash :text="runtime.genesis_hash" :head="8" :tail="6" />
            </template>
            <template v-else>
              <span class="mono dim">{{ PLACEHOLDER }}</span>
            </template>
          </KvRow>
          <KvRow label="Runtime hash">
            <template v-if="wasmHash">
              <Hash :text="wasmHash" :head="8" :tail="6" />
            </template>
            <template v-else>
              <span class="mono dim">{{ PLACEHOLDER }}</span>
            </template>
          </KvRow>
        </Kv>
      </div>

      <RuntimeUpgradeTimeline :upgrades="upgrades ?? []" :spec="spec" :now="now" />
    </div>

    <div style="height: 60px" />
  </section>
</template>

<style scoped lang="scss">
.rt-page {
  padding-bottom: 64px;
}

// ---------- HERO + WASM ----------
.rt-hero {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 32px;
  padding: 44px 0 36px;
  border-bottom: 1px solid var(--line);
}

.rt-eyebrow {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: var(--ink-dimmer);
  display: inline-flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 18px;

  .sep {
    opacity: 0.4;
  }

  &__active {
    color: var(--teal-500);
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
}

[data-theme="light"] .rt-eyebrow__active {
  color: var(--teal-700);
}

.rt-title {
  font-family: var(--font-display);
  font-size: 56px;
  letter-spacing: -0.035em;
  line-height: 0.98;
  font-weight: 700;
  margin-bottom: 20px;
  display: flex;
  align-items: baseline;
  gap: 14px;
  flex-wrap: wrap;

  &__suffix {
    color: var(--ink-dimmer);
    font-weight: 500;
  }

  &__v {
    color: var(--teal-500);
    font-family: var(--font-mono);
    font-size: 40px;
    letter-spacing: -0.02em;
    font-weight: 500;
  }
}

[data-theme="light"] .rt-title__v {
  color: var(--teal-700);
}

.rt-lede {
  font-size: 15px;
  color: var(--ink-dim);
  max-width: 620px;
  line-height: 1.6;
  text-wrap: pretty;
}

// WASM card sitting to the right of the hero copy.
.rt-wasm {
  border: 1px solid var(--line-2);
  padding: 22px 22px;
  min-width: 280px;
  position: relative;
  background:
    linear-gradient(135deg, rgba(0, 177, 140, 0.04) 0%, transparent 55%),
    var(--bg-1);
  display: flex;
  flex-direction: column;
  gap: 10px;
  align-self: start;

  .glyph {
    position: absolute;
    right: 18px;
    top: 18px;
    width: 40px;
    height: 40px;
    display: grid;
    place-items: center;
    color: var(--teal-500);
    opacity: 0.4;
  }

  &__label {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--ink-dimmer);
  }

  &__size {
    font-family: var(--font-display);
    font-size: 20px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }

  &__size-exact {
    font-size: 13px;
    font-weight: 400;
  }

  &__kv {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 14px;
    font-size: 12px;
    margin-top: 4px;

    .k {
      color: var(--ink-dimmer);
      font-family: var(--font-mono);
      font-size: 10.5px;
      letter-spacing: 0.06em;
      text-transform: uppercase;
    }

    .v {
      font-family: var(--font-mono);
      color: var(--ink-dim);
      word-break: break-all;
    }
  }

  &__bar {
    height: 3px;
    background: var(--chip-bg);
    margin-top: 8px;
    position: relative;
    overflow: hidden;

    > i {
      position: absolute;
      inset: 0;
      background: repeating-linear-gradient(
        90deg,
        var(--teal-500) 0 6px,
        transparent 6px 10px
      );
      opacity: 0.5;
    }
  }

  // Download / Raw-metadata buttons sit below the dashed bar.
  &__actions {
    display: flex;
    gap: 6px;
    margin-top: 10px;
  }
}

[data-theme="light"] .rt-wasm {
  .glyph {
    color: var(--teal-700);
  }

  &__bar > i {
    background: repeating-linear-gradient(
      90deg,
      var(--teal-700) 0 6px,
      transparent 6px 10px
    );
    opacity: 0.45;
  }
}

// ---------- STATS STRIP ----------
.rt-stats {
  display: grid;
  grid-template-columns: repeat(6, 1fr);
  border: 1px solid var(--line);
  background: var(--bg-1);
  margin-top: 32px;
}

.rt-stat {
  padding: 22px 22px;
  gap: 6px;
  border-right: 1px solid var(--line);

  &:last-child {
    border-right: 0;
  }

  .value {
    font-size: 28px;
  }

  &__value--accent {
    color: var(--teal-500);
  }
}

[data-theme="light"] .rt-stat__value--accent {
  color: var(--teal-700);
}

// ---------- SECTION HEADER ----------
.sec-head {
  display: flex;
  align-items: baseline;
  gap: 14px;
  padding: 44px 0 16px;

  h2 {
    font-size: 22px;
    letter-spacing: -0.02em;
    font-weight: 700;
  }

  .tag {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-dimmer);
  }

  .divider {
    flex: 1;
    height: 1px;
    background: var(--line);
    align-self: center;
  }
}

.rt-col-2 {
  display: grid;
  grid-template-columns: 1.1fr 0.9fr;
  gap: 24px;
}

// ---------- RESPONSIVE ----------
@media (max-width: 1100px) {
  .rt-hero {
    grid-template-columns: 1fr;
  }
  .rt-wasm {
    min-width: 0;
  }
  .rt-stats {
    grid-template-columns: repeat(3, 1fr);
  }
  .rt-stat:nth-child(3) {
    border-right: 0;
  }
  .rt-col-2 {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 640px) {
  .rt-hero {
    padding: 28px 0 24px;
  }
  .rt-title {
    font-size: 36px;
    gap: 8px;

    &__v {
      font-size: 24px;
    }
  }
  .rt-stats {
    grid-template-columns: repeat(2, 1fr);
  }
  .rt-stat:nth-child(2n) {
    border-right: 0;
  }
}
</style>
