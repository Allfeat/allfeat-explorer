// Active network resolver. Reads `?network=<id>` from the current route
// and cross-checks it against the networks store. Falls back to the
// store's default when the query is absent or points at an unknown id.
//
// Returns a Ref so callers react automatically to route / store changes.

import { computed, type ComputedRef } from 'vue'
import type { NetworkSpec } from '@bindings'
import { useNetworksStore } from '~/stores/networks'

export interface ActiveNetwork {
  id: ComputedRef<string | null>
  spec: ComputedRef<NetworkSpec | null>
  isKnown: ComputedRef<boolean>
  isMainnet: ComputedRef<boolean>
}

export function useActiveNetwork(): ActiveNetwork {
  const route = useRoute()
  const store = useNetworksStore()

  const id = computed<string | null>(() => {
    const raw = route.query.network
    const requested = typeof raw === 'string' ? raw : null
    if (requested && store.byId(requested)) return requested
    return store.defaultId
  })

  const spec = computed<NetworkSpec | null>(() => {
    const current = id.value
    return current ? store.byId(current) ?? null : null
  })

  const isKnown = computed(() => spec.value !== null)
  const isMainnet = computed(() => !(spec.value?.testnet ?? false))

  return { id, spec, isKnown, isMainnet }
}
