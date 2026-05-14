// Network-aware link + navigation helper. Consolidates the
// `activeId.value ? { network: activeId.value } : {}` pattern that every
// page previously re-derived inline for <NuxtLink :to> and navigateTo()
// call sites, plus the per-entity `<section>/<id><queryString>` href
// builders that were re-declared across 7+ files.
//
// Returns:
//   - query / queryString:  reactive route-query bits
//   - to(path):             RouteLocationRaw builder for <NuxtLink :to>
//   - blockHref/accountHref/extrinsicHref/atsHref/envelopeHref: string
//     hrefs for `<a href>` and `<NuxtLink to>` call sites that prefer a
//     flat string over an object.
//
// The `network` key is only present when a non-null network id is active;
// pages on the default network therefore stay on clean URLs.

import { computed, type ComputedRef } from 'vue'
import type { RouteLocationRaw } from 'vue-router'

type EntityId = string | number | bigint

export interface UseNetworkLink {
  query: ComputedRef<{ network?: string }>
  queryString: ComputedRef<string>
  to: (path: string, extraQuery?: Record<string, string>) => RouteLocationRaw
  blockHref: (n: EntityId) => string
  accountHref: (addr: string) => string
  extrinsicHref: (id: string) => string
  atsHref: (id: EntityId) => string
  envelopeHref: (id: EntityId) => string
}

export function useNetworkLink(): UseNetworkLink {
  const { id: activeId } = useActiveNetwork()

  const query = computed<{ network?: string }>(() =>
    activeId.value ? { network: activeId.value } : {},
  )

  const queryString = computed<string>(() =>
    activeId.value ? `?network=${activeId.value}` : '',
  )

  function to(path: string, extraQuery?: Record<string, string>): RouteLocationRaw {
    return {
      path,
      query: extraQuery ? { ...query.value, ...extraQuery } : query.value,
    }
  }

  const href = (section: string) => (id: EntityId) =>
    `/${section}/${String(id)}${queryString.value}`

  return {
    query,
    queryString,
    to,
    blockHref: href('blocks'),
    accountHref: href('accounts'),
    extrinsicHref: href('extrinsics'),
    atsHref: href('ats'),
    envelopeHref: href('token/envelopes'),
  }
}
