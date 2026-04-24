// Cursor-based pagination composable used by every list page
// (blocks, extrinsics, events, ats feed, …). Owns:
//
//   - URL cursor state via `?cursor=…` (bookmarkable, history-aware back)
//   - the `useAsyncData` fetch with `count` + `cursor` + filter query params
//   - derived computeds exposed to the page (items, pageInfo, total,
//     hasMore, nextCursor, canGoBack)
//   - Next/Prev navigation helpers that round-trip through the router
//
// The list-page pattern is always the same shape — same SSR-stable key,
// same newest-first cursor convention, same `Page<T>` envelope — so
// factoring it here keeps the pages down to their actual UI concerns.
//
// Usage:
//   const { items, pending, error, total, hasMore, canGoBack, goBack, goNext }
//     = usePaginatedList<Block>({
//       keyPrefix: 'blocks-list',
//       path: '/blocks',
//       pageSize: 25,
//       // Optional: server-side filters forwarded as extra query params
//       // and folded into the cache key. The caller is responsible for
//       // dropping `cursor` when filter values change.
//       filters: () => ({ signed: route.query.signed as string | null }),
//     })
//
// The fetch URL is `/networks/${net}${path}?count=…&cursor=…&<filters>`.
// The useAsyncData key is `${keyPrefix}:${net}:${cursor ?? 'head'}:${filters}`.

import type { ComputedRef, Ref } from 'vue'
import { toValue } from 'vue'
import type { Page, PageInfo } from '@bindings'

type FilterValue = string | null | undefined
type FilterMap = Record<string, FilterValue>
type FilterSource = FilterMap | (() => FilterMap) | Ref<FilterMap> | ComputedRef<FilterMap>

export interface UsePaginatedListOptions {
  /** Prefix for the useAsyncData key, e.g. 'blocks-list'. */
  keyPrefix: string
  /** Resource path under `/networks/${net}`, e.g. '/blocks' or '/ats/feed'. */
  path: string
  /** Page size. Defaults to 25. */
  pageSize?: number
  /**
   * Optional server-side filters. Keys with non-empty string values
   * are appended to the fetch query string and folded into the
   * useAsyncData cache key; null / undefined / empty strings are
   * skipped. Pass a getter or ref backed by the route query so filter
   * changes trigger a refetch; pages are responsible for also clearing
   * `cursor` when filter values change, since a cursor from an older
   * predicate won't align with the new filtered set.
   */
  filters?: FilterSource
}

export interface PaginatedList<T> {
  items: ComputedRef<T[]>
  pageInfo: ComputedRef<PageInfo | null>
  total: ComputedRef<string | null>
  hasMore: ComputedRef<boolean>
  nextCursor: ComputedRef<string | null>
  canGoBack: ComputedRef<boolean>
  cursor: ComputedRef<string | null>
  pending: Ref<boolean>
  error: Ref<Error | null>
  goBack: () => void
  goNext: () => void
  refresh: () => Promise<void>
}

export function usePaginatedList<T>(
  options: UsePaginatedListOptions,
): PaginatedList<T> {
  const { keyPrefix, path, pageSize = 25, filters } = options

  const route = useRoute()
  const router = useRouter()
  const { id: activeId } = useActiveNetwork()

  const cursor = computed<string | null>(() => {
    const q = route.query.cursor
    return typeof q === 'string' && q.length > 0 ? q : null
  })

  // Normalised filter view: drop empty / nullish entries and sort by
  // key so the cache signature is order-independent. A caller that
  // swaps filter order mustn't trigger a spurious refetch.
  const filterEntries = computed<[string, string][]>(() => {
    if (!filters) return []
    const raw = toValue(filters as FilterSource) ?? {}
    const out: [string, string][] = []
    for (const [k, v] of Object.entries(raw)) {
      if (typeof v === 'string' && v.length > 0) out.push([k, v])
    }
    out.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    return out
  })

  const filterSignature = computed<string>(() =>
    filterEntries.value.map(([k, v]) => `${k}=${v}`).join('&'),
  )

  const emptyPage = (): Page<T> => ({
    items: [],
    page_info: { total: null, next_cursor: null, has_more: false },
  })

  const { data, pending, error, refresh } = useAsyncData<Page<T>>(
    () =>
      `${keyPrefix}:${activeId.value ?? 'unknown'}:${cursor.value ?? 'head'}:${filterSignature.value}`,
    async () => {
      const net = activeId.value
      if (net === null) return emptyPage()
      const qs = new URLSearchParams({ count: String(pageSize) })
      if (cursor.value) qs.set('cursor', cursor.value)
      for (const [k, v] of filterEntries.value) qs.set(k, v)
      return await $fetch<Page<T>>(
        `/networks/${net}${path}?${qs.toString()}`,
        { baseURL: apiBaseUrl() },
      )
    },
    {
      watch: [activeId, cursor, filterSignature],
      immediate: activeId.value !== null,
      default: (): Page<T> => emptyPage(),
      // Only reuse the SSR payload during the initial hydration pass.
      // On client-side navigation back to this page, refetch — otherwise
      // list tables show stale rows after leaving and returning to the page.
      getCachedData: (key, nuxtApp) =>
        nuxtApp.isHydrating ? nuxtApp.payload.data[key] : undefined,
    },
  )

  const items = computed<T[]>(() => data.value?.items ?? [])
  const pageInfo = computed<PageInfo | null>(() => data.value?.page_info ?? null)
  const total = computed<string | null>(() => pageInfo.value?.total ?? null)
  const hasMore = computed<boolean>(() => pageInfo.value?.has_more ?? false)
  const nextCursor = computed<string | null>(() => pageInfo.value?.next_cursor ?? null)
  const canGoBack = computed<boolean>(() => cursor.value !== null)

  // History-aware previous: keeps the cursor chain bookmarkable without a
  // prev-cursor on the wire. When the user landed directly on a later
  // page (no history to go back to), fall back to the cursor-less head.
  function goBack() {
    if (import.meta.client && window.history.length > 1) {
      router.back()
    } else {
      navigateTo({ query: { ...route.query, cursor: undefined } })
    }
  }

  function goNext() {
    if (!nextCursor.value) return
    navigateTo({ query: { ...route.query, cursor: nextCursor.value } })
  }

  return {
    items,
    pageInfo,
    total,
    hasMore,
    nextCursor,
    canGoBack,
    cursor,
    pending,
    error: error as Ref<Error | null>,
    goBack,
    goNext,
    refresh: async () => {
      await refresh()
    },
  }
}
