// Merged build identity for footer display.
//
// Frontend side comes from `runtimeConfig.public` (package.json version
// + git sha baked in at `nuxt build` time). Backend side comes from
// `GET /api/v1/meta` — fetched once, cached under a fixed key so the
// payload is shared across every page that shows the footer.

import type { BuildInfo } from '@bindings'

export interface BuildInfoPair {
  frontend: { version: string, gitSha: string }
  api: { version: string | null, gitSha: string | null }
}

export function useBuildInfo() {
  const config = useRuntimeConfig()
  const frontend = {
    version: config.public.frontendVersion as string,
    gitSha: config.public.frontendGitSha as string,
  }

  const { data, pending, error } = useFetch<BuildInfo>('/meta', {
    key: 'build-info',
    baseURL: apiBaseUrl(),
    default: () => null as unknown as BuildInfo,
    // Endpoint is static per deploy — no need to refetch on route change.
    server: true,
  })

  const info = computed<BuildInfoPair>(() => ({
    frontend,
    api: {
      version: data.value?.version ?? null,
      gitSha: data.value?.git_sha ?? null,
    },
  }))

  return { info, pending, error }
}
