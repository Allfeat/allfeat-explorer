// Client-only plugin that drives the live WebSocket singleton.
//
// The socket belongs to the tab, not to any single component. We wait
// for `app:mounted` before wiring the watcher so the initial connection
// attempt cannot race hydration: if the WS `open` event lands before
// the footer hydrates, Vue sees `connection = 'connected'` while the
// SSR markup still says `connecting`, and we get a hydration mismatch.
// Registering the watcher after mount guarantees hydration completes
// against the stable `'connecting'` default before the socket opens.
//
// Server runs skip this plugin entirely (`.client.ts` suffix), so SSR
// renders with the store's default connection state ('connecting') and
// empty buffers. Pages that use `useLiveBlocks` seed the buffers on SSR
// via useFetch, so the footer's BLOCK/FINALIZED readout is filled when
// the user lands on a page that consumes blocks (Phase 9 dashboard).

import { watch } from 'vue'
import { useActiveNetwork } from '~/composables/useActiveNetwork'
import { useLiveSocket } from '~/composables/useLiveSocket'

export default defineNuxtPlugin((nuxtApp) => {
  const { id: activeId } = useActiveNetwork()
  const socket = useLiveSocket()

  nuxtApp.hook('app:mounted', () => {
    watch(
      activeId,
      (id) => {
        socket.switchNetwork(id)
      },
      { immediate: true },
    )
  })
})
