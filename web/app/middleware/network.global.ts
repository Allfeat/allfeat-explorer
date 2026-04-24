// Route middleware — validates the `?network=<id>` query against the
// catalogue loaded by `plugins/networks.server.ts`. When the id is known,
// the middleware is a no-op. When it's unknown (user typo, stale bookmark,
// link across deploys), we strip the query and let the default network
// take over rather than silently serving the wrong chain.
//
// Runs on both SSR and client via the `.global` suffix, but bails when
// the networks store isn't loaded yet (cold SSR boots the plugin before
// the first page render, but we stay defensive).

import { useNetworksStore } from '~/stores/networks'

export default defineNuxtRouteMiddleware((to) => {
  const store = useNetworksStore()
  if (!store.loaded) return

  const requested = to.query.network
  if (typeof requested !== 'string' || requested === '') return
  if (store.byId(requested)) return

  const { network: _stripped, ...rest } = to.query
  return navigateTo({ path: to.path, query: rest, hash: to.hash }, { replace: true })
})
