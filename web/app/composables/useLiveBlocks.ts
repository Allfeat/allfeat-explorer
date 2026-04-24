// Seed + live view of the latest blocks for the active network.
//
// Seeds the shared live store with an HTTP snapshot and returns the
// store's reactive slice; WebSocket pushes keep the buffer fresh after
// mount via the live plugin. Switching network re-fetches automatically
// because useNetworkFetch watches activeId.
//
// Store seeding uses useFetch's `transform` option, which fires on both
// SSR and client as soon as the handler resolves — unlike `watch`, which
// doesn't run during Vue SSR. This guarantees the Pinia store is
// populated before Nuxt serialises state into the payload, so the
// rendered HTML ships with real rows and client hydration picks them up.

import { storeToRefs } from 'pinia'
import type { Block, Page } from '@bindings'
import { useLiveStore } from '~/stores/live'

const emptyPage = (): Page<Block> => ({
  items: [],
  page_info: { total: null, next_cursor: null, has_more: false },
})

export interface UseLiveBlocksOptions {
  /** Initial seed size. Defaults to 25 (matches store cap). */
  count?: number
}

export function useLiveBlocks(options: UseLiveBlocksOptions = {}) {
  const store = useLiveStore()
  const { blocks } = storeToRefs(store)

  // `/blocks` returns a `Page<Block>` envelope since the pagination
  // migration — unwrap `items` for the seed. We don't care about
  // `page_info` here: this composable only needs the newest window to
  // paint the dashboard hero; the ongoing live push rides the
  // WebSocket and doesn't touch this endpoint.
  const { pending, error } = useLiveSeed<Page<Block>, Block>({
    keyPrefix: 'live-blocks',
    path: '/blocks',
    count: options.count ?? 25,
    buffer: blocks,
    empty: emptyPage,
    extractItems: page => page?.items,
    seed: items => store.seedBlocks(items),
  })

  return { blocks, pending, error }
}
