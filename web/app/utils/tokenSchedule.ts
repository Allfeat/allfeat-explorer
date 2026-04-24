// Pre-launch vesting schedule for the 13 genesis envelopes — drives the
// "Token emission over 5 years" chart on the token hub.
//
// Numbers mirror the Allfeat tokenomics report (1B AFT total). The chain
// is the source of truth for *actual* on-chain unlocks; this file is the
// static *plan*. Frontend-only per `frontend-only for display-layer
// mappings`, same reasoning as `envelopes.ts`.

import type { EnvelopeId } from '@bindings'

export interface VestingParams {
  /** Total cap in AFT (whole tokens, not planck). */
  readonly cap: number
  /** Tokens unlocked at TGE (t=0). */
  readonly upfront: number
  /** Months of cliff after TGE before linear vest begins. */
  readonly lockMonths: number
  /** Months of linear vest after the cliff. Zero = no vest. */
  readonly vestMonths: number
}

export type EnvelopeGroup = 'team-advisory' | 'public-sale' | 'operations-ecosystem'

export interface EnvelopeScheduleEntry {
  readonly id: EnvelopeId
  readonly label: string
  readonly color: string
  readonly group: EnvelopeGroup
  readonly params: VestingParams
  /** PoM (Reward) is milestone-triggered, not linear — handled specially. */
  readonly milestoneBased?: boolean
}

// Order drives stacking bottom → top in the area chart. Mirrors the
// layout used in the tokenomics report figure.
export const SCHEDULE: readonly EnvelopeScheduleEntry[] = [
  { id: 'teams', label: 'Team', color: '#00B18C', group: 'team-advisory',
    params: { cap: 67_000_000, upfront: 0, lockMonths: 12, vestMonths: 36 } },
  { id: 'private1', label: 'Private 1', color: '#14C79B', group: 'team-advisory',
    params: { cap: 120_000_000, upfront: 6_000_000, lockMonths: 8, vestMonths: 38 } },
  { id: 'private2', label: 'Private 2', color: '#2DDDB0', group: 'team-advisory',
    params: { cap: 80_000_000, upfront: 4_000_000, lockMonths: 3, vestMonths: 36 } },

  { id: 'public1', label: 'Public 1', color: '#FF4A5F', group: 'public-sale',
    params: { cap: 30_000_000, upfront: 5_000_000, lockMonths: 0, vestMonths: 6 } },
  { id: 'public2', label: 'Public 2', color: '#F26578', group: 'public-sale',
    params: { cap: 75_000_000, upfront: 0, lockMonths: 18, vestMonths: 12 } },
  { id: 'public3', label: 'Public 3', color: '#E67A8A', group: 'public-sale',
    params: { cap: 30_000_000, upfront: 5_000_000, lockMonths: 0, vestMonths: 6 } },
  { id: 'public4', label: 'Public 4', color: '#D98E9C', group: 'public-sale',
    params: { cap: 80_000_000, upfront: 0, lockMonths: 12, vestMonths: 12 } },
  { id: 'community-rewards', label: 'Reward', color: '#C43D4D', group: 'public-sale',
    params: { cap: 260_000_000, upfront: 4_333_333, lockMonths: 0, vestMonths: 168 },
    milestoneBased: true },

  { id: 'kol', label: 'KOL', color: '#C4C0B1', group: 'operations-ecosystem',
    params: { cap: 3_000_000, upfront: 0, lockMonths: 9, vestMonths: 9 } },
  { id: 'airdrop', label: 'Airdrop', color: '#B8B39F', group: 'operations-ecosystem',
    params: { cap: 10_000_000, upfront: 10_000_000, lockMonths: 0, vestMonths: 0 } },
  { id: 'listing', label: 'CEX/DEX', color: '#ACA68D', group: 'operations-ecosystem',
    params: { cap: 100_000_000, upfront: 0, lockMonths: 4, vestMonths: 12 } },
  { id: 'research-development', label: 'R&D', color: '#A0997B', group: 'operations-ecosystem',
    params: { cap: 125_000_000, upfront: 25_000_000, lockMonths: 1, vestMonths: 26 } },
  { id: 'reserve', label: 'Reserve', color: '#958D69', group: 'operations-ecosystem',
    params: { cap: 20_000_000, upfront: 20_000_000, lockMonths: 0, vestMonths: 0 } },
] as const

// Tokens unlocked by month `t` (0 = TGE). Linear vest after the cliff,
// clamped to cap. For milestone-based schedules (PoM) we use a sub-linear
// power curve (exponent 0.85) that matches the published chart shape:
// fast early tranches slow as AFT/MIDDS reward declines across tranches.
export function unlockedAt(e: EnvelopeScheduleEntry, t: number): number {
  const { cap, upfront, lockMonths, vestMonths } = e.params
  if (t <= 0) return upfront
  if (e.milestoneBased) {
    const x = Math.min(1, t / vestMonths)
    return upfront + (cap - upfront) * Math.pow(x, 0.85)
  }
  if (vestMonths === 0) return cap
  if (t < lockMonths) return upfront
  const vested = Math.min(1, (t - lockMonths) / vestMonths)
  return upfront + (cap - upfront) * vested
}

export interface SchedulePoint {
  readonly month: number
  readonly values: Readonly<Record<EnvelopeId, number>>
  readonly total: number
}

export function buildSchedule(months = 60): SchedulePoint[] {
  const out: SchedulePoint[] = []
  for (let t = 0; t <= months; t++) {
    const values = {} as Record<EnvelopeId, number>
    let total = 0
    for (const e of SCHEDULE) {
      const v = unlockedAt(e, t)
      values[e.id] = v
      total += v
    }
    out.push({ month: t, values, total })
  }
  return out
}

const MONTH_NAMES = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC']

// TGE is Feb 2026 → month offset 0 maps to Feb-26.
export function monthLabel(t: number): string {
  const absMonth = 1 + t // 1 = February
  const yearOff = Math.floor(absMonth / 12)
  const monthIdx = ((absMonth % 12) + 12) % 12
  const yy = (26 + yearOff).toString().padStart(2, '0')
  return `${MONTH_NAMES[monthIdx]}-${yy}`
}
