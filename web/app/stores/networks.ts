// Catalogue of enabled networks, loaded once from /api/v1/networks.
//
// Populated by a Nuxt plugin on first SSR render so every page gets the
// list baked into its payload — avoids a client-side network flash on
// NetworkSwitch / active-network resolution.

import { defineStore } from 'pinia'
import type { NetworkSpec } from '@bindings'

interface NetworksState {
  items: NetworkSpec[]
  loaded: boolean
}

export const useNetworksStore = defineStore('networks', {
  state: (): NetworksState => ({
    items: [],
    loaded: false,
  }),

  getters: {
    // First network acts as the default when ?network= is absent or points
    // at an unknown id. The backend orders the list deterministically; we
    // don't re-sort here.
    defaultId(state): string | null {
      return state.items[0]?.id ?? null
    },

    byId(state): (id: string) => NetworkSpec | undefined {
      return (id: string) => state.items.find(n => n.id === id)
    },
  },

  actions: {
    setItems(items: NetworkSpec[]) {
      this.items = items
      this.loaded = true
    },
  },
})
