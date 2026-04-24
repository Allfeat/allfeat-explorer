// Seed + live view of the latest events for the active network. Same
// pattern as useLiveExtrinsics: there's no WS topic for events, so we
// re-seed via HTTP whenever a new head block lands. One small payload
// per block is cheap enough for a dashboard tile, and walking the last
// N blocks on the backend surfaces Initialization/Finalization events
// (session changes, grandpa authority rotations, …) that wouldn't show
// up if we derived events from the extrinsics feed alone.

import { storeToRefs } from 'pinia'
import type { BlockEvent, Page } from '@bindings'
import { useLiveStore } from '~/stores/live'

const emptyPage = (): Page<BlockEvent> => ({
  items: [],
  page_info: { total: null, next_cursor: null, has_more: false },
})

export interface UseLiveEventsOptions {
  count?: number
}

export function useLiveEvents(options: UseLiveEventsOptions = {}) {
  const store = useLiveStore()
  const { events } = storeToRefs(store)
  const head = computed<string | null>(() => store.head)

  const { pending, error } = useLiveSeed<Page<BlockEvent>, BlockEvent>({
    keyPrefix: 'live-events',
    path: '/events',
    count: options.count ?? 25,
    buffer: events,
    empty: emptyPage,
    extractItems: page => page?.items,
    seed: items => store.seedEvents(items),
    watch: [head],
  })

  return { events, pending, error }
}
