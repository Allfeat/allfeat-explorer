// Resolve a single SS58 address against the active network's
// known-accounts registry. Reactive on both the active network and the
// supplied address — good for re-rendering `<Addr>` when the network
// switcher changes or the address prop updates.

import { computed, toValue, type ComputedRef, type MaybeRefOrGetter } from 'vue'
import { lookupKnownAccount, type KnownAccount } from '~/utils/knownAccounts'

export function useKnownAccount(
  address: MaybeRefOrGetter<string | null | undefined>,
): ComputedRef<KnownAccount | null> {
  const { id } = useActiveNetwork()
  return computed(() => lookupKnownAccount(id.value, toValue(address)))
}

/**
 * Returns a function suitable for rendering an address as a label in a
 * template: known-accounts name wins, otherwise an optional caller-supplied
 * fallback (e.g. block.author_name), otherwise a truncated SS58.
 *
 * Call once in `<script setup>` so the active-network lookup happens at
 * setup time; the returned callable is cheap to invoke inside loops.
 */
export function useAddrLabel() {
  const { id } = useActiveNetwork()
  return (
    address: string | null | undefined,
    fallback: string | null | undefined = null,
    head = 6,
    tail = 6,
  ): string => {
    if (!address) return ''
    const known = lookupKnownAccount(id.value, address)
    if (known) return known.name
    if (fallback) return fallback
    return shortAddr(address, head, tail)
  }
}
