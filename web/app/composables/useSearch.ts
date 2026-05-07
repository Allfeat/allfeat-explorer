// Navbar search engine.
//
// The backend has no /search endpoint — every entity is reached via an
// exact-id GET. This composable compensates by:
//
//   1. Classifying the input (see searchPatterns.ts) into the set of
//      endpoints that could possibly match.
//   2. Firing each candidate lookup in parallel with $fetch + AbortController.
//   3. Collapsing the resolved rows into a grouped result set keyed by kind.
//
// Supporting behaviour:
//
//   - Debounced queries (200 ms) so keystrokes don't fan out into loads
//     of backend traffic.
//   - A per-network LRU cache (query → grouped results) so re-typing the
//     same thing or arrow-ing through the dropdown twice is free. Failed
//     lookups cache as `null` so we don't retry the miss back-to-back.
//   - AbortController cancellation on every new query — stale responses
//     never replace fresh ones.
//   - Block-hash matches are resolved from the live Pinia buffer (recent
//     blocks only), because there's no by-hash endpoint; if the block
//     rolled out of the buffer, the hash search silently misses rather
//     than adding a misleading "not found" row.

import { computed, ref, shallowRef, watch } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import { useDebounceFn } from '@vueuse/core'
import type { Account, AtsRecord, Block, Extrinsic } from '@bindings'
import { detectKinds, type SearchKind } from '~/utils/searchPatterns'
import { useLiveStore } from '~/stores/live'

export interface SearchHit {
  kind: SearchKind
  /** Stable router target, used on click / enter. */
  to: string
  /** Primary line — monospace-friendly. */
  title: string
  /** Secondary line — free text (module, balance, owner …). */
  subtitle: string
}

export interface SearchResults {
  blocks: SearchHit[]
  extrinsics: SearchHit[]
  accounts: SearchHit[]
  ats: SearchHit[]
}

export interface UseSearchReturn {
  query: Ref<string>
  trimmed: ComputedRef<string>
  loading: Ref<boolean>
  results: Ref<SearchResults>
  total: ComputedRef<number>
  /** Ordered flat list, used by the dropdown for keyboard navigation. */
  flat: ComputedRef<SearchHit[]>
  /** Reset state — used on close / network swap. */
  reset: () => void
}

const EMPTY: SearchResults = { blocks: [], extrinsics: [], accounts: [], ats: [] }

// A "lookup failed / entity absent" cache sentinel. Keeping this distinct
// from `undefined` lets us differentiate "we haven't checked" (re-probe)
// from "we checked and got 404" (skip).
const MISS = Symbol('search-miss')
type Miss = typeof MISS

// Small LRU — plenty for a session's worth of typing. Caches the grouped
// result for each <network, query> pair. We key per network so a network
// swap doesn't show Melodie blocks while we're browsing Allfeat.
const CACHE_MAX = 128
const cache = new Map<string, SearchResults | Miss>()

function cacheGet(key: string): SearchResults | Miss | undefined {
  const v = cache.get(key)
  if (v === undefined) return undefined
  cache.delete(key)
  cache.set(key, v) // bump LRU recency
  return v
}

function cacheSet(key: string, value: SearchResults | Miss): void {
  if (cache.has(key)) cache.delete(key)
  cache.set(key, value)
  if (cache.size > CACHE_MAX) {
    const oldest = cache.keys().next().value
    if (oldest !== undefined) cache.delete(oldest)
  }
}

// Per-network endpoint paths. The `/api/v1` base is prepended by
// `apiBaseUrl()` from useApi.ts. Segments that echo user input go
// through `encodeURIComponent` as defence-in-depth — the pattern
// detector only lets in base58 / hex / digits today, but the endpoint
// is the last place we want an unescaped surprise.
const path = {
  block: (net: string, n: string) => `/networks/${net}/blocks/${n}`,
  extrinsic: (net: string, id: string) => `/networks/${net}/extrinsics/${encodeURIComponent(id)}`,
  account: (net: string, addr: string) => `/networks/${net}/accounts/${encodeURIComponent(addr)}`,
  ats: (net: string, id: string) => `/networks/${net}/ats/${id}`,
}

