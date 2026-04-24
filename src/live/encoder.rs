//! Per-(network, topic) shared encoder/fanout for the live WebSocket.
//!
//! Without the hub, every session's forwarder pulls the same `Block` /
//! `Transfer` / `AtsFeedItem` off the provider broadcast and re-runs
//! `serde_json::to_string` on it — N subscribers ⇒ N identical encode
//! passes per event. The hub inserts one task per (network, topic) combo
//! that pulls from the provider once, encodes the `ServerMsg` once, and
//! rebroadcasts the finished [`Message`] to all session receivers, so
//! sessions just pump pre-encoded frames into their sink.
//!
//! Lifetime: each subscriber holds an [`Arc<EncoderEntry>`] lease. When
//! the last Arc drops, [`EncoderEntry::drop`] aborts the encoder task,
//! which in turn drops the provider subscription. The per-network
//! enrichment gate on `RpcClient` (`receiver_count > 0`) re-arms during
//! idle periods, so an explorer with no live viewers doesn't pay the
//! extra RPC cost per block.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use axum::extract::ws::Message;
use futures::StreamExt;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::data::{BoxStream, ChainData};
use crate::domain::{AtsFeedItem, Block, Transfer};
use crate::live::protocol::{ServerMsg, Topic};
use crate::network::{ChainCtx, NetworkSpec};

/// Ring-buffer size for the encoded broadcast. Matches the backpressure
/// profile of the session sink: either the client drains in time, or
/// it's already on track to be culled by the heartbeat drop limit.
const ENCODED_CAP: usize = 256;

type Key = (&'static str, Topic);

/// Process-wide registry of encoder tasks. One instance in [`AppState`];
/// sessions look up the right entry via [`EncoderHub::subscribe`].
///
/// [`AppState`]: crate::server::AppState
pub struct EncoderHub {
    inner: Mutex<HashMap<Key, Weak<EncoderEntry>>>,
}

impl EncoderHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Return a shared lease plus a fresh receiver for `(spec, topic)`.
    ///
    /// The first caller for a given key opens a provider stream and
    /// spawns the encoder task; subsequent callers reuse the entry
    /// while at least one `Arc<EncoderEntry>` is still live. The
    /// returned lease must be held by the caller (typically in the
    /// session's forwarder task) for as long as it wants messages —
    /// dropping the last Arc tears the encoder down.
    pub async fn subscribe(
        &self,
        provider: Arc<dyn ChainData>,
        spec: &'static NetworkSpec,
        topic: Topic,
    ) -> Result<(Arc<EncoderEntry>, broadcast::Receiver<Message>), String> {
        let key = (spec.id, topic);
        let mut map = self.inner.lock().await;
        if let Some(weak) = map.get(&key) {
            if let Some(entry) = weak.upgrade() {
                let rx = entry.tx.subscribe();
                return Ok((entry, rx));
            }
        }

        // `ChainCtx::now_ms` is only consulted by the mock producer for
        // deterministic derivation — the RPC path ignores it and the
        // encoder never reads it directly, so 0 is fine here (matches
        // the pre-hub call site in `server.rs`).
        let ctx = ChainCtx::new(spec, 0);
        let (tx, rx) = broadcast::channel::<Message>(ENCODED_CAP);
        let tx_task = tx.clone();
        let handle = match topic {
            Topic::Blocks => {
                let stream = provider
                    .subscribe_blocks(ctx)
                    .await
                    .map_err(|e| format!("subscribe blocks: {e}"))?;
                tokio::spawn(run_encoder(stream, tx_task, topic, wrap_block))
            }
            Topic::Transfers => {
                let stream = provider
                    .subscribe_transfers(ctx)
                    .await
                    .map_err(|e| format!("subscribe transfers: {e}"))?;
                tokio::spawn(run_encoder(stream, tx_task, topic, wrap_transfer))
            }
            Topic::AtsFeed => {
                let stream = provider
                    .subscribe_ats_feed(ctx)
                    .await
                    .map_err(|e| format!("subscribe ats_feed: {e}"))?;
                tokio::spawn(run_encoder(stream, tx_task, topic, wrap_ats_item))
            }
        };
        let entry = Arc::new(EncoderEntry {
            tx,
            task: std::sync::Mutex::new(Some(handle)),
        });
        map.insert(key, Arc::downgrade(&entry));
        Ok((entry, rx))
    }
}

/// Lease + broadcast endpoint for one `(network, topic)` encoder task.
/// Sessions keep one of these alive alongside their forwarder task;
/// dropping the last Arc aborts the task via [`Drop`].
pub struct EncoderEntry {
    tx: broadcast::Sender<Message>,
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Drop for EncoderEntry {
    fn drop(&mut self) {
        if let Some(handle) = self.task.lock().unwrap().take() {
            handle.abort();
        }
    }
}

/// Pump a provider stream, wrap each item through `wrap`, serialise it
/// once, and rebroadcast the finished [`Message`]. Exits when the source
/// stream ends (provider disconnect) or when the task is aborted from
/// [`EncoderEntry::drop`].
async fn run_encoder<T, F>(
    mut stream: BoxStream<T>,
    tx: broadcast::Sender<Message>,
    topic: Topic,
    wrap: F,
) where
    T: 'static + Send,
    F: Fn(T) -> ServerMsg + Send + 'static,
{
    while let Some(item) = stream.next().await {
        let msg = wrap(item);
        let encoded = encode_message(&msg);
        // Send errors just mean no live receivers at the moment — the
        // next item either finds a fresh subscriber, or the task gets
        // aborted once the last `EncoderEntry` drops.
        let _ = tx.send(encoded);
    }
    tracing::debug!(?topic, "encoder stream ended");
}

fn encode_message(msg: &ServerMsg) -> Message {
    // Matches the invariant in `live/server.rs::encode`: every
    // `ServerMsg` variant is serialisable; a future variant that isn't
    // would break the `protocol.rs` round-trip test before reaching
    // production.
    let text = serde_json::to_string(msg).expect("ServerMsg must always serialise");
    Message::Text(text.into())
}

fn wrap_block(data: Block) -> ServerMsg {
    ServerMsg::Block { data }
}

fn wrap_transfer(data: Transfer) -> ServerMsg {
    ServerMsg::Transfer { data }
}

fn wrap_ats_item(data: AtsFeedItem) -> ServerMsg {
    ServerMsg::AtsItem { data }
}
