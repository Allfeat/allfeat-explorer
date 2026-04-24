// SS58 encode / decode for in-browser address re-formatting.
//
// The backend already serves addresses encoded under the active network's
// prefix. This helper exists so the account-detail view can offer a
// per-page toggle between the network-native format and the generic
// Substrate format (prefix 42), purely client-side, without round-tripping
// to the backend or invalidating cached responses.

import { base58 } from '@scure/base'
import { blake2b } from '@noble/hashes/blake2.js'

const SS58_PRE = new TextEncoder().encode('SS58PRE')

function ss58Checksum(payload: Uint8Array): Uint8Array {
  const data = new Uint8Array(SS58_PRE.length + payload.length)
  data.set(SS58_PRE, 0)
  data.set(payload, SS58_PRE.length)
  return blake2b(data).slice(0, 2)
}

// Substrate's 14-bit prefix encoding. Prefixes 0–63 use a single byte;
// 64–16383 are split across two bytes with the high bit pattern `0b01`
// marking the longer form. See the SS58 spec in the substrate-runtime docs.
function encodePrefix(prefix: number): Uint8Array {
  if (!Number.isInteger(prefix) || prefix < 0 || prefix > 16383) {
    throw new Error(`SS58 prefix out of range: ${prefix}`)
  }
  if (prefix < 64) return new Uint8Array([prefix])
  const lo = ((prefix & 0x00fc) >> 2) | 0x40
  const hi = (prefix >> 8) | ((prefix & 0x0003) << 6)
  return new Uint8Array([lo, hi])
}

function decodePrefix(bytes: Uint8Array): { prefix: number, len: number } {
  const b0 = bytes[0]
  if (b0 === undefined) throw new Error('SS58: empty payload')
  if (b0 < 64) return { prefix: b0, len: 1 }
  if (b0 < 128) {
    const b1 = bytes[1]
    if (b1 === undefined) throw new Error('SS58: truncated 2-byte prefix')
    const prefix = ((b0 & 0x3f) << 2) | (b1 >> 6) | ((b1 & 0x3f) << 8)
    return { prefix, len: 2 }
  }
  throw new Error(`SS58: invalid leading byte 0x${b0.toString(16)}`)
}

export function decodeSs58(addr: string): { publicKey: Uint8Array, prefix: number } {
  const raw = base58.decode(addr)
  const { prefix, len: prefixLen } = decodePrefix(raw)
  if (raw.length < prefixLen + 2) throw new Error('SS58: address too short')
  const payload = raw.slice(0, raw.length - 2)
  const checksum = raw.slice(raw.length - 2)
  const expected = ss58Checksum(payload)
  if (checksum[0] !== expected[0] || checksum[1] !== expected[1]) {
    throw new Error('SS58: checksum mismatch')
  }
  return { publicKey: payload.slice(prefixLen), prefix }
}

export function encodeSs58(publicKey: Uint8Array, prefix: number): string {
  const prefixBytes = encodePrefix(prefix)
  const payload = new Uint8Array(prefixBytes.length + publicKey.length)
  payload.set(prefixBytes, 0)
  payload.set(publicKey, prefixBytes.length)
  const checksum = ss58Checksum(payload)
  const full = new Uint8Array(payload.length + 2)
  full.set(payload, 0)
  full.set(checksum, payload.length)
  return base58.encode(full)
}

export function reencodeSs58(addr: string, prefix: number): string {
  const { publicKey } = decodeSs58(addr)
  return encodeSs58(publicKey, prefix)
}

// Generic-Substrate prefix (addresses starting with `5`). Hardcoded here so
// callers don't need to thread it through; matches the backend default in
// `crate::data::ss58`.
export const GENERIC_SS58_PREFIX = 42
