//! WebSocket endpoint that multiplexes live topics onto one connection.
//!
//! One `GET /ws?network=<id>` upgrade per browser tab. The client drives the
//! session with `{"type":"subscribe", "topic":"..."}` frames; the server
//! spawns one forwarder task per active topic and fans the matching
//! `ChainData::subscribe_*` stream into the sink. Server heartbeats every
//! 30 s guard against stale connections behind proxies that silently drop
//! idle sockets.
//!
//! ## Concurrency shape
//!
//! Per connection we run three logical tasks joined by a
//! `tokio::sync::mpsc` sink:
//!
//! * **Receiver** — reads client frames, dispatches subscribe/unsubscribe to
//!   the forwarder map, replies to pings, exits on close.
//! * **Forwarders** (0..=3, one per topic) — each owns a boxed stream from
//!   the provider and pushes serialised `ServerMsg` frames into the sink.
//! * **Heartbeat** — pushes a `Ping` every 30 s.
//!
//! All three produce through the mpsc `sink_tx`; a single sink task owns
//! the split `SplitSink<WebSocket>` half so axum's sink doesn't need
//! external synchronization.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::data::{BoxStream, ChainData};
use crate::domain::{AtsFeedItem, Block, Transfer};
use crate::live::protocol::{ClientMsg, ServerMsg, Topic};
use crate::network::{by_id_or_default, ChainCtx, NetworkSpec};
use crate::server::AppState;

/// Heartbeat cadence. Browsers tolerate minutes of idleness, but some
/// enterprise proxies cut silent sockets at 60 s — 30 s stays comfortably
/// under that.
const HEARTBEAT: Duration = Duration::from_secs(30);

/// Bound for the sink channel. Every writer (forwarders + heartbeat +
/// receiver) pushes here; if the client is slow and the channel fills,
/// senders apply backpressure rather than unbounded buffering.
const SINK_CAPACITY: usize = 128;

/// Maximum consecutive dropped heartbeats before we give up on the
/// session. Three drops at 30 s cadence ≈ 90 s of sink backpressure —
/// enough to cover a transient GC pause, short of the point where a LB
/// would cut us anyway. Calibrate in tandem with `HEARTBEAT`.
const HEARTBEAT_DROP_LIMIT: u32 = 3;

/// Hard cap on a single inbound frame. The client protocol fits in well
/// under 100 bytes per message (`{"type":"subscribe","topic":"ats_feed"}`
/// is ~40), so 4 KiB leaves ample headroom without letting a hostile
/// client burn parse time on megabyte JSON blobs. Axum's default is 16 MB.
const MAX_INBOUND_FRAME_BYTES: usize = 4 * 1024;

/// Hard cap on a full inbound message (continuation frames concatenated).
/// Same reasoning as `MAX_INBOUND_FRAME_BYTES`; doubled so a pathological
/// client can't split a single bloated payload into a handful of frames
/// and still sneak past. Axum's default is 64 MB.
const MAX_INBOUND_MESSAGE_BYTES: usize = 8 * 1024;

/// Per-session inbound rate limit. Twenty frames per rolling second is
/// ~10x the steady-state demand of a real client (heartbeat pongs plus
/// the occasional subscribe). Exceeding it closes the session with an
/// error frame — we'd rather drop a misbehaving client than let them burn
/// CPU parsing JSON on our box.
const MAX_INBOUND_FRAMES_PER_SEC: u32 = 20;

/// Absolute ceiling on subscribe operations over the life of a session.
/// Once the client has churned through this many subscribe dispatches we
/// close the connection: nobody flipping tabs reaches three digits of
/// subscribes, but a sub/unsub-in-a-loop attack easily would.
const MAX_SUBSCRIBES_PER_SESSION: u32 = 128;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Network id (`?network=allfeat`). Falls back to the default when
    /// missing or unknown — matches the rest of the app, which also
    /// silently snaps unknown ids to the default network.
    network: Option<String>,
}

