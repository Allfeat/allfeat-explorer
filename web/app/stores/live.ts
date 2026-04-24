// Live buffers — blocks / transfers / ats feed — populated by the
// WebSocket plugin on the client and by useFetch seeds on SSR. Each
// buffer is newest-first and capped; inserts dedupe on identity so a
// push over an already-present item (e.g. a re-org or seed overlap)
// replaces the older row in place.
//
// Ordering rules match the mockup / API:
//   - blocks:    by numeric block number, BigInt-compared so u64 is safe
//   - transfers: by extrinsic id (<block>-<index>), inserted at head
//   - atsFeed:   by push order, inserted at head (composite key ats_id+version)
//
// The store is intentionally dumb: no fetch logic lives here. Composables
// own the seed path; the socket plugin owns the push path.

import { defineStore } from 'pinia'
import type { AtsFeedItem, Block, BlockEvent, Extrinsic, Transfer, WaveformBlock } from '@bindings'
import type { ConnState } from '~/types/live'

const CAP = 25
// Hero waveform window — 72 bars matches the maquette and gives roughly
// 7 minutes of history at 6s block time. Kept as a separate buffer so
// the dashboard rows stay tight and the waveform seed payload stays
// small (lean projection over /waveform, see useLiveWaveform).
const WAVEFORM_CAP = 72

interface LiveState {
  blocks: Block[]
  waveformBlocks: WaveformBlock[]
  transfers: Transfer[]
  extrinsics: Extrinsic[]
  events: BlockEvent[]
  atsFeed: AtsFeedItem[]
  connection: ConnState
}

function cmpBigint(a: string, b: string): number {
  const ba = BigInt(a)
  const bb = BigInt(b)
  if (ba === bb) return 0
  return ba > bb ? 1 : -1
}

function atsKey(a: AtsFeedItem): string {
  return `${a.ats_id}:${a.version_index}`
}

export const useLiveStore = defineStore('live', {
  state: (): LiveState => ({
    blocks: [],
    waveformBlocks: [],
    transfers: [],
    extrinsics: [],
    events: [],
    atsFeed: [],
    // Initial SSR value: 'connecting' is honest — the client hasn't had
    // a chance to open the socket yet. Flips to 'connected' on WS open.
    connection: 'connecting',
  }),

  getters: {
    // Highest block seen so far. `null` when the buffer is empty (SSR
    // before any seed, or just after clearLive on a network switch).
    head(state): string | null {
      return state.blocks[0]?.number ?? null
    },
    // Highest *finalized* block. Mock + chain both emit blocks with
    // `finalized: true` for heads already in the canonical chain.
    finalizedHead(state): string | null {
      return state.blocks.find(b => b.finalized)?.number ?? null
    },
  },

  actions: {
    seedBlocks(items: Block[]) {
      const sorted = [...items].sort((a, b) => cmpBigint(b.number, a.number))
      this.blocks = dedupeBy(sorted, b => b.number).slice(0, CAP)
    },
    pushBlock(b: Block) {
      // Re-orgs: replace any existing block with the same number so we
      // don't end up with two rows for the same height.
      const filtered = this.blocks.filter(x => x.number !== b.number)
      filtered.push(b)
      filtered.sort((x, y) => cmpBigint(y.number, x.number))
      this.blocks = filtered.slice(0, CAP)
      // Mirror into the waveform buffer so the hero advances on every
      // live push — same source of truth, just a leaner shape and a
      // larger window.
      this._upsertWaveform(blockToWaveform(b))
    },

    seedWaveform(items: WaveformBlock[]) {
      const sorted = [...items].sort((a, b) => cmpBigint(b.number, a.number))
      this.waveformBlocks = dedupeBy(sorted, w => w.number).slice(0, WAVEFORM_CAP)
    },
    _upsertWaveform(w: WaveformBlock) {
      const filtered = this.waveformBlocks.filter(x => x.number !== w.number)
      filtered.push(w)
      filtered.sort((x, y) => cmpBigint(y.number, x.number))
      this.waveformBlocks = filtered.slice(0, WAVEFORM_CAP)
    },

    seedTransfers(items: Transfer[]) {
      this.transfers = dedupeBy(items, t => t.extrinsic.id).slice(0, CAP)
    },
    pushTransfer(t: Transfer) {
      // Head insertion; dedup keeps the fresh frame, drops the older
      // seed row if it was already present.
      const filtered = this.transfers.filter(x => x.extrinsic.id !== t.extrinsic.id)
      this.transfers = [t, ...filtered].slice(0, CAP)
      // Mirror the underlying extrinsic so the extrinsics/events panel
      // stays in sync with every transfer that lands. Non-transfer
      // extrinsics come in via the HTTP re-seed on head-block change.
      this._pushExtrinsic(t.extrinsic)
    },

    seedExtrinsics(items: Extrinsic[]) {
      const sorted = [...items].sort(cmpExtrinsicsDesc)
      this.extrinsics = dedupeBy(sorted, e => e.id).slice(0, CAP)
    },
    _pushExtrinsic(e: Extrinsic) {
      const filtered = this.extrinsics.filter(x => x.id !== e.id)
      filtered.push(e)
      filtered.sort(cmpExtrinsicsDesc)
      this.extrinsics = filtered.slice(0, CAP)
    },

    seedEvents(items: BlockEvent[]) {
      const sorted = [...items].sort(cmpEventsDesc)
      this.events = dedupeBy(sorted, eventKey).slice(0, CAP)
    },

    seedAtsFeed(items: AtsFeedItem[]) {
      this.atsFeed = dedupeBy(items, atsKey).slice(0, CAP)
    },
    pushAtsItem(a: AtsFeedItem) {
      const key = atsKey(a)
      const filtered = this.atsFeed.filter(x => atsKey(x) !== key)
      this.atsFeed = [a, ...filtered].slice(0, CAP)
    },

    // Called on deliberate network switch so the buffers don't mix
    // frames from two chains.
    clearLive() {
      this.blocks = []
      this.waveformBlocks = []
      this.transfers = []
      this.extrinsics = []
      this.events = []
      this.atsFeed = []
    },

    setConnection(state: ConnState) {
      this.connection = state
    },
  },
})

// Orders extrinsics newest-first by (block_number desc, index desc).
// BigInt compare on block_number so u64 heights are safe.
function cmpExtrinsicsDesc(a: Extrinsic, b: Extrinsic): number {
  const bn = cmpBigint(b.block_number, a.block_number)
  if (bn !== 0) return bn
  return b.index - a.index
}

// Dedup key for events: `<block>-<index>` uniquely identifies one row.
function eventKey(e: BlockEvent): string {
  return `${e.block_number}-${e.index}`
}

// Newest-first by (block_number desc, index desc).
function cmpEventsDesc(a: BlockEvent, b: BlockEvent): number {
  const bn = cmpBigint(b.block_number, a.block_number)
  if (bn !== 0) return bn
  return b.index - a.index
}

function blockToWaveform(b: Block): WaveformBlock {
  return {
    number: b.number,
    extrinsic_count: b.extrinsic_count,
    event_count: b.event_count,
    ref_time_pct: b.ref_time_pct,
    finalized: b.finalized,
    timestamp_ms: b.timestamp_ms,
  }
}

function dedupeBy<T>(items: T[], key: (x: T) => string | number): T[] {
  const seen = new Set<string | number>()
  const out: T[] = []
  for (const it of items) {
    const k = key(it)
    if (seen.has(k)) continue
    seen.add(k)
    out.push(it)
  }
  return out
}
