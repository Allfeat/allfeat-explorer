// Reactive view of the live WebSocket connection state. Thin wrapper
// over the live store so components can read `connection.value` without
// importing Pinia boilerplate.

import { storeToRefs } from 'pinia'
import type { Ref } from 'vue'
import type { ConnState } from '~/types/live'
import { useLiveStore } from '~/stores/live'

export function useConnectionState(): Ref<ConnState> {
  const store = useLiveStore()
  const { connection } = storeToRefs(store)
  return connection
}
