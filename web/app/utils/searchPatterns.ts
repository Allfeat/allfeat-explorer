// Pure input-classification helpers for the navbar search bar.
//
// The backend doesn't expose prefix/search endpoints — every lookup is by
// exact id. We compensate by inspecting what the user typed and firing
// point-lookups against the endpoints that accept that shape. A single
// input can match several kinds (e.g. "123" is a valid Block number AND
// ATS id), so `detectKinds` returns a set and the caller probes each in
// parallel.

export type SearchKind =
  | 'block-number'
  | 'block-hash'
  | 'extrinsic-id'
  | 'account'
  | 'ats-id'

// SS58 candidate alphabet. Real Substrate addresses use base58 (which
// excludes 0, O, I, l), but the mock generator in `utils/mockSs58.ts`
// uses the full alphanumeric alphabet — and production on Allfeat
// already echoes mock-style addresses back through the API. Accepting
// the broader alphabet here is strictly a superset of real base58, so
// real addresses still pass; the endpoint is the source of truth for
// whether the account exists.
const SS58_CHAR = /^[0-9A-Za-z]+$/

// u32 cap (ATS ids are u32 on-chain); beyond this it can't be an ATS.
const U32_MAX = 0xffff_ffff

export interface DetectedInput {
  kinds: Set<SearchKind>
  /** Parsed integer when the query is all digits, else null. */
  asInt: bigint | null
  /** Pre-split `<block>-<index>` extrinsic id when the shape matches. */
  asExtrinsic: { block: string, index: number } | null
  /** Full 0x-prefixed block hash when the shape matches exactly. */
  asBlockHash: string | null
  /** Raw SS58 candidate when the shape matches. */
  asAddress: string | null
}

export function detectKinds(raw: string): DetectedInput {
  const q = raw.trim()
  const out: DetectedInput = {
    kinds: new Set(),
    asInt: null,
    asExtrinsic: null,
    asBlockHash: null,
    asAddress: null,
  }
  if (!q) return out

  // Pure integer → both a plausible block number (u64) and ATS id (u32).
  if (/^\d+$/.test(q)) {
    try {
      const n = BigInt(q)
      if (n >= 0n) {
        out.asInt = n
        out.kinds.add('block-number')
        if (n <= BigInt(U32_MAX)) out.kinds.add('ats-id')
      }
    } catch { /* overflow / malformed — ignore */ }
    return out
  }

  // Extrinsic id is `<block-number>-<index>`, index fits in u32.
  const extMatch = q.match(/^(\d+)-(\d+)$/)
  if (extMatch) {
    const block = extMatch[1]!
    const index = Number(extMatch[2]!)
    if (Number.isSafeInteger(index) && index >= 0) {
      out.asExtrinsic = { block, index }
      out.kinds.add('extrinsic-id')
    }
    return out
  }

  // Block hash — exact shape only (32-byte hex). Partial pastes stay silent.
  if (/^0x[0-9a-fA-F]{64}$/.test(q)) {
    out.asBlockHash = q.toLowerCase()
    out.kinds.add('block-hash')
    return out
  }

  // SS58 account. Permissive length; the account endpoint rejects bad ones.
  if (SS58_CHAR.test(q) && q.length >= 40 && q.length <= 60) {
    out.asAddress = q
    out.kinds.add('account')
    return out
  }

  return out
}
