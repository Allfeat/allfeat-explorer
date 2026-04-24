// Runtime identity snapshot for the active network. Fed by
// `/networks/{id}/runtime[?at=N]`. `at` is reactive — flipping the
// toolbar from "Latest" to a historical block re-triggers the fetch
// under a distinct useFetch key so the two responses don't collide
// in the cache. The endpoint returns a small object (no lists) so we
// don't need pagination infrastructure; a single useFetch call is
// enough.

import type { Ref } from 'vue'
import type { RuntimeDetails } from '@bindings'

interface RuntimeResponse {
  runtime: RuntimeDetails
}

export interface RuntimeDetailsResult {
  runtime: Ref<RuntimeDetails | null>
  pending: Ref<boolean>
  error: Ref<Error | null>
  refresh: () => Promise<void>
}

/**
 * Load runtime details for the active network. Pass `at` to pin the
 * snapshot at a specific block number — useful for the "At block…"
 * toolbar. Absent (or `null`) means "latest finalized".
 */
export function useRuntimeDetails(
  at?: Ref<number | null | undefined>,
): RuntimeDetailsResult {
  const { data, pending, error, refresh } = useNetworkFetch<RuntimeResponse | null>(
    net => `runtime-details:${net}:${at?.value ?? 'latest'}`,
    net => {
      const base = `/networks/${net}/runtime`
      const v = at?.value
      return typeof v === 'number' ? `${base}?at=${v}` : base
    },
    {
      default: () => null,
      watch: at ? [at] : [],
    },
  )

  const runtime = computed<RuntimeDetails | null>(() => data.value?.runtime ?? null)

  return {
    runtime,
    pending,
    error: error as Ref<Error | null>,
    refresh: () => refresh(),
  }
}
