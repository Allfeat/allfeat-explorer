// Seed the networks store at SSR boot. Uses apiFetch so the call always
// resolves to the absolute backend origin (relative URLs during SSR
// bounce through Nitro's internal dispatcher and recurse into Vue
// Router). The payload is replayed to the client via Nuxt's state
// hydration — no client-side refetch.

import type { NetworkSpec } from '@bindings'
import { useNetworksStore } from '~/stores/networks'
import { apiFetch } from '~/composables/useApi'

interface NetworksResponse {
  networks: NetworkSpec[]
}

export default defineNuxtPlugin(async () => {
  const store = useNetworksStore()
  if (store.loaded) return

  try {
    const res = await apiFetch<NetworksResponse>('/networks')
    store.setItems(res.networks)
  }
  catch (err) {
    // If the backend is down at boot we surface an empty list; pages
    // that need a network will 404 cleanly via useActiveNetwork().
    console.error('[networks.server] failed to load /networks', err)
    store.setItems([])
  }
})
