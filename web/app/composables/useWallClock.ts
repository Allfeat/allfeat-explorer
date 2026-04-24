// Singleton 1-Hz wall clock shared by TimeAgo and countdowns. SSR renders
// with the request-time snapshot; the interval starts once on the client
// after hydration and runs for the session. We keep the interval alive
// for the whole client lifetime — every data-heavy page has at least one
// consumer and cycling the timer on mount/unmount would cost more than it
// saves. `useState` gives us SSR-safe, request-scoped storage so the
// initial timestamp doesn't leak across requests.

let clientInterval: ReturnType<typeof setInterval> | null = null

export function useWallClock(): Ref<number> {
  const now = useState<number>('wall-clock', () => Date.now())

  if (import.meta.client && !clientInterval) {
    clientInterval = setInterval(() => {
      now.value = Date.now()
    }, 1000)
  }

  return now
}
