<script setup lang="ts">
// Extrinsics table. Each row opens the detail page; the block cell
// jumps to the block instead (stop-propagated) so users can drill
// sideways without double-navigating.
//
// Transfer extrinsics get a special "from → to + amount" rendering in
// the signer cell so users can see both parties at a glance instead of
// having to open the detail page.

import type { Extrinsic, ExtrinsicArgs } from '@bindings'

defineProps<{
  extrinsics: readonly Extrinsic[]
}>()

const { to } = useNetworkLink()
const { spec } = useActiveNetwork()
const tokenSymbol = computed(() => spec.value?.token ?? '')

function openExtrinsic(id: string) {
  navigateTo(to(`/extrinsics/${id}`))
}

function openBlock(number: string, event: Event) {
  event.stopPropagation()
  navigateTo(to(`/blocks/${number}`))
}

function decodedField(args: ExtrinsicArgs, name: string): string | null {
  if (!('Decoded' in args)) return null
  const hit = args.Decoded.fields.find(f => f.name === name)
  return hit ? hit.value : null
}

// A `Balances.transfer*` call carries `dest` (SS58) and `value` (planck
// as decimal string) fields in the runtime metadata — detect the call
// by (module, call) rather than by variant tag so the shape tracks the
// on-chain names directly.
function transferOf(ext: Extrinsic): { dest: string, value: string } | null {
  if (ext.module !== 'Balances' && ext.module !== 'balances') return null
  if (!ext.call.startsWith('transfer')) return null
  const dest = decodedField(ext.args, 'dest')
  const value = decodedField(ext.args, 'value')
  return dest && value ? { dest, value } : null
}
</script>

<template>
  <table class="table">
    <thead>
      <tr>
        <th>Extrinsic ID</th>
        <th>Block</th>
        <th>Hash</th>
        <th>Call</th>
        <th>From / To</th>
        <th class="right">Fee</th>
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
        <td><span class="hash extrinsic-id">{{ e.id }}</span></td>
        <td data-label="Block">
          <button
            type="button"
            class="hash link-button"
            :aria-label="`Open block ${e.block_number}`"
            @click="openBlock(e.block_number, $event)"
            @keydown.enter.space.prevent="openBlock(e.block_number, $event)"
          >
            #{{ fmtInt(e.block_number) }}
          </button>
        </td>
        <td data-label="Hash"><Hash :text="e.hash" :head="8" :tail="6" /></td>
        <td data-label="Call">
          <Chip variant="module">{{ e.module }}</Chip>
          <Chip variant="call" class="call-chip">{{ e.call }}</Chip>
        </td>
        <td data-label="From / To">
          <div v-if="e.signer && transferOf(e)" class="transfer-flow">
            <Addr :text="e.signer" />
            <span class="transfer-arrow">
              <span aria-hidden="true">↓</span>
              <span class="transfer-amount mono">{{ fmtAFT(transferOf(e)!.value, 12, 6) }} {{ tokenSymbol }}</span>
            </span>
            <Addr :text="transferOf(e)!.dest" />
          </div>
          <Addr v-else-if="e.signer" :text="e.signer" />
          <span v-else class="dim text-xs">—</span>
        </td>
        <td data-label="Fee" class="right mono text-xs">{{ fmtAFT(e.fee, 12, 6) }} {{ tokenSymbol }}</td>
        <td data-label="Result"><StatusPill :result="e.result === 'Success' ? 'success' : 'failed'" /></td>
        <td data-label="Time" class="right">
          <span class="mono text-xs dim"><TimeAgo :timestamp="e.timestamp_ms" /></span>
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

.extrinsic-id {
  font-weight: 600;
}

.call-chip {
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

.transfer-flow {
  display: inline-flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  padding: 2px 0;
}

.transfer-arrow {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-left: 6px;
  color: var(--ink-dim);
  font-size: 11px;
  line-height: 1;

  & > span:first-child {
    color: var(--teal-500);
    font-weight: 700;
  }
}

.transfer-amount {
  font-size: 11px;
  letter-spacing: 0.01em;
  padding: 2px 6px;
  border-radius: 999px;
  background: rgba(0, 200, 180, 0.08);
  color: var(--teal-500, #2cd6c0);
  border: 1px solid rgba(0, 200, 180, 0.18);
}
</style>
