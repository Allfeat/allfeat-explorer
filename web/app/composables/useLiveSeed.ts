// Generic seed + live view factory for the dashboard buffers. Every
// per-entity live composable (blocks, extrinsics, events, ats feed,
// waveform) shares the same shape:
//
//   1. Fetch `/networks/:net/<path>?count=N` via useNetworkFetch for a
//      stable SSR→client key and a per-network refetch.
//   2. On transform, hand the newly-arrived items to a store seed action
//      so the payload is available both during SSR (hydration-clean) and
//      after mount.
//   3. Return the store's reactive slice for the caller to render.
//
// Response shape is injected through `extractItems` so both `Page<T>`
// envelopes (blocks / extrinsics / events / ats) and plain arrays
// (waveform) share the same plumbing. See useLiveBlocks & siblings for
// thin wrappers — keeping those lets pages call `useLiveBlocks({ count })`
// without knowing anything about the factory.

import { type Ref, type WatchSource } from 'vue'

export interface UseLiveSeedConfig<TResp, TItem> {
  /** Prefix for the useFetch key, e.g. 'live-blocks'. */
  keyPrefix: string
  /** Resource path under `/networks/${net}`, e.g. '/blocks' or '/waveform'. */
  path: string
  /** Page size; appended as `?count=N`. */
  count: number
  /** Reactive newest-first buffer exposed to the caller. */
  buffer: Ref<TItem[]>
  /** Fetch-payload factory — used as useFetch's `default`. */
  empty: () => TResp
  /** Extract the items array from the fetch response. */
  extractItems: (resp: TResp) => TItem[] | null | undefined
  /** Store action that replaces the buffer with a fresh snapshot. */
  seed: (items: TItem[]) => void
  /** Extra reactive sources that should trigger a re-seed. */
  watch?: WatchSource[]
}

export interface UseLiveSeedResult<TItem> {
  items: Ref<TItem[]>
  pending: Ref<boolean>
  error: Ref<unknown>
}

export function useLiveSeed<TResp, TItem>(
  config: UseLiveSeedConfig<TResp, TItem>,
): UseLiveSeedResult<TItem> {
  const { pending, error } = useNetworkFetch<TResp>(
    net => `${config.keyPrefix}:${net}:n${config.count}`,
    net => `/networks/${net}${config.path}?count=${config.count}`,
    {
      default: config.empty,
      watch: config.watch,
      transform: (resp) => {
        const items = config.extractItems(resp)
        if (items && items.length > 0) config.seed(items)
        return resp
      },
    },
  )

  return { items: config.buffer, pending, error }
}
