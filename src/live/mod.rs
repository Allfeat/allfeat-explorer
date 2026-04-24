//! Live-update plumbing: shared protocol + server handler.
//!
//! The live stream is a single multiplexed WebSocket connection per tab, fed
//! by a per-network producer task on the server. The browser subscribes to
//! topics (blocks, transfers, ATS feed) over one socket and the server
//! fans-out the matching broadcast into the sink. The browser-side client
//! lives in the Nuxt app (`web/app/composables/useLiveSocket.ts`) — this
//! crate only owns the server half and the shared wire types.
//!
//! * [`protocol`] — message types on the wire. Derives `Serialize` +
//!   `Deserialize` + (behind `ts-bindings`) `TS`, so the TypeScript client
//!   consumes the exact same shape the server emits.
//! * [`server`] — Axum handler, broadcast fan-out, heartbeat. `ssr` only.
//! * [`merge`] — dedup/cap helper shared with tests and any future native
//!   consumer. Pure domain logic, no transport dependencies.

pub mod merge;
pub mod protocol;

#[cfg(feature = "ssr")]
pub mod encoder;
#[cfg(feature = "ssr")]
pub mod server;

pub use merge::{merge_live, LiveItem};
pub use protocol::{ClientMsg, ServerMsg, Topic};