// $fetch wrapper — 404 resolves to null (we treat it as "absent"), other
// errors also collapse to null so a flaky request doesn't poison the
// dropdown. If we ever want to surface retryable errors distinctly this
// is the seam to change.
async function probe<T>(
  url: string,
  signal: AbortSignal,
): Promise<T | null> {
  try {
    return await $fetch<T>(url, { baseURL: apiBaseUrl(), signal })
  } catch (err) {
    if ((err as { name?: string })?.name === 'AbortError') throw err
    return null
  }
}

function buildBlockHit(b: Block): SearchHit {
  const extPart = b.extrinsic_count === 1 ? '1 extrinsic' : `${b.extrinsic_count} extrinsics`
  const evtPart = b.event_count === 1 ? '1 event' : `${b.event_count} events`
  return {
    kind: 'block-number',
    to: `/blocks/${b.number}`,
    title: `Block #${fmtInt(b.number)}`,
    subtitle: `${extPart} · ${evtPart} · ${shortHash(b.hash)}`,
  }
}

function buildBlockHashHit(b: Block): SearchHit {
  return {
    kind: 'block-hash',
    to: `/blocks/${b.number}`,
    title: `Block ${shortHash(b.hash)}`,
    subtitle: `#${fmtInt(b.number)} · ${b.extrinsic_count} extrinsics`,
  }
}

function buildExtrinsicHit(x: Extrinsic): SearchHit {
  const outcome = x.result === 'Success' ? 'success' : 'failed'
  return {
    kind: 'extrinsic-id',
    to: `/extrinsics/${x.id}`,
    title: `Extrinsic ${x.id}`,
    subtitle: `${x.module}.${x.call} · ${outcome}`,
  }
}

function buildAccountHit(a: Account, knownLabel: string | null, tokenSymbol: string): SearchHit {
  const title = knownLabel ?? shortAddr(a.address)
  const balance = `${fmtAFT(a.balance.total, 12, 2)} ${tokenSymbol}`
  return {
    kind: 'account',
    to: `/accounts/${a.address}`,
    title,
    subtitle: knownLabel ? `${shortAddr(a.address)} · ${balance}` : balance,
  }
}

// Known-account match with no backend confirmation: we have the label +
// kind from the local registry, but no balance data. Subtitle shows the
// truncated SS58 and the role kind so the row is meaningful on its own.
function buildKnownAccountHit(m: { address: string, name: string, kind: string }): SearchHit {
  return {
    kind: 'account',
    to: `/accounts/${m.address}`,
    title: m.name,
    subtitle: `${shortAddr(m.address)} · ${m.kind}`,
  }
}

// Fallback hit for a well-formed SS58 that the backend doesn't know
// about — typically an address with no on-chain activity yet. Every
// SS58 exists implicitly on a Substrate chain with a zero balance, so
// the user should still be able to jump to the detail page (which
// renders the inactive-account view when the lookup 404s).
function buildInactiveAccountHit(address: string, knownLabel: string | null): SearchHit {
  return {
    kind: 'account',
    to: `/accounts/${address}`,
    title: knownLabel ?? shortAddr(address),
    subtitle: 'No on-chain activity yet',
  }
}

function buildAtsHit(r: AtsRecord): SearchHit {
  return {
    kind: 'ats-id',
    to: `/ats/${r.ats_id}`,
    title: `ATS #${fmtInt(r.ats_id)}`,
    subtitle: `owner ${shortAddr(r.owner)} · ${r.version_count} version${r.version_count === 1 ? '' : 's'}`,
  }
}