/// Axum handler: upgrades the HTTP request to a WebSocket and delegates to
/// [`run_session`] once the handshake succeeds. Cheap by itself — all the
/// real work lives in the spawned session.
///
/// Enforces the origin allowlist before the upgrade: browsers always send an
/// `Origin` header on WS handshakes, and a mismatch (or missing header when
/// the allowlist is non-wildcard) earns a `403 Forbidden` with no upgrade.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<WsQuery>,
) -> Response {
    if !origin_allowed(&headers, &state.config.ws_allowed_origins) {
        let origin = headers
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("<missing>");
        tracing::warn!(%origin, "ws upgrade rejected: origin not allowed");
        return StatusCode::FORBIDDEN.into_response();
    }
    let spec = match q.network.as_deref() {
        Some(id) => by_id_or_default(id),
        None => crate::network::DEFAULT_NETWORK,
    };
    // Tight caps on inbound size so a hostile client can't force us to
    // parse megabytes of JSON per frame. Defaults (16 MB / 64 MB) are
    // laughably permissive for a protocol whose largest legitimate
    // message is well under 100 bytes.
    ws.max_frame_size(MAX_INBOUND_FRAME_BYTES)
        .max_message_size(MAX_INBOUND_MESSAGE_BYTES)
        .on_upgrade(move |socket| run_session(socket, state, spec))
}

/// Decide whether the incoming handshake's `Origin` header passes the
/// allowlist. Wildcard short-circuits (dev default). Missing or unreadable
/// `Origin` is rejected under a strict allowlist — browsers always send one
/// on a same-origin or cross-origin WS upgrade, so the only clients missing
/// it are non-browser scrapers we'd rather block.
fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    if allowed.iter().any(|s| s == "*") {
        return true;
    }
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    allowed.iter().any(|a| a == origin)
}

/// Drive a single WS connection until the browser closes it or a fatal
/// error fires. Spawns the sink task + receiver + heartbeat, waits for the
/// receiver to exit, then tears everything down.
async fn run_session(socket: WebSocket, state: AppState, spec: &'static NetworkSpec) {
    let (ws_tx, ws_rx) = socket.split();
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(SINK_CAPACITY);

    // Sink task: owns the write half. Every other task pushes messages
    // through `sink_tx`; this loop drains them serially.
    let sink_handle = tokio::spawn(run_sink(ws_tx, sink_rx));

    // Heartbeat task: periodic server-side ping so proxies don't cull the
    // socket. Uses `try_send` rather than `send().await` so a slow client
    // can never stall the heartbeat behind a full sink — we'd rather drop
    // a ping than let the LB decide the connection is dead. Three
    // consecutive drops in a row close the session: if the client is that
    // far behind, they're unreachable in practice.
    let heartbeat_tx = sink_tx.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HEARTBEAT);
        // First tick fires immediately — skip it so we don't spam the
        // client during handshake.
        ticker.tick().await;
        let mut consecutive_drops: u32 = 0;
        loop {
            ticker.tick().await;
            let msg = encode(&ServerMsg::Ping);
            match heartbeat_tx.try_send(msg) {
                Ok(()) => consecutive_drops = 0,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    consecutive_drops += 1;
                    tracing::debug!(consecutive_drops, "ws sink full, dropping heartbeat tick");
                    if consecutive_drops >= HEARTBEAT_DROP_LIMIT {
                        tracing::warn!(
                            consecutive_drops,
                            "ws heartbeat drop limit reached, closing session"
                        );
                        return; // sink_tx drop signals sink task → session closes
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
    });

    // Receiver loop: runs inline so we exit `run_session` as soon as the
    // client disconnects. On exit we abort the sink + heartbeat + any
    // remaining forwarders.
    let mut forwarders: HashMap<Topic, JoinHandle<()>> = HashMap::new();
    run_receiver(ws_rx, sink_tx.clone(), state, spec, &mut forwarders).await;

    // Tidy up in a deterministic order: stop producers first so they
    // don't try to push to a dead sink, then close the sink itself.
    for (_, h) in forwarders.drain() {
        h.abort();
    }
    heartbeat_handle.abort();
    // Dropping `sink_tx` (the last copy owned here) closes the channel,
    // letting the sink task return gracefully after draining pending
    // frames.
    drop(sink_tx);
    let _ = sink_handle.await;
}

/// Drain the mpsc into the WS write half. Exits when the channel closes
/// (all senders dropped) or the remote side errors.
async fn run_sink(mut ws_tx: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Message>) {
    while let Some(msg) = rx.recv().await {
        if ws_tx.send(msg).await.is_err() {
            return;
        }
    }
    let _ = ws_tx.close().await;
}

