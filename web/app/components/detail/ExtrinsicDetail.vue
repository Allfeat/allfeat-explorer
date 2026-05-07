<script setup lang="ts">
// Extrinsic detail. Mirrors the maquette's ExtrinsicDetail: KV overview,
// side "Result" panel, and tabs for Parameters / Events / Raw.
//
// The ExtrinsicArgs union is externally-tagged on the wire — `Decoded`
// carries a list of `(name, type_name, value)` fields walked out of the
// runtime metadata, `Raw` falls through for anything the decoder
// couldn't resolve (unknown pallet/variant, truncated bytes). A couple
// of (pallet, call) pairs get a richer rendering (`Timestamp.set`
// formats the `now` field as a wall clock; `Balances.transfer*` labels
// the value in AFT) — everything else renders generically via the
// metadata `type_name`.

import type { CallField, Extrinsic, ExtrinsicArgs } from '@bindings'

const props = defineProps<{
  extrinsic: Extrinsic
}>()

const route = useRoute()
const { blockHref, accountHref } = useNetworkLink()
const { spec } = useActiveNetwork()
const tokenSymbol = computed(() => spec.value?.token ?? '')

function isDecoded(args: ExtrinsicArgs): args is { Decoded: { fields: CallField[] } } {
  return 'Decoded' in args
}

function isRaw(args: ExtrinsicArgs): args is { Raw: { hex: string } } {
  return 'Raw' in args
}

const decodedFields = computed<CallField[]>(() =>
  isDecoded(props.extrinsic.args) ? props.extrinsic.args.Decoded.fields : [],
)

const rawHex = computed<string | null>(() =>
  isRaw(props.extrinsic.args) ? props.extrinsic.args.Raw.hex : null,
)

// Pretty-print a well-known field when we can (`Timestamp.set.now`
// → wall-clock string; `Balances.transfer*.value` → AFT). The generic
// fallback just echoes the `scale_value` rendering the backend sent.
function renderFieldValue(field: CallField): { primary: string, secondary?: string } {
  const m = props.extrinsic.module
  const c = props.extrinsic.call
  if (m === 'Timestamp' && c === 'set' && field.name === 'now') {
    const ms = Number(field.value)
    if (Number.isFinite(ms)) return { primary: fmtUtcTime(ms), secondary: `${fmtInt(ms)} ms` }
  }
  if ((m === 'Balances' || m === 'balances') && c.startsWith('transfer') && field.name === 'value') {
    return { primary: `${fmtAFT(field.value, 12, 6)} ${tokenSymbol.value}`, secondary: `${fmtInt(field.value)} planck` }
  }
  return { primary: field.value }
}

function isAddressField(field: CallField): boolean {
  const m = props.extrinsic.module
  const c = props.extrinsic.call
  if ((m === 'Balances' || m === 'balances') && c.startsWith('transfer') && field.name === 'dest') return true
  const typeName = field.type_name ?? ''
  if (/AccountId|MultiAddress|LookupSource/.test(typeName)) return true
  return looksLikeAddress(field.value)
}

const activeTab = computed<string>(() => {
  const q = route.query.tab
  return typeof q === 'string' ? q : 'parameters'
})

const resultVariant = computed<'success' | 'failed'>(() =>
  props.extrinsic.result === 'Success' ? 'success' : 'failed',
)

const rawJson = computed<string>(() => JSON.stringify(props.extrinsic, null, 2))
</script>

