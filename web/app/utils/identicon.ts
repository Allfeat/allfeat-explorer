// Deterministic conic-gradient identicon derived from any string seed
// (usually an SS58 address). The gradient stops are fixed — only the
// starting angle rotates — so every address maps to one stable swatch
// without bundling a real blake2 identicon lib. Matches the maquette's
// `hashAngle` helper.

export function hashAngle(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) {
    h = (h * 31 + s.charCodeAt(i)) % 360
  }
  return h
}

export function identiconGradient(seed: string): string {
  const angle = hashAngle(seed)
  return `conic-gradient(from ${angle}deg, var(--teal-500), var(--red-500), var(--cream-200), var(--teal-700), var(--teal-500))`
}