/// Handle client-originated frames: dispatch subscribe/unsubscribe to
/// forwarder tasks, ignore or reply to protocol frames, drop malformed
/// text with a best-effort `Error` push so the client knows to stop
/// resending.
///
/// Two budget-based guards flank the dispatch:
///
/// * A rolling 1-second frame budget ([`MAX_INBOUND_FRAMES_PER_SEC`])
///   closes the session if a client floods us. We notify via a final
///   `Error` frame so the client can distinguish rate-limit from a bare
///   disconnect.
/// * A session-lifetime subscribe counter ([`MAX_SUBSCRIBES_PER_SESSION`])
///   catches sub/unsub-in-a-loop patterns that would otherwise never
///   trigger the per-second guard (one op every ~100 ms fits comfortably
///   under 20/sec).
async fn run_receiver(
    mut ws_rx: SplitStream<WebSocket>,
    sink_tx: mpsc::Sender<Message>,
    state: AppState,
    spec: &'static NetworkSpec,
    forwarders: &mut HashMap<Topic, JoinHandle<()>>,
) {
    let mut window_start = Instant::now();
    let mut frame_budget = MAX_INBOUND_FRAMES_PER_SEC;
    let mut subscribes_ever: u32 = 0;

    while let Some(frame) = ws_rx.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(_) => return,
        };

        // Refill budget every second. Counting every inbound frame
        // (including pongs and control frames) is deliberate: a spammer
        // can just pick the cheapest frame type, so we care about raw
        // frame rate, not payload shape.
        if window_start.elapsed() >= Duration::from_secs(1) {
            window_start = Instant::now();
            frame_budget = MAX_INBOUND_FRAMES_PER_SEC;
        }
        if frame_budget == 0 {
            tracing::warn!("ws inbound rate limit exceeded; closing session");
            let _ = sink_tx
                .send(encode(&ServerMsg::Error {
                    message: "inbound rate limit exceeded".into(),
                }))
                .await;
            return;
        }
        frame_budget -= 1;

        match frame {
            Message::Text(text) => {
                let parsed: Result<ClientMsg, _> = serde_json::from_str(&text);
                match parsed {
                    Ok(ClientMsg::Subscribe { topic }) => {
                        subscribes_ever = subscribes_ever.saturating_add(1);
                        if subscribes_ever > MAX_SUBSCRIBES_PER_SESSION {
                            tracing::warn!(
                                subscribes_ever,
                                "ws subscribe churn limit exceeded; closing session"
                            );
                            let _ = sink_tx
                                .send(encode(&ServerMsg::Error {
                                    message: "subscribe churn limit exceeded".into(),
                                }))
                                .await;
                            return;
                        }
                        if forwarders.contains_key(&topic) {
                            continue; // already subscribed, no-op
                        }
                        match spawn_forwarder(topic, state.provider.clone(), spec, sink_tx.clone())
                            .await
                        {
                            Ok(handle) => {
                                forwarders.insert(topic, handle);
                            }
                            Err(msg) => {
                                let _ = sink_tx
                                    .send(encode(&ServerMsg::Error { message: msg }))
                                    .await;
                            }
                        }
                    }
                    Ok(ClientMsg::Unsubscribe { topic }) => {
                        if let Some(h) = forwarders.remove(&topic) {
                            h.abort();
                        }
                    }
                    Ok(ClientMsg::Pong) => { /* liveness reply; nothing to do */ }
                    Err(e) => {
                        let _ = sink_tx
                            .send(encode(&ServerMsg::Error {
                                message: format!("bad client frame: {e}"),
                            }))
                            .await;
                    }
                }
            }
            Message::Binary(_) => {
                let _ = sink_tx
                    .send(encode(&ServerMsg::Error {
                        message: "binary frames are not supported".into(),
                    }))
                    .await;
            }
            Message::Ping(payload) => {
                // axum tungstenite auto-replies to WS-level ping with pong,
                // but forwarding through the sink would race with that
                // reply. Nothing to do.
                let _ = payload;
            }
            Message::Pong(_) => { /* heartbeat liveness; not used */ }
            Message::Close(_) => return,
        }
    }
}