<template>
  <div>
    <div class="page-title">
      <div>
        <h1 style="display: flex; align-items: center; gap: 14px; white-space: nowrap;">
          {{ extrinsic.id }}
          <StatusPill :result="resultVariant" />
        </h1>
      </div>
    </div>

    <div class="ex-grid">
      <div class="panel">
        <Kv>
          <KvRow label="Block">
            <NuxtLink class="hash" :to="blockHref(extrinsic.block_number)">
              #{{ fmtInt(extrinsic.block_number) }}
            </NuxtLink>
          </KvRow>
          <KvRow label="Timestamp">
            <span class="mono">{{ fmtUtcTime(extrinsic.timestamp_ms) }}</span>
          </KvRow>
          <KvRow label="Hash">
            <Hash :text="extrinsic.hash" :head="12" :tail="12" />
          </KvRow>
          <KvRow label="Call">
            <Chip variant="module">{{ extrinsic.module }}</Chip>
            <Chip variant="call" style="margin-left: 4px;">{{ extrinsic.call }}</Chip>
          </KvRow>
          <KvRow label="Signed">
            <span v-if="extrinsic.signed">Yes (Sr25519)</span>
            <span v-else class="dim">No (inherent)</span>
          </KvRow>
          <KvRow v-if="extrinsic.signer" label="Signer">
            <Addr :text="extrinsic.signer" :to="accountHref(extrinsic.signer)" />
          </KvRow>
          <KvRow v-if="extrinsic.signed && extrinsic.nonce !== null" label="Nonce">
            <span class="mono">{{ fmtInt(extrinsic.nonce ?? 0) }}</span>
          </KvRow>
          <KvRow label="Tip">
            <span class="mono">{{ fmtAFT(extrinsic.tip) }} {{ tokenSymbol }}</span>
          </KvRow>
          <KvRow label="Fee">
            <span class="mono">{{ fmtAFT(extrinsic.fee, 12, 9) }} {{ tokenSymbol }}</span>
            <span class="dim"> ({{ fmtInt(extrinsic.fee) }} planck)</span>
          </KvRow>
        </Kv>
      </div>

      <div class="panel" style="align-self: start;">
        <div class="panel-head">
          <h3>Result</h3>
        </div>
        <div style="padding: 20px;">
          <StatusPill :result="resultVariant" />
          <div style="margin-top: 12px; font-size: 13px; color: var(--ink-dim); line-height: 1.5;">
            <template v-if="extrinsic.result === 'Success'">
              Extrinsic applied successfully.
            </template>
            <template v-else>
              Execution reverted. See dispatch error for details.
            </template>
          </div>
          <div style="border-top: 1px solid var(--line); margin-top: 14px; padding-top: 14px;">
            <div class="ui-label" style="margin-bottom: 4px;">Events emitted</div>
            <div class="big-num">{{ extrinsic.events.length }}</div>
          </div>
        </div>
      </div>
    </div>

    <div class="panel" style="margin-top: 24px;">
      <Tabs
        :items="[
          { id: 'parameters', label: 'Parameters' },
          { id: 'events', label: 'Events', count: extrinsic.events.length },
          { id: 'raw', label: 'Raw' },
        ]"
      />

      <div v-if="activeTab === 'parameters'" style="padding: 20px;">
        <div v-if="decodedFields.length > 0" class="param-grid">
          <template v-for="(field, i) in decodedFields" :key="i">
            <div class="param-k">
              <div class="mono" style="font-size: 13px; font-weight: 600;">{{ field.name ?? `#${i}` }}</div>
              <div v-if="field.type_name" class="mono text-xs dim" style="margin-top: 2px;">
                {{ field.type_name }}
              </div>
            </div>
            <div class="param-v">
              <Addr v-if="isAddressField(field)" :text="field.value" :to="accountHref(field.value)" />
              <template v-else>
                <div class="mono">{{ renderFieldValue(field).primary }}</div>
                <div v-if="renderFieldValue(field).secondary" class="mono text-xs dim">
                  {{ renderFieldValue(field).secondary }}
                </div>
              </template>
            </div>
          </template>
        </div>

        <div v-else-if="rawHex !== null" class="param-grid">
          <div class="param-k">
            <div class="mono" style="font-size: 13px; font-weight: 600;">raw</div>
            <div class="mono text-xs dim" style="margin-top: 2px;">Bytes</div>
          </div>
          <div class="param-v">
            <div class="mono text-xs" style="word-break: break-all; color: var(--ink-dim);">
              {{ rawHex }}
            </div>
          </div>
        </div>

        <div v-else class="dim text-xs" style="padding: 8px;">
          Call takes no parameters.
        </div>
      </div>

      <table v-else-if="activeTab === 'events'" class="table">
        <thead>
          <tr>
            <th>Index</th>
            <th>Event</th>
            <th>Data</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(ev, i) in extrinsic.events" :key="i">
            <td><span class="hash">#{{ i }}</span></td>
            <td data-label="Event">
              <Chip variant="module">{{ ev.module }}</Chip>
              <Chip variant="call" style="margin-left: 4px;">{{ ev.name }}</Chip>
            </td>
            <td data-label="Data">
              <span v-if="ev.fields.length === 0" class="mono text-xs dim">—</span>
              <div v-else style="display: flex; flex-direction: column; gap: 2px;">
                <span
                  v-for="(f, j) in ev.fields"
                  :key="j"
                  class="mono text-xs"
                >
                  <span class="dim">{{ f.name ?? `#${j}` }}:</span>
                  <NuxtLink
                    v-if="looksLikeAddress(f.value)"
                    class="hash"
                    :to="accountHref(f.value)"
                  >{{ f.value }}</NuxtLink>
                  <template v-else>{{ f.value }}</template>
                </span>
              </div>
            </td>
          </tr>
          <tr v-if="extrinsic.events.length === 0">
            <td colspan="3" style="padding: 40px; text-align: center;" class="dim">
              No events emitted.
            </td>
          </tr>
        </tbody>
      </table>

      <pre v-else-if="activeTab === 'raw'" class="raw-json">{{ rawJson }}</pre>
    </div>
  </div>
</template>

<style scoped>
.ex-grid {
  display: grid;
  grid-template-columns: 1fr 320px;
  gap: 24px;
  margin-top: 24px;
}

@media (max-width: 900px) {
  .ex-grid {
    grid-template-columns: 1fr;
  }
}

.big-num {
  font-family: var(--font-display);
  font-size: 26px;
  font-weight: 700;
  letter-spacing: -0.02em;
}

.param-grid {
  display: grid;
  grid-template-columns: 180px 1fr;
  gap: 0;
}

.param-k {
  padding: 14px;
  border-bottom: 1px solid var(--line);
  border-right: 1px solid var(--line);
}

.param-v {
  padding: 14px;
  border-bottom: 1px solid var(--line);
}

.param-grid > .param-k:last-of-type,
.param-grid > .param-v:last-of-type {
  border-bottom: none;
}

.raw-json {
  padding: 20px;
  margin: 0;
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-dim);
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.6;
}
</style>
