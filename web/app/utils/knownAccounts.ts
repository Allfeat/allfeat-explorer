// Per-network registry of SS58 addresses that should render with a
// friendly name instead of the default truncated hash.
//
// Two sources feed the registry:
//
//   1. Hand-maintained entries (`STATIC_REGISTRY`) — real deployment
//      addresses (treasury, sudo key, validator set, well-known
//      holders). These take priority on collision.
//
//   2. Mock-derived entries (`buildMockRegistry`) — derived at module
//      load from the same LCG + role-seed constants the Rust mock
//      generator uses (see web/app/utils/mockSs58.ts and
//      src/mock/data.rs). They only match when the backend runs in
//      `mock` mode; in prod builds the addresses they compute simply
//      never appear on the wire, so the noise cost is zero.
//
// Resolution is O(1) via a per-network object, so this scales to a few
// thousand entries without measurable cost.

import {
  MOCK_VALIDATOR_POOL,
  mockSudo,
  mockTreasury,
  mockValidator,
} from '~/utils/mockSs58'

export type KnownAccountKind =
  | 'treasury'
  | 'sudo'
  | 'validator'
  | 'council'
  | 'team'
  | 'foundation'
  | 'exchange'
  | 'dev'
  | 'other'

export interface KnownAccount {
  name: string
  kind: KnownAccountKind
  /** Optional human-readable note surfaced on the account detail page. */
  note?: string
}

type NetworkRegistry = Record<string, KnownAccount>

// Well-known Substrate development accounts. Addresses are derived from
// the seeds `//Alice`, `//Bob`, ... under the generic SS58 prefix (42),
// which is what the backend's `AccountId32::to_string()` emits today.
// Shared across every network registered below — if a test chain ever
// funds `//Alice`, we want the label to show up regardless of network.
const DEV_ACCOUNTS: NetworkRegistry = {
  '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY': { name: 'Alice', kind: 'dev' },
  '5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty': { name: 'Bob', kind: 'dev' },
  '5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y': { name: 'Charlie', kind: 'dev' },
  '5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy': { name: 'Dave', kind: 'dev' },
  '5HGjWAeFDfFCWPsjFQdVV2Msvz2XtMktvgocEZcCj68kUMaw': { name: 'Eve', kind: 'dev' },
  '5CiPPseXPECbkjWCa6MnjNokrgYjMqmKndv2rSnekmSK2DjL': { name: 'Ferdie', kind: 'dev' },
  '5GNJqTPyNqANBkUVMN1LPPrxXnFouWXoe2wNSmmEoLctxiZY': { name: 'Alice//stash', kind: 'dev' },
  '5HpG9w8EBLe5XCrbczpwq5TSXvedjrBGCwqxK1iQ7qUsSWFc': { name: 'Bob//stash', kind: 'dev' },
  '5DqkYdZSUnbZYJdKcXxPUPZBtWK66U9W8fvhe1E2W4LAGXUH': { name: 'Charlie//stash', kind: 'dev' },
  '5FHDmQjCKwPmsaAxfiQd5p7nSYVJnzAUvB2CgZGcLQxoChT8': { name: 'Dave//stash', kind: 'dev' },
  '5HBuLJz9LdkUNseUEL6DLeVkx2bqEi6pQr8Ea7fS4bzx7i7E': { name: 'Eve//stash', kind: 'dev' },
  '5CRmqmsiNFExV6VbdmPJViVxrWmkaXXvBrSX8oqBT8R9vmWk': { name: 'Ferdie//stash', kind: 'dev' },
}

// Keys are SS58 addresses; declaration order is irrelevant.
//
// TODO: fill in real Allfeat / Melodie addresses as they become known.
// Treasury SS58 is derivable from `PalletId(*b"py/trsry")` + the chain's
// SS58 prefix. Sudo can be queried from `sudo.key()` storage on-chain.
// Validators come from `session.validators()`.
const STATIC_REGISTRY: Record<string, NetworkRegistry> = {
  allfeat: {
    ...DEV_ACCOUNTS,
    qSwoJVKfgchSRjD6CZ739j9G7zR1khXqkvbeMVCN1NPKJgeup: {
      name: 'Treasury',
      kind: 'treasury',
    },
    qSysBTZC3yQRKNroife4djUTQwnfVxHQ19PpxgHKcRFJszHRA: {
      name: 'Sudo',
      kind: 'sudo',
    },
    qSuThURyMoCfggEMKFSL1j1V9TH3vhvLF5cKGaQKxYPAWfSby: {
      name: 'MusicDASH',
      kind: 'validator',
    },
    qSujxSNtCBdLQbXvmwWmY8Vtk19EKK3zcA1e5bFfEGgscD83J: {
      name: 'Snow-Fall',
      kind: 'validator',
    },
    qSv3xY3t1rFkhxvpSBdqChhwjTCqJ1qJjNoq5ZKFo8vvTgms4: {
      name: 'Allfeat Foundation 1',
      kind: 'validator',
    },
    qSuo1LcUoi7JFNQbar8r8N7JN9JSMggsFEUKcPWc6sEyPjiFa: {
      name: 'Allfeat Foundation 2',
      kind: 'validator',
    },
    qSyiGBEyMGAdC4oxUvDZVAAiCqDswfVyw4FsmkLnvmmnwB1mx: {
      name: 'exa_Validator',
      kind: 'validator',
    },
  },
  melodie: {
    ...DEV_ACCOUNTS,
  },
}

