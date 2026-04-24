// WebSocket singleton for the live topics (blocks / transfers / ats_feed).
//
// One connection per tab, keyed on the active network id. Module-level
// state is deliberate: all composable callers share the same socket, the
// same reconnect budget, and the same subscription set. Calling
// `switchNetwork` tears down the current socket, purges the buffers, and
// opens a fresh session — we never multiplex two networks on one socket
// because the server keys its forwarders to a single NetworkSpec.
//
// Reconnect policy mirrors the plan: exponential backoff starting at
// 250 ms, capped at 5 s. Connection state is mirrored into the Pinia
// store so UI consumers can react without talking to this module.
//
// Dev notes: Nuxt/Nitro does not proxy WebSocket upgrades, so we target
// `runtimeConfig.public.wsBase` directly (see nuxt.config.ts). In prod
// the reverse proxy handles both HTTP and WS, so the same variable
// points at the public edge.

import type { ClientMsg, ServerMsg } from '~/types/live'
import { ALL_TOPICS } from '~/types/live'
import { useLiveStore } from '~/stores/live'

const INITIAL_RECONNECT_MS = 250
const MAX_RECONNECT_MS = 5000

// Module-level — shared across every useLiveSocket() caller in the tab.
let socket: WebSocket | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
let reconnectDelay = INITIAL_RECONNECT_MS
// The network id we *intend* to be connected to. Survives a close so the
// reconnect loop knows where to go back.
let targetNetworkId: string | null = null
// Distinguishes a deliberate close (switch / dispose) from an unexpected
// one so the close handler doesn't schedule a reconnect we don't want.
let closing = false

export function useLiveSocket() {
  const store = useLiveStore()
  const config = useRuntimeConfig()

  function open(networkId: string) {
    if (!import.meta.client) return

    closing = false
    targetNetworkId = networkId
    // Reconnecting if we already had a socket; else it's a cold start.
    store.setConnection(socket ? 'reconnecting' : 'connecting')

    const base = config.public.wsBase
    const url = `${base}/live?network=${encodeURIComponent(networkId)}`
    const ws = new WebSocket(url)
    socket = ws

    ws.addEventListener('open', () => {
      if (ws !== socket) return // stale socket (switched mid-handshake)
      reconnectDelay = INITIAL_RECONNECT_MS
      store.setConnection('connected')
      for (const topic of ALL_TOPICS) {
        sendOn(ws, { type: 'subscribe', topic })
      }
    })

    ws.addEventListener('message', (ev) => {
      if (ws !== socket) return
      const raw = typeof ev.data === 'string' ? ev.data : ''
      if (!raw) return
      let msg: ServerMsg
      try {
        msg = JSON.parse(raw) as ServerMsg
      }
      catch (err) {
        console.warn('[live] bad frame', err, raw)
        return
      }
      dispatch(ws, msg)
    })

    // 'error' fires before 'close' on most browsers; let close handle
    // the reconnect scheduling so we don't double-schedule.
    ws.addEventListener('error', () => {
      if (ws !== socket) return
      try { ws.close() }
      catch { /* already closed */ }
    })

    ws.addEventListener('close', () => {
      if (ws !== socket) return
      socket = null
      if (closing) {
        store.setConnection('offline')
        return
      }
      store.setConnection('reconnecting')
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null
        if (targetNetworkId && !closing) open(targetNetworkId)
      }, reconnectDelay)
      reconnectDelay = Math.min(reconnectDelay * 2, MAX_RECONNECT_MS)
    })
  }

  function dispatch(ws: WebSocket, msg: ServerMsg) {
    switch (msg.type) {
      case 'block':
        store.pushBlock(msg.data); return
      case 'transfer':
        store.pushTransfer(msg.data); return
      case 'ats_item':
        store.pushAtsItem(msg.data); return
      case 'ping':
        sendOn(ws, { type: 'pong' }); return
      case 'error':
        console.warn('[live] server error:', msg.message); return
    }
  }

  function sendOn(ws: WebSocket, msg: ClientMsg) {
    if (ws.readyState !== WebSocket.OPEN) return
    ws.send(JSON.stringify(msg))
  }

  // Swap connection to a new network (or null → tear down). Cheap no-op
  // when the id hasn't changed.
  function switchNetwork(networkId: string | null) {
    if (!import.meta.client) return

    if (networkId === null) {
      dispose()
      return
    }
    if (networkId === targetNetworkId && socket) return

    // First call after page load: the store is already seeded with SSR
    // data (blocks/transfers/atsFeed hydrated via Nuxt payload). We're
    // about to open the socket for the same network — keep that state
    // so the footer chip and dashboard panels don't flash empty before
    // the first live push lands.
    const isInitialOpen = targetNetworkId === null && socket === null

    // Cancel any pending reconnect for the old target.
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    // Close the current socket deliberately: the close handler will see
    // `closing = true` and *not* schedule a reconnect. We re-open below.
    if (socket) {
      closing = true
      try { socket.close() }
      catch { /* ignored */ }
      socket = null
    }

    if (!isInitialOpen) {
      store.clearLive()
    }
    reconnectDelay = INITIAL_RECONNECT_MS
    open(networkId)
  }

  function dispose() {
    closing = true
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (socket) {
      try { socket.close() }
      catch { /* ignored */ }
      socket = null
    }
    targetNetworkId = null
    store.setConnection('offline')
  }

  return { switchNetwork, dispose }
}

// HMR: Vite hot-reloads the module, which resets our `let` state but the
// browser keeps the existing WebSocket alive. Close it on dispose so we
// don't leak sockets across reloads — the plugin will re-open on the
// re-imported module.
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    closing = true
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (socket) {
      try { socket.close() }
      catch { /* ignored */ }
      socket = null
    }
  })
}
