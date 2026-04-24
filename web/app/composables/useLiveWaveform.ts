// Seed + live view of the home-page waveform window.
//
// Hits the lean /waveform endpoint instead of /blocks so the SSR seed
// payload stays small (~3 kB instead of ~250 kB for the same 72-bar
// window). Live updates piggy-back on the existing Block WS push: the
// store's pushBlock action mirrors each frame into waveformBlocks so
// this composable never needs its own subscription.

import { storeToRefs } from 'pinia'
import type { WaveformBlock } from '@bindings'
import { useLiveStore } from '~/stores/live'

export interface UseLiveWaveformOptions {
  /** Initial seed size. Defaults to 72 (matches the hero's bar count). */
  count?: number
}

export function useLiveWaveform(options: UseLiveWaveformOptions = {}) {
  const store = useLiveStore()
  const { waveformBlocks } = storeToRefs(store)

  const { pending, error } = useLiveSeed<WaveformBlock[], WaveformBlock>({
    keyPrefix: 'live-waveform',
    path: '/waveform',
    count: options.count ?? 72,
    buffer: waveformBlocks,
    empty: () => [],
    extractItems: items => items,
    seed: items => store.seedWaveform(items),
  })

  return { blocks: waveformBlocks, pending, error }
}
