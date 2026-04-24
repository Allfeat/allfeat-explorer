// Seed + live view of the latest extrinsics for the active network.
// Same seed pattern as useLiveBlocks; see that file for the design notes.
// The backend has no `extrinsics` WS topic, so we re-seed via HTTP
// whenever a new head block is pushed into the live store — cheap
// enough for a dashboard (one small payload per block, ~6 s cadence) and
// keeps the feed honest for non-transfer extrinsics that don't flow
// through the transfers topic. Transfer-type extrinsics stay fresh in
// the store via the WS transfer push (see live store's pushTransfer).

import { storeToRefs } from 'pinia'
import type { Extrinsic, Page } from '@bindings'
import { useLiveStore } from '~/stores/live'

const emptyPage = (): Page<Extrinsic> => ({
  items: [],
  page_info: { total: null, next_cursor: null, has_more: false },
})

export interface UseLiveExtrinsicsOptions {
  count?: number
}

export function useLiveExtrinsics(options: UseLiveExtrinsicsOptions = {}) {
  const store = useLiveStore()
  const { extrinsics } = storeToRefs(store)
  const head = computed<string | null>(() => store.head)

  const { pending, error } = useLiveSeed<Page<Extrinsic>, Extrinsic>({
    keyPrefix: 'live-extrinsics',
    path: '/extrinsics',
    count: options.count ?? 25,
    buffer: extrinsics,
    empty: emptyPage,
    extractItems: page => page?.items,
    seed: items => store.seedExtrinsics(items),
    watch: [head],
  })

  return { extrinsics, pending, error }
}
