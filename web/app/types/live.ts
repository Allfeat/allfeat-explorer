// WebSocket wire protocol — hand-written mirror of src/live/protocol.rs.
// These types aren't domain types, so ts-rs doesn't emit them; keep them
// in sync with the Rust side when the protocol evolves. The `type`
// discriminator is snake_case (matches serde `rename_all = "snake_case"`),
// and Topic also uses the serialized snake_case form (`ats_feed`, not
// `atsFeed`) because that's what the server expects on the wire.

import type { AtsFeedItem, Block, Transfer } from '@bindings'

export type Topic = 'blocks' | 'transfers' | 'ats_feed'

export type ClientMsg =
  | { type: 'subscribe', topic: Topic }
  | { type: 'unsubscribe', topic: Topic }
  | { type: 'pong' }

export type ServerMsg =
  | { type: 'block', data: Block }
  | { type: 'transfer', data: Transfer }
  | { type: 'ats_item', data: AtsFeedItem }
  | { type: 'ping' }
  | { type: 'error', message: string }

export type ConnState = 'connecting' | 'connected' | 'reconnecting' | 'offline'

export const ALL_TOPICS: readonly Topic[] = ['blocks', 'transfers', 'ats_feed'] as const
