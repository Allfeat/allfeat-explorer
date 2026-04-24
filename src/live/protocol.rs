//! Wire protocol for the live-update WebSocket.
//!
//! JSON text frames, one message per frame. Both sides share these types so
//! the contract lives in one place; if the shape changes, both SSR and
//! hydrate fail to build together.
//!
//! ## Framing
//!
//! Client → server:
//!
//! ```json
//! {"type":"subscribe","topic":"blocks"}
//! {"type":"unsubscribe","topic":"transfers"}
//! {"type":"pong"}
//! ```
//!
//! Server → client:
//!
//! ```json
//! {"type":"block","data": <Block>}
//! {"type":"transfer","data": <Transfer>}
//! {"type":"ats_item","data": <AtsFeedItem>}
//! {"type":"ping"}
//! {"type":"error","message":"..."}
//! ```
//!
//! The `type` discriminator is `rename_all = "snake_case"`, so the hand-
//! written JSON above round-trips through `serde_json` without glue.

use serde::{Deserialize, Serialize};

use crate::domain::{AtsFeedItem, Block, Transfer};

/// Topics the browser can subscribe to. Each topic maps to one server-side
/// broadcast channel per network. Extending the set means adding a variant
/// and a matching producer task — the client side uses the serialized
/// wire name, so `snake_case` is the authoritative spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Blocks,
    Transfers,
    AtsFeed,
}

impl Topic {
    pub const ALL: &'static [Topic] = &[Topic::Blocks, Topic::Transfers, Topic::AtsFeed];

    pub fn as_str(self) -> &'static str {
        match self {
            Topic::Blocks => "blocks",
            Topic::Transfers => "transfers",
            Topic::AtsFeed => "ats_feed",
        }
    }
}

/// Messages the browser sends to the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Start receiving a topic. Re-subscribing is a no-op (the server
    /// tracks active topics per connection).
    Subscribe { topic: Topic },
    /// Stop receiving a topic. The server drops the forwarder task.
    Unsubscribe { topic: Topic },
    /// Liveness reply to the server's heartbeat. The server doesn't
    /// actually require it today (it relies on WS-level close detection)
    /// but the round-trip keeps intermediaries from idling the connection.
    Pong,
}

/// Messages the server pushes to the browser.
///
/// Payloads are flattened under `data` so `match`-ing on `type` in TypeScript
/// (should a non-Rust client ever need to read the stream) stays readable.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Block {
        data: Block,
    },
    Transfer {
        data: Transfer,
    },
    AtsItem {
        data: AtsFeedItem,
    },
    /// Heartbeat. Clients may reply with [`ClientMsg::Pong`] but the server
    /// never blocks on the reply.
    Ping,
    /// Something went wrong applying a subscribe / feeding a topic. Not
    /// fatal — the connection stays open, other topics keep streaming.
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format is the public contract — if these snapshots break
    /// we're changing the protocol, and the client side has to change with
    /// the server side.
    #[test]
    fn client_subscribe_roundtrips() {
        let msg = ClientMsg::Subscribe {
            topic: Topic::Blocks,
        };
        let s = serde_json::to_string(&msg).unwrap();
        assert_eq!(s, r#"{"type":"subscribe","topic":"blocks"}"#);
        let back: ClientMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn topics_use_snake_case_on_the_wire() {
        let s = serde_json::to_string(&Topic::AtsFeed).unwrap();
        assert_eq!(s, "\"ats_feed\"");
    }

    #[test]
    fn server_ping_is_compact() {
        let s = serde_json::to_string(&ServerMsg::Ping).unwrap();
        assert_eq!(s, r#"{"type":"ping"}"#);
    }
}
