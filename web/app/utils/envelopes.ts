// Frontend-only metadata layer for the 13 genesis envelopes.
//
// Backend (`pallet-token-allocation`) already returns cap / distributed /
// upfront / cliff / vesting for each `EnvelopeId`. This file adds the
// *scoping* we want users to see in the UI: category grouping, one-line
// purpose, and a canonical share of the 1B genesis cap.
//
// Kept frontend-side per the `frontend-only for display-layer mappings`
// convention — the chain is the source of truth for numbers; this file is
// the source of truth for how we describe them.

import type { EnvelopeId } from '@bindings'

export type EnvelopeCategory =
  | 'team-advisory'
  | 'public-sale'
  | 'ecosystem'
  | 'operations'

export interface EnvelopeCategoryInfo {
  id: EnvelopeCategory
  label: string
  /** Short one-line scope description for section headers. */
  blurb: string
}

export const ENVELOPE_CATEGORIES: readonly EnvelopeCategoryInfo[] = [
  {
    id: 'team-advisory',
    label: 'Team & Advisory',
    blurb: 'Founders, contributors, and strategic investors — long lock-ups and gradual unlocks.',
  },
  {
    id: 'public-sale',
    label: 'Public Sale',
    blurb: 'Community token sales: early retail liquidity plus deferred tranches for later stages.',
  },
  {
    id: 'ecosystem',
    label: 'Ecosystem',
    blurb: 'Rewards and onboarding distributions that seed network participation.',
  },
  {
    id: 'operations',
    label: 'Operations',
    blurb: 'Exchange liquidity, R&D, and the foundation reserve that keep the protocol running.',
  },
] as const

export interface EnvelopeMeta {
  /** Stable share of the 1B genesis cap (percent, same values as the tokenomics report). */
  readonly supplyPct: number
  /** Meta-category this envelope belongs to. */
  readonly category: EnvelopeCategory
  /** One-line purpose shown on the card and detail page. No PoM/MIDDS wording. */
  readonly blurb: string
}

export const ENVELOPE_META: Record<EnvelopeId, EnvelopeMeta> = {
  'teams': {
    supplyPct: 6.7,
    category: 'team-advisory',
    blurb: 'Founding team and core contributors. Long lock then linear release to align long-term commitment.',
  },
  'kol': {
    supplyPct: 0.3,
    category: 'team-advisory',
    blurb: 'Key opinion leaders who amplify outreach. Short engagement cycle with a matching lock + vest.',
  },
  'private1': {
    supplyPct: 12.0,
    category: 'team-advisory',
    blurb: 'First strategic investors. Small launch unlock, then a long vest to stage entry onto the market.',
  },
  'private2': {
    supplyPct: 8.0,
    category: 'team-advisory',
    blurb: 'Second strategic round. Partial launch unlock with a multi-year linear vest.',
  },
  'public1': {
    supplyPct: 3.0,
    category: 'public-sale',
    blurb: 'First public sale. Launch unlock + short linear vest to smooth early price discovery.',
  },
  'public2': {
    supplyPct: 7.5,
    category: 'public-sale',
    blurb: 'Deferred public sale. Locked through the first operating year, then vested monthly.',
  },
  'public3': {
    supplyPct: 3.0,
    category: 'public-sale',
    blurb: 'Second immediate public sale. Launch unlock + short linear vest, complementing Public 1.',
  },
  'public4': {
    supplyPct: 8.0,
    category: 'public-sale',
    blurb: 'Late public sale. Year-long lock, then linear release as network traction builds.',
  },
  'airdrop': {
    supplyPct: 1.0,
    category: 'ecosystem',
    blurb: 'Promotional distribution to seed early users. Fully unlocked at launch.',
  },
  'community-rewards': {
    supplyPct: 26.0,
    category: 'ecosystem',
    blurb: 'Network participation rewards. Released progressively as the ecosystem grows.',
  },
  'listing': {
    supplyPct: 10.0,
    category: 'operations',
    blurb: 'Market-making pool for centralised and decentralised exchange liquidity.',
  },
  'research-development': {
    supplyPct: 12.5,
    category: 'operations',
    blurb: 'Protocol improvement, feature rollouts, security audits, and external builder grants.',
  },
  'reserve': {
    supplyPct: 2.0,
    category: 'operations',
    blurb: 'Foundation contingency fund for partnerships, operations, and unforeseen needs.',
  },
}

/** Get the meta for an envelope, or `undefined` if the id is unknown. */
export function envelopeMeta(id: EnvelopeId): EnvelopeMeta | undefined {
  return ENVELOPE_META[id]
}

/** Get the category info (label, blurb) by id. */
export function categoryInfo(id: EnvelopeCategory): EnvelopeCategoryInfo {
  const hit = ENVELOPE_CATEGORIES.find(c => c.id === id)
  if (!hit) {
    return { id, label: id, blurb: '' }
  }
  return hit
}

/** Sum of `supplyPct` across all envelopes in a category (from the static map). */
export function categorySupplyPct(id: EnvelopeCategory): number {
  let sum = 0
  for (const key in ENVELOPE_META) {
    const meta = ENVELOPE_META[key as EnvelopeId]
    if (meta.category === id) sum += meta.supplyPct
  }
  // Round to 1 decimal to hide float drift (0.3 + 6.7 + 12 + 8 = 27).
  return Math.round(sum * 10) / 10
}