export function useSearch(): UseSearchReturn {
  const { id: activeId, spec } = useActiveNetwork()
  const live = useLiveStore()

  const query = ref('')
  const trimmed = computed(() => query.value.trim())
  const loading = ref(false)
  const results = shallowRef<SearchResults>(EMPTY)

  let controller: AbortController | null = null

  function reset() {
    query.value = ''
    results.value = EMPTY
    loading.value = false
    controller?.abort()
    controller = null
  }

  // Hash search operates entirely off the Pinia live buffer — no backend
  // round-trip. Called synchronously so it's cheap to re-run on any input.
  function matchBlockHashLocal(hash: string): SearchHit | null {
    const b = live.blocks.find(x => x.hash.toLowerCase() === hash)
    return b ? buildBlockHashHit(b) : null
  }

  async function run(q: string, net: string): Promise<void> {
    const key = `${net}::${q}`

    const cached = cacheGet(key)
    if (cached !== undefined) {
      results.value = cached === MISS ? EMPTY : cached
      loading.value = false
      return
    }

    controller?.abort()
    controller = new AbortController()
    const signal = controller.signal
    loading.value = true

    const detected = detectKinds(q)
    const jobs: Promise<void>[] = []
    const acc: SearchResults = { blocks: [], extrinsics: [], accounts: [], ats: [] }

    // Local known-account search runs on *every* query (label, SS58
    // prefix, substring) — it doesn't depend on pattern detection. Cheap
    // in-memory scan against the per-network registry, so we always
    // surface matching labels even when the user hasn't finished typing
    // the address or types a natural-language name like "Treasury".
    const localAccounts = searchKnownAccounts(net, q, 8)
    const accountsSeen = new Set<string>()
    for (const m of localAccounts) {
      acc.accounts.push(buildKnownAccountHit(m))
      accountsSeen.add(m.address)
    }

    // Block # + ATS # share the same numeric input so we fan out and
    // collapse results into their own sections — both can hit.
    if (detected.kinds.has('block-number') && detected.asInt !== null) {
      const n = detected.asInt.toString()
      jobs.push(probe<Block>(path.block(net, n), signal).then(b => {
        if (b) acc.blocks.push(buildBlockHit(b))
      }))
    }
    if (detected.kinds.has('ats-id') && detected.asInt !== null) {
      const n = detected.asInt.toString()
      jobs.push(probe<AtsRecord>(path.ats(net, n), signal).then(r => {
        if (r) acc.ats.push(buildAtsHit(r))
      }))
    }
    if (detected.kinds.has('extrinsic-id') && detected.asExtrinsic) {
      const id = `${detected.asExtrinsic.block}-${detected.asExtrinsic.index}`
      jobs.push(probe<Extrinsic>(path.extrinsic(net, id), signal).then(x => {
        if (x) acc.extrinsics.push(buildExtrinsicHit(x))
      }))
    }
    // Full-address account probe — only runs when the local pass didn't
    // already produce a hit for the same SS58, so we avoid a duplicate
    // row (and a wasted round-trip for known validators / sudo / etc.).
    // When the backend 404s, we fall back to an "inactive" hit so any
    // valid SS58 is always reachable from the search bar, including
    // addresses that haven't transacted yet.
    if (detected.kinds.has('account') && detected.asAddress && !accountsSeen.has(detected.asAddress)) {
      const addr = detected.asAddress
      const knownLabel = lookupKnownAccount(net, addr)?.name ?? null
      const token = spec.value?.token ?? ''
      jobs.push(probe<Account>(path.account(net, addr), signal).then(a => {
        if (a) {
          acc.accounts.push(buildAccountHit(a, knownLabel, token))
        } else {
          acc.accounts.push(buildInactiveAccountHit(addr, knownLabel))
        }
      }))
    }
    if (detected.kinds.has('block-hash') && detected.asBlockHash) {
      const hit = matchBlockHashLocal(detected.asBlockHash)
      if (hit) acc.blocks.push(hit)
    }

    // No pattern hit and no local label hit? Cache the miss early and bail.
    if (jobs.length === 0 && acc.accounts.length === 0) {
      results.value = EMPTY
      cacheSet(key, MISS)
      loading.value = false
      return
    }

    try {
      await Promise.all(jobs)
    } catch (err) {
      if ((err as { name?: string })?.name === 'AbortError') return
      // Swallow — partial results are still useful.
    }

    // Guard against a stale run finishing after a newer one started.
    if (signal.aborted) return

    results.value = acc
    const empty = acc.blocks.length + acc.extrinsics.length + acc.accounts.length + acc.ats.length === 0
    cacheSet(key, empty ? MISS : acc)
    loading.value = false
  }

  const debouncedRun = useDebounceFn(run, 200)

  watch([trimmed, activeId], ([q, net]) => {
    if (!q) {
      results.value = EMPTY
      loading.value = false
      controller?.abort()
      return
    }
    if (!net) {
      results.value = EMPTY
      return
    }
    loading.value = true
    debouncedRun(q, net)
  })

  const flat = computed<SearchHit[]>(() => [
    ...results.value.blocks,
    ...results.value.extrinsics,
    ...results.value.accounts,
    ...results.value.ats,
  ])

  const total = computed(() => flat.value.length)

  return { query, trimmed, loading, results, total, flat, reset }
}
