// Build the list of slots shown by <Pagination/>: actual page numbers
// interleaved with 'ellipsis' markers. Always keeps first + last visible
// and a sibling window around the current page. The "<=7 → show all"
// shortcut avoids emitting ellipses when there's no gap to hide.

export type PageSlot = number | 'ellipsis'

export function paginationPages(current: number, total: number, siblings = 1): PageSlot[] {
  if (total <= 1) return [1]
  if (total <= 7) return Array.from({ length: total }, (_, i) => i + 1)

  const out: PageSlot[] = [1]
  const left = Math.max(current - siblings, 2)
  const right = Math.min(current + siblings, total - 1)

  if (left > 2) out.push('ellipsis')
  for (let i = left; i <= right; i++) out.push(i)
  if (right < total - 1) out.push('ellipsis')
  out.push(total)

  return out
}
