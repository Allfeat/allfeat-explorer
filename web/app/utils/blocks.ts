// Block-count → human duration helpers. Allfeat mainnet ticks every 6s,
// so this module hard-codes that cadence — if the chain ever swaps in a
// different block time these helpers should accept it as a parameter
// rather than be re-derived per-network.

const SECONDS_PER_BLOCK = 6
const SECONDS_PER_MINUTE = 60
const SECONDS_PER_HOUR = 3_600
const SECONDS_PER_DAY = 86_400
const SECONDS_PER_MONTH = 2_592_000 // 30-day month, matches the mock spec
const SECONDS_PER_YEAR = 31_557_600

/// Compact human label for a block-denominated duration: "12 months",
/// "3.5 years", "12 days", etc. Returns "instant" for 0 blocks.
export function blocksToHumanDuration(blocks: number | string | bigint): string {
  const n = toNumber(blocks)
  if (n === 0) return 'instant'

  const seconds = n * SECONDS_PER_BLOCK

  if (seconds < SECONDS_PER_HOUR) {
    return formatUnit(seconds / SECONDS_PER_MINUTE, 'minute')
  }
  if (seconds < SECONDS_PER_DAY) {
    return formatUnit(seconds / SECONDS_PER_HOUR, 'hour')
  }
  if (seconds < SECONDS_PER_MONTH) {
    return formatUnit(seconds / SECONDS_PER_DAY, 'day')
  }
  if (seconds < SECONDS_PER_YEAR) {
    return formatUnit(seconds / SECONDS_PER_MONTH, 'month')
  }
  return formatUnit(seconds / SECONDS_PER_YEAR, 'year')
}

/// "in X blocks" → ms estimate. Defaults to the 6s mainnet cadence; callers
/// on faster chains (e.g. testnet @ 3s) pass `blockTimeSecs` explicitly.
/// Returns positive numbers only; values <= 0 collapse to 0 so the caller
/// can format "now".
export function blocksToMs(
  blocks: number | string | bigint,
  blockTimeSecs: number = SECONDS_PER_BLOCK,
): number {
  const secs = blockTimeSecs > 0 ? blockTimeSecs : SECONDS_PER_BLOCK
  return Math.max(0, toNumber(blocks) * secs * 1000)
}

function toNumber(b: number | string | bigint): number {
  if (typeof b === 'number') return Math.max(0, Math.trunc(b))
  if (typeof b === 'bigint') return Number(b < 0n ? 0n : b)
  try {
    const n = Number.parseInt(b, 10)
    return Number.isFinite(n) && n >= 0 ? n : 0
  }
  catch {
    return 0
  }
}

function formatUnit(value: number, unit: string): string {
  // One decimal except when the rounded value is integer or unit is "minute".
  const rounded = Math.round(value * 10) / 10
  const isInteger = Math.abs(rounded - Math.round(rounded)) < 0.05
  const display = isInteger || unit === 'minute' ? Math.round(value) : rounded.toFixed(1)
  const plural = Number.parseFloat(String(display)) === 1 ? unit : `${unit}s`
  return `${display} ${plural}`
}
