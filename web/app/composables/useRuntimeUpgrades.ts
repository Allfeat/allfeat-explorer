// Runtime-upgrade timeline for the active network. Fed by
// `/networks/{id}/runtime/upgrades`. The response is one row per
// distinct `spec_version` the indexer has observed, newest-first, with
// the currently-active spec flagged `is_current = true`.
//
// Indexed networks walk the `blocks` history for the aggregate; the
// RPC-only fallback returns a single "current" row with `first_block =
// 0` as a sentinel meaning "deployment block unknown". The page reads
// that sentinel to render the row without a block-link affordance
// rather than linking to block zero.

import type { Ref } from 'vue'
import type { RuntimeUpgrade } from '@bindings'

interface UpgradesResponse {
  upgrades: RuntimeUpgrade[]
}

export interface RuntimeUpgradesResult {
  upgrades: Ref<RuntimeUpgrade[]>
  pending: Ref<boolean>
  error: Ref<Error | null>
}

export function useRuntimeUpgrades(): RuntimeUpgradesResult {
  const { data, pending, error } = useNetworkFetch<UpgradesResponse>(
    net => `runtime-upgrades:${net}`,
    net => `/networks/${net}/runtime/upgrades`,
    { default: () => ({ upgrades: [] }) },
  )

  const upgrades = computed(() => data.value.upgrades)

  return {
    upgrades: upgrades as Ref<RuntimeUpgrade[]>,
    pending,
    error: error as Ref<Error | null>,
  }
}
