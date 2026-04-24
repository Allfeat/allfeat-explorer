// Network-aware useFetch wrapper. Every data call in this app repeats the
// same six options — stable key across SSR→client, per-network URL
// templating, apiBaseUrl() selection, activeId reactivity, a default
// factory, and (for live feeds) a transform that seeds the Pinia store.
// This composable centralises that pattern so pages and live composables
// don't re-derive it by hand.
//
// Usage:
//   const { data, pending, error } = useNetworkFetch<Block[]>(
//     net => `live-blocks:${net}:n${count}`,
//     net => `/networks/${net}/blocks?count=${count}`,
//     { default: () => [], transform: items => (store.seedBlocks(items), items) },
//   )
//
// The `'unknown'` placeholder is passed when activeId is null. `immediate`
// is gated on activeId, so the request never actually fires in that state —
// the placeholder only exists to keep the URL / key typed as `string`.
//
// The wrapper narrows Nuxt's generic UseFetchOptions to the shape every
// network-scoped request actually needs (required `default`, optional
// `transform` and `watch`). The return `AsyncData<T, …>` hides the
// "data might be undefined" quirk useFetch normally produces when
// DefaultT isn't explicitly forwarded as a generic, because our contract
// says `default` always returns T.

import type { AsyncData, NuxtApp } from 'nuxt/app'
import type { WatchSource } from 'vue'

export interface UseNetworkFetchOptions<T> {
  /** Default value — keeps `data.value` typed as T instead of T | undefined. */
  default: () => T
  /** Optional post-process hook — runs on SSR and client. Used to seed Pinia stores. */
  transform?: (value: T) => T
  /** Extra reactive sources to watch beyond the active network id. */
  watch?: WatchSource[]
}

export function useNetworkFetch<T>(
  keyFor: (networkId: string) => string,
  pathFor: (networkId: string) => string,
  options: UseNetworkFetchOptions<T>,
): AsyncData<T, Error | null> {
  const { id: activeId } = useActiveNetwork()

  // The raw useFetch's generic chain (ResT → DataT → DefaultT) resists
  // being threaded through a wrapper generic without losing DefaultT
  // inference, so we cast the options to the internal shape and assert
  // the return type to what our contract guarantees (`data: Ref<T>`).
  const result = useFetch(
    () => pathFor(activeId.value ?? 'unknown'),
    {
      key: () => keyFor(activeId.value ?? 'unknown'),
      baseURL: apiBaseUrl(),
      immediate: activeId.value !== null,
      watch: [activeId, ...(options.watch ?? [])],
      default: options.default,
      transform: options.transform,
      // Only reuse the SSR payload during the initial hydration pass.
      // On client-side navigation back to a page, refetch — otherwise
      // list/overview data stays frozen at whatever it was on first load.
      getCachedData: (key: string, nuxtApp: NuxtApp) =>
        nuxtApp.isHydrating ? nuxtApp.payload.data[key] : undefined,
    } as unknown as Parameters<typeof useFetch<T>>[1],
  )

  return result as unknown as AsyncData<T, Error | null>
}
