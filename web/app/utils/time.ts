// Human-readable relative time. `now` is passed in so the function stays
// pure and SSR-safe — components source it from useWallClock() on the
// client and from Date.now() at render on the server.

export function timeAgo(ts: number, now: number): string {
  const diff = Math.max(1, Math.round((now - ts) / 1000))
  if (diff < 60) return `${diff} sec${diff === 1 ? '' : 's'} ago`
  const m = Math.round(diff / 60)
  if (m < 60) return `${m} min${m === 1 ? '' : 's'} ago`
  const h = Math.round(m / 60)
  if (h < 24) return `${h} hr${h === 1 ? '' : 's'} ago`
  const d = Math.round(h / 24)
  return `${d} day${d === 1 ? '' : 's'} ago`
}
