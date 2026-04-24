// Seed + live view of the ATS version feed for the active network.
// Same pattern as useLiveBlocks; see that file for the design notes.

import { storeToRefs } from 'pinia'
import type { AtsFeedItem, Page } from '@bindings'
import { useLiveStore } from '~/stores/live'

const emptyPage = (): Page<AtsFeedItem> => ({
  items: [],
  page_info: { total: null, next_cursor: null, has_more: false },
})

export interface UseLiveAtsFeedOptions {
  count?: number
}

export function useLiveAtsFeed(options: UseLiveAtsFeedOptions = {}) {
  const store = useLiveStore()
  const { atsFeed } = storeToRefs(store)

  const { pending, error } = useLiveSeed<Page<AtsFeedItem>, AtsFeedItem>({
    keyPrefix: 'live-ats-feed',
    path: '/ats/feed',
    count: options.count ?? 25,
    buffer: atsFeed,
    empty: emptyPage,
    extractItems: page => page?.items,
    seed: items => store.seedAtsFeed(items),
  })

  return { items: atsFeed, pending, error }
}
