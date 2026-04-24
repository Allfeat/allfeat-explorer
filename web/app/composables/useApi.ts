// API client wrapper. Picks the correct base URL depending on where the
// call runs:
//   - SSR: absolute backend origin (runtimeConfig.apiOrigin + apiBase),
//     because Nitro's internal dispatcher doesn't honour Vite's dev proxy
//     and relative URLs would otherwise loop back through Vue Router.
//   - Client: the public `apiBase` relative path, which dev-time Vite
//     proxies to the backend and prod reverse-proxy forwards natively.

export function apiBaseUrl(): string {
  const config = useRuntimeConfig()
  if (import.meta.server) {
    return `${config.apiOrigin}${config.public.apiBase}`
  }
  return config.public.apiBase
}

export function apiFetch<T>(path: string, opts: Parameters<typeof $fetch<T>>[1] = {}) {
  return $fetch<T>(path, {
    baseURL: apiBaseUrl(),
    ...opts,
  })
}
