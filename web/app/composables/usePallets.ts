// Pallet catalogue for the active network. Fed by
// `/networks/{id}/metadata/pallets`, which reads the static runtime
// metadata on the server. The list is stable per network, so it's
// fetched once per route visit and reused across components on the
// page (the underlying useFetch key is deduped by Nuxt).

import type { Ref } from 'vue'

interface PalletsResponse {
  pallets: string[]
}

export interface Pallets {
  pallets: Ref<string[]>
  pending: Ref<boolean>
  error: Ref<Error | null>
}

export function usePallets(): Pallets {
  const { data, pending, error } = useNetworkFetch<PalletsResponse>(
    net => `metadata-pallets:${net}`,
    net => `/networks/${net}/metadata/pallets`,
    { default: () => ({ pallets: [] }) },
  )

  const pallets = computed(() => data.value.pallets)

  return {
    pallets: pallets as Ref<string[]>,
    pending,
    error: error as Ref<Error | null>,
  }
}
