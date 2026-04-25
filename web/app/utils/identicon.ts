// Deterministic per-address visual fingerprint. From any string seed
// (typically an SS58 address) we derive a hue and the shape of a
// continuous waveform to render inside a circular disc.
//
// Anti-collision strategy: two FNV-1a passes (different offset basis)
// feed a xorshift32 PRNG that we expand into 32 bytes. Distinct bytes
// drive distinct features (colour, harmonic mix, phase), so the
// effective unique-combination space is ~2^64 — well beyond the size
// of any realistic SS58 set the explorer renders.

export interface IdenticonParams {
  hue: number
  saturation: number
  lightness: number
  points: { x: number; y: number }[]
}

const SAMPLES = 72

function expandSeed(s: string, n: number): Uint8Array {
  let h1 = 0x811c9dc5 >>> 0
  let h2 = 0xdeadbeef >>> 0
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i)
    h1 = Math.imul(h1 ^ c, 0x01000193) >>> 0
    h2 = Math.imul(h2 ^ c, 0x01000193) >>> 0
  }
  let state = (h1 ^ Math.imul(h2, 0x9e3779b1)) >>> 0
  if (state === 0) state = 1

  const out = new Uint8Array(n)
  for (let i = 0; i < n; i++) {
    state ^= state << 13
    state ^= state >>> 17
    state ^= state << 5
    state >>>= 0
    out[i] = state & 0xff
  }
  return out
}

export function identiconParams(seed: string): IdenticonParams {
  const bytes = expandSeed(seed, 32)
  // 16 bits → 360° (granular enough to read as a unique tone), saturation
  // 60–94 keeps the line vibrant, lightness 56–70 stays visible on the
  // dark disc without going into eye-burning neon territory.
  const hue = Math.floor(((bytes[0]! << 8) | bytes[1]!) * 360 / 65536)
  const saturation = 60 + (bytes[2]! % 35)
  const lightness = 56 + (bytes[3]! % 15)

  // Control-point waveform: pick 5–12 anchor heights from the seed and
  // cosine-interpolate between them. Unlike a harmonic sum, adjacent
  // peaks/valleys can have wildly different magnitudes — that's what
  // gives the organic, less-uniform look (a tall peak next to a small
  // bump, then a deep dip), which a pure sine never produces.
  const ctrlCount = 5 + (bytes[4]! % 8)
  const segments = ctrlCount - 1

  const ctrl: number[] = []
  for (let i = 0; i < ctrlCount; i++) {
    // bytes 12..23 give 12 distinct anchor values — enough for the
    // ctrlCount range without ever wrapping back into colour bytes.
    ctrl.push((bytes[12 + i]! / 127.5) - 1)
  }

  const points: { x: number; y: number }[] = []
  for (let i = 0; i < SAMPLES; i++) {
    const t = i / (SAMPLES - 1)
    const idx = t * segments
    const i0 = Math.min(Math.floor(idx), segments - 1)
    const local = idx - i0
    // Cosine interpolation: smooth tangent at every anchor, no sharp
    // corners — reads as a flowing wave instead of a polyline.
    const f = (1 - Math.cos(local * Math.PI)) * 0.5
    const y = ctrl[i0]! * (1 - f) + ctrl[i0 + 1]! * f
    points.push({ x: t, y })
  }

  let peak = 0
  for (const p of points) {
    const a = Math.abs(p.y)
    if (a > peak) peak = a
  }
  if (peak > 0) {
    for (const p of points) p.y /= peak
  }

  return { hue, saturation, lightness, points }
}

export function identiconPath(
  params: IdenticonParams,
  viewBox: number,
  margin: number,
): string {
  const w = viewBox - margin * 2
  const cy = viewBox / 2
  // Fixed amp ~52% of disc height. Control-point waves rarely peg both
  // adjacent anchors to opposite extremes, so the typical visual amplitude
  // stays well clear of the disc edges even without a freq-dependent cap.
  const amp = ((viewBox / 2) - margin) * 0.62
  let d = ''
  for (let i = 0; i < params.points.length; i++) {
    const p = params.points[i]!
    const x = margin + p.x * w
    const y = cy - p.y * amp
    d += i === 0 ? `M${x.toFixed(2)},${y.toFixed(2)}` : `L${x.toFixed(2)},${y.toFixed(2)}`
  }
  return d
}

export function identiconColor(params: IdenticonParams): string {
  return `hsl(${params.hue} ${params.saturation}% ${params.lightness}%)`
}