/// Open the right provider stream for a topic and spawn a task that
/// forwards every item into the sink. Returns the handle so the session
/// can abort it on unsubscribe / close. Provider errors at subscribe time
/// bubble back as an [`Error`] frame — the connection stays usable.
async fn spawn_forwarder(
    topic: Topic,
    provider: Arc<dyn ChainData>,
    spec: &'static NetworkSpec,
    sink_tx: mpsc::Sender<Message>,
) -> Result<JoinHandle<()>, String> {
    // `now_ms` only matters for mock-mode context derivation inside the
    // producer; the stream itself is wall-clock driven. Passing 0 keeps
    // the RPC path happy (it ignores now_ms) and the mock producer has
    // its own clock.
    let ctx = ChainCtx::new(spec, 0);
    match topic {
        Topic::Blocks => {
            let stream = provider
                .subscribe_blocks(ctx)
                .await
                .map_err(|e| format!("subscribe blocks: {e}"))?;
            Ok(tokio::spawn(forward::<Block>(stream, sink_tx, topic)))
        }
        Topic::Transfers => {
            let stream = provider
                .subscribe_transfers(ctx)
                .await
                .map_err(|e| format!("subscribe transfers: {e}"))?;
            Ok(tokio::spawn(forward::<Transfer>(stream, sink_tx, topic)))
        }
        Topic::AtsFeed => {
            let stream = provider
                .subscribe_ats_feed(ctx)
                .await
                .map_err(|e| format!("subscribe ats_feed: {e}"))?;
            Ok(tokio::spawn(forward::<AtsFeedItem>(stream, sink_tx, topic)))
        }
    }
}

/// Generic forwarder: pumps a provider stream into the sink as
/// `ServerMsg`. Exits when the source stream ends (provider side dead) or
/// the sink is closed (session winding down).
async fn forward<T>(mut stream: BoxStream<T>, sink_tx: mpsc::Sender<Message>, topic: Topic)
where
    T: 'static + Send + Into<Payload>,
{
    while let Some(item) = stream.next().await {
        let msg = match item.into() {
            Payload::Block(b) => ServerMsg::Block { data: b },
            Payload::Transfer(t) => ServerMsg::Transfer { data: t },
            Payload::AtsItem(i) => ServerMsg::AtsItem { data: i },
        };
        if sink_tx.send(encode(&msg)).await.is_err() {
            return;
        }
    }
    tracing::debug!(?topic, "forwarder: provider stream ended");
}

/// Tagged union so one generic `forward` function can cover all three
/// topics without a macro. The `Into` bounds let the item type pick its
/// variant at the call site.
enum Payload {
    Block(Block),
    Transfer(Transfer),
    AtsItem(AtsFeedItem),
}

impl From<Block> for Payload {
    fn from(v: Block) -> Self {
        Payload::Block(v)
    }
}

impl From<Transfer> for Payload {
    fn from(v: Transfer) -> Self {
        Payload::Transfer(v)
    }
}

impl From<AtsFeedItem> for Payload {
    fn from(v: AtsFeedItem) -> Self {
        Payload::AtsItem(v)
    }
}

fn encode(msg: &ServerMsg) -> Message {
    // Serialising a tagged enum with owned strings cannot fail; unwrap is
    // safe here. If it ever does (new variant with a non-Serialize field),
    // a failing test in `protocol.rs` catches it.
    let text = serde_json::to_string(msg).expect("ServerMsg must always serialise");
    Message::Text(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_origin(origin: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::ORIGIN, origin.parse().unwrap());
        h
    }

    #[test]
    fn wildcard_accepts_any_origin() {
        let allowed = vec!["*".to_string()];
        assert!(origin_allowed(
            &headers_with_origin("https://evil.example"),
            &allowed
        ));
        assert!(origin_allowed(&HeaderMap::new(), &allowed));
    }

    #[test]
    fn exact_match_accepted() {
        let allowed = vec!["https://explorer.allfeat.com".to_string()];
        assert!(origin_allowed(
            &headers_with_origin("https://explorer.allfeat.com"),
            &allowed
        ));
    }

    #[test]
    fn mismatch_rejected() {
        let allowed = vec!["https://explorer.allfeat.com".to_string()];
        assert!(!origin_allowed(
            &headers_with_origin("https://evil.example"),
            &allowed
        ));
    }

    #[test]
    fn missing_origin_rejected_under_strict_allowlist() {
        let allowed = vec!["https://explorer.allfeat.com".to_string()];
        assert!(!origin_allowed(&HeaderMap::new(), &allowed));
    }

    #[test]
    fn multi_host_allowlist() {
        let allowed = vec![
            "https://explorer.allfeat.com".to_string(),
            "https://staging.explorer.allfeat.com".to_string(),
        ];
        assert!(origin_allowed(
            &headers_with_origin("https://staging.explorer.allfeat.com"),
            &allowed
        ));
        assert!(!origin_allowed(
            &headers_with_origin("https://other.example"),
            &allowed
        ));
    }
}
