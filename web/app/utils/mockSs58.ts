// Byte-for-byte TS port of `src/mock/rng.rs::Lcg` and
// `src/mock/data.rs::ss58_seeded` / `mix`. Used only to derive the
// SS58 addresses of mock "known role" accounts (sudo / treasury /
// validators) so the frontend registry can label them without a
// round-trip to the backend.
//
// When touching the Rust formula, update this file in lockstep — the
// whole point is that a given `(network_seed, role_seed)` pair produces
// the exact same SS58 on both sides.

const SS58_ALPHABET =
  'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'

// LCG constants match Rust Lcg::advance verbatim.
const LCG_MUL = 1664525
const LCG_ADD = 1013904223

// u32 wrapping multiply. JS numbers are f64 so `(a * b) | 0` can overflow
// 32 bits intermediate — do it in BigInt, then mask. The `Number(...)`
// coerces through a BigInt ≤ 2^32, which is a safe integer.
function mulWrap(a: number, b: number): number {
  return Number((BigInt(a >>> 0) * BigInt(b >>> 0)) & 0xffffffffn)
}

function addWrap(a: number, b: number): number {
  return ((a >>> 0) + (b >>> 0)) >>> 0
}

class Lcg {
  private state: number
  constructor(seed: number) {
    this.state = seed >>> 0
  }
  // Returns an unsigned 32-bit integer in [0, 2^32). Kept explicitly
  // unsigned — JS bitwise ops interpret their operand as signed i32, so
  // `state & 0xffffffff` would produce negative numbers for states
  // ≥ 2^31 and desync every modulo downstream.
  nextU32(): number {
    this.state = addWrap(mulWrap(this.state, LCG_MUL), LCG_ADD)
    return this.state
  }
}

export function mixSeed(networkSeed: number, x: number): number {
  return addWrap(networkSeed, x)
}

export function ss58Seeded(seed: number): string {
  const rng = new Lcg(seed)
  let out = '5'
  for (let i = 0; i < 47; i++) {
    // Rust: `(v as usize) % SS58_ALPHABET.len()`. `v` is u32; on 64-bit
    // Rust `as usize` widens without change. Modulo against 62 is safe
    // in JS because the numerator fits in Number.MAX_SAFE_INTEGER.
    out += SS58_ALPHABET[rng.nextU32() % SS58_ALPHABET.length]
  }
  return out
}

// Role seeds — mirror `src/mock/data.rs::SEED_*`.
export const MOCK_SEED_SUDO = 0x50000001
export const MOCK_SEED_TREASURY = 0x50000002
export const MOCK_VALIDATOR_POOL = 8
export const MOCK_SEED_VALIDATOR_BASE = 0x50000100

export function mockSudo(networkSeed: number): string {
  return ss58Seeded(mixSeed(networkSeed, MOCK_SEED_SUDO))
}

export function mockTreasury(networkSeed: number): string {
  return ss58Seeded(mixSeed(networkSeed, MOCK_SEED_TREASURY))
}

export function mockValidator(networkSeed: number, i: number): string {
  const idx = i % MOCK_VALIDATOR_POOL
  return ss58Seeded(mixSeed(networkSeed, MOCK_SEED_VALIDATOR_BASE + idx))
}