// Mirror of `src/network.rs::NETWORKS[*].seed`. These are `#[serde(skip)]`
// on the Rust side, so they never reach the frontend via the API — the
// only way to recompute the mock role SS58s is to duplicate them here.
// Keep in sync if the network catalogue grows or a seed changes.
const MOCK_NETWORK_SEEDS: Record<string, number> = {
  allfeat: 0x0a1fea70,
  melodie: 0xbeaaf00d,
}

function buildMockRegistry(networkId: string): NetworkRegistry {
  const seed = MOCK_NETWORK_SEEDS[networkId]
  if (seed === undefined) return {}
  const entries: NetworkRegistry = {
    [mockTreasury(seed)]: {
      name: 'Treasury',
      kind: 'treasury',
      note: 'Mocked treasury account — receives a slice of balance transfers and appears in Top Accounts.',
    },
    [mockSudo(seed)]: {
      name: 'Sudo',
      kind: 'sudo',
      note: 'Mocked sudo key — signs roughly one extrinsic out of eleven per block.',
    },
  }
  for (let i = 0; i < MOCK_VALIDATOR_POOL; i++) {
    entries[mockValidator(seed, i)] = {
      name: `Validator ${i + 1}`,
      kind: 'validator',
    }
  }
  return entries
}

// Per-network merged registry. Static entries override mock entries in
// case of collision, giving operators a clean override lever should a
// real address ever happen to collide with a mocked one.
const REGISTRY: Record<string, NetworkRegistry> = Object.fromEntries(
  Object.keys(MOCK_NETWORK_SEEDS).map(networkId => [
    networkId,
    { ...buildMockRegistry(networkId), ...(STATIC_REGISTRY[networkId] ?? {}) },
  ]),
)

export function lookupKnownAccount(
  networkId: string | null | undefined,
  address: string | null | undefined,
): KnownAccount | null {
  if (!networkId || !address) return null
  const net = REGISTRY[networkId]
  if (!net) return null
  return net[address] ?? null
}

// Shape returned by `searchKnownAccounts`, carrying everything the
// navbar search needs to build a suggestion row without re-reading the
// registry.
export interface KnownAccountMatch extends KnownAccount {
  address: string
}

// Fuzzy-ish search across the per-network registry. Scores by match
// quality so the dropdown surfaces the best label hit first:
//
//   exact name           100
//   name prefix           80
//   address prefix (case)  70
//   name substring        60
//   address prefix (ci)    50
//
// Case-sensitive address prefix wins over case-insensitive because SS58
// is a case-sensitive alphabet — "5fhSCz" and "5FHSCZ" are different
// addresses. Names are matched case-insensitively since users type them
// as natural language.
//
// `limit` caps the returned list; the default (8) matches the dropdown
// budget and keeps the comparison O(n·m) bounded for tiny n.
export function searchKnownAccounts(
  networkId: string | null | undefined,
  query: string,
  limit = 8,
): KnownAccountMatch[] {
  if (!networkId) return []
  const q = query.trim()
  if (q.length < 2) return []
  const net = REGISTRY[networkId]
  if (!net) return []

  const qLower = q.toLowerCase()
  const scored: Array<{ match: KnownAccountMatch, score: number }> = []

  for (const address of Object.keys(net)) {
    const info = net[address]!
    const nameLower = info.name.toLowerCase()
    let score = 0

    if (nameLower === qLower) score = 100
    else if (nameLower.startsWith(qLower)) score = 80
    else if (address.startsWith(q)) score = 70
    else if (nameLower.includes(qLower)) score = 60
    else if (address.toLowerCase().startsWith(qLower)) score = 50

    if (score > 0) {
      scored.push({
        match: { address, name: info.name, kind: info.kind, note: info.note },
        score,
      })
    }
  }

  // Stable sort: primary by score desc, secondary by name asc — so ties
  // read alphabetically ("Validator 1" before "Validator 10").
  scored.sort((a, b) =>
    b.score - a.score
    || a.match.name.localeCompare(b.match.name, 'en', { numeric: true }),
  )

  return scored.slice(0, limit).map(r => r.match)
}
