// Display-side formatters. Ported from the React maquette (`data.js`
// `shortHash`/`shortAddr`/`fmtAFT`/`fmtInt`) so outputs stay identical to
// the design mock.

// Collapse a 0x-prefixed hex string to head…tail. `head` is the number of
// hex chars after "0x"; the leading "0x" is kept verbatim.
export function shortHash(h: string, head = 6, tail = 4): string {
  if (!h) return ''
  return `${h.slice(0, head + 2)}…${h.slice(-tail)}`
}

// Collapse an SS58 address to head…tail. Works for any string; on malformed
// input shorter than head+tail, returns the original unchanged.
export function shortAddr(a: string, head = 6, tail = 6): string {
  if (!a) return ''
  if (a.length <= head + tail) return a
  return `${a.slice(0, head)}…${a.slice(-tail)}`
}

// Heuristic SS58 shape check used to linkify addresses surfaced as plain
// strings in event field values (the decoder emits SS58 for any 32-byte
// unnamed composite — see data/metadata.rs::render_value). Length 45–50
// matches every common SS58 prefix; charset excludes 0/O/I/l per base58.
const SS58_LIKE = /^[1-9A-HJ-NP-Za-km-z]{45,50}$/
export function looksLikeAddress(v: string | null | undefined): boolean {
  return typeof v === 'string' && SS58_LIKE.test(v)
}

// Format an integer with thousands separators. Accepts `number`, `bigint`,
// or a numeric string (u64/u128 coming from the API as `string` via ts-rs).
// For string input we route through BigInt to preserve precision beyond
// Number.MAX_SAFE_INTEGER.
export function fmtInt(n: number | string | bigint): string {
  if (typeof n === 'bigint') return n.toLocaleString('en-US')
  if (typeof n === 'number') return n.toLocaleString('en-US')
  try {
    return BigInt(n).toLocaleString('en-US')
  } catch {
    return n
  }
}

// Coerce an API planck value (`u64_string` / `u128_string` serde helpers emit
// numeric strings; older code paths still pass `number`) to a BigInt without
// throwing. `0n` on malformed or absent input so render math can keep going
// without null checks at every call site.
export function toBigIntSafe(v: number | string | bigint | undefined | null): bigint {
  if (v === undefined || v === null) return 0n
  if (typeof v === 'bigint') return v
  if (typeof v === 'number') return BigInt(Math.trunc(v))
  try { return BigInt(v) } catch { return 0n }
}

// Convert a planck-denominated integer to a human AFT amount. `decimals`
// is the chain's token-decimal count (12 for AFT). `fix` caps the number
// of fractional digits in the output.
//
// We accept JS number for small amounts (tip, fee in the mock) and
// BigInt/string for real balances. For JS number the classic `p / 10**d`
// path is fine; for BigInt/string we split the value into integer and
// fractional BigInts so huge balances don't lose precision going through
// Number.
// Wall-clock timestamp as "YYYY-MM-DD HH:MM:SS UTC", matching the
// maquette's `new Date(ts).toISOString().replace('T',' ').slice(0,19)`.
export function fmtUtcTime(ms: number): string {
  return `${new Date(ms).toISOString().replace('T', ' ').slice(0, 19)} UTC`
}

export function fmtAFT(planck: number | string | bigint, decimals = 12, fix = 4): string {
  const bi = toBigIntSafe(planck)
  if (bi === 0n) return '0'

  const neg = bi < 0n
  const abs = neg ? -bi : bi
  const base = 10n ** BigInt(decimals)
  const wholeUnrounded = abs / base

  // Scale precision down for large whole parts — `fix` is treated as a
  // ceiling. Two decimals on a million-AFT balance is noise; the magnitude
  // already conveys the relevant information.
  let effectiveFix = fix
  if (wholeUnrounded >= 1_000_000n) effectiveFix = 0
  else if (wholeUnrounded >= 1_000n) effectiveFix = Math.min(fix, 2)

  // Round half-up at the `effectiveFix`-th fractional digit so a near-cap
  // total-issuance reading like 999_999_999.999… AFT renders as
  // "1,000,000,000" instead of truncating to "999,999,999".
  const dropDigits = Math.max(0, decimals - effectiveFix)
  const halfUnit = dropDigits > 0 ? 10n ** BigInt(dropDigits - 1) * 5n : 0n
  const rounded = abs + halfUnit
  const whole = rounded / base
  const frac = rounded % base

  // Stringify the fractional part zero-padded to `decimals`, then trim to
  // `effectiveFix` digits. We avoid JS floating-point entirely.
  const fracStr = frac.toString().padStart(decimals, '0').slice(0, Math.max(0, effectiveFix))
  const fracTrimmed = fracStr.replace(/0+$/, '')

  // Below 1 AFT with no leading digits after truncation → fall back to
  // exponential notation for visibility, matching the maquette.
  if (whole === 0n && fracTrimmed === '') {
    const approx = Number(abs) / Number(base)
    return (neg ? '-' : '') + (approx === 0 ? '0' : approx.toExponential(2))
  }

  const wholeStr = whole.toLocaleString('en-US')
  const out = fracTrimmed ? `${wholeStr}.${fracTrimmed}` : wholeStr
  return neg ? `-${out}` : out
}

// Format a percentage with up to `max` decimals, stripping trailing zeros so
// `100.0%` renders as `100%` while `12.5%` is preserved. Falls back to `0%`
// on non-finite input so templates don't crash on division-by-zero.
export function fmtPct(value: number, max = 1): string {
  if (!Number.isFinite(value)) return '0%'
  return `${+value.toFixed(max)}%`
}

// Compact byte size — B / KB / MB. Not quite IEC (1024-based) and not
// quite SI (1000-based) — we follow the maquette's convention: binary
// prefixes with no suffix letter ("MB" not "MiB") to match the runtime
// page's existing wording.
export function fmtBytesCompact(n: number | null | undefined, placeholder = '—'): string {
  if (n == null) return placeholder
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(2)} KB`
  return `${(n / 1_048_576).toFixed(2)} MB`
}

// Allfeat's mainnet packs `major.minor.patch` into a single u32 (top
// digit = major, next three = minor, final three = patch, e.g.
// 1_001_004 → "1.001.004"). Smaller chains (Melodie's pre-mainnet
// builds, dev networks) use a plain counter — a dev runtime on spec 201
// should display as "201", not "0.000.201". The 1_000_000 threshold
// separates the two conventions cleanly: below it we show the raw
// integer, at or above it we assume packed encoding and unpack.
export function fmtSpecVersionDotted(n: number | null | undefined, placeholder = '—'): string {
  if (n == null) return placeholder
  if (n < 1_000_000) return fmtInt(n)
  const s = String(n)
  return `${s.slice(0, -6)}.${s.slice(-6, -3)}.${s.slice(-3)}`
}
