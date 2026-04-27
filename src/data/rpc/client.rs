//! Per-network subxt client wrapper with documented scaling policy.
//!
//! One [`RpcClient`] per configured network, built once at boot and shared
//! across requests via `Arc`. The inner `OnlineClient` is already Arc-wrapped
//! internally by subxt, so its clones are cheap and the same connection is
//! reused for every query — never open one per request.
//!
//! ## Scaling layers
//!
//! (1) **Shared client** — this struct. The inner `OnlineClient` sits behind
//!     an `RwLock<Option<…>>` so the connection can be re-established from a
//!     shared `&self`: transient WebSocket failures invalidate the slot and
//!     the next caller re-connects. `subxt()` retries once on
//!     [`DataError::Transport`] — a single hiccup during the handshake
//!     doesn't take the server down until operators intervene.
//!
//! (2) **Hot + finalized caches** — see [`super::cache`] for the full
//!     policy. Each `ChainData` method consults a `moka::future::Cache` keyed
//!     by its inputs before dispatching to subxt, with `try_get_with`
//!     coalescing concurrent misses so a 100-user dashboard burst still maps
//!     to a single RPC call per key.
//!
//! (3) **Live finalized-head watch** — on the first successful connect we
//!     spawn a background task that subscribes to finalized headers and
//!     publishes the head number to a [`watch`] channel. Any hot path that
//!     needs the finalized head reads the channel in O(1) instead of
//!     round-tripping `at_current_block`. A stale channel (connection
//!     dropped) transparently falls back to the RPC path.
//!
//! See `paritytech/subxt` `subxt/examples/` for the canonical subxt API
//! patterns used by this module (connection, historic blocks, subscriptions).

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use blake2::digest::consts::U32;
use blake2::Digest;
use subxt::client::OnlineClientAtBlock;
use subxt::ext::codec;
use subxt::ext::codec::Decode;
use subxt::{OnlineClient, SubstrateConfig};
use tokio::sync::{broadcast, watch, RwLock};

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::mappers::hex_bytes;
use crate::domain::{AtsFeedItem, Block, RuntimeCodeInfo, RuntimeIdentity, Transfer};
use crate::network::RuntimeKind;

use super::cache::Caches;
use super::mappers;

/// Substrate's canonical blake2 variant: Blake2b truncated to 256 bits.
/// Matches `sp_core::hashing::blake2_256`, which the runtime itself uses
/// when addressing `:code` and when hashing block headers — keeping the
/// same hasher here lets the UI show a hash that matches polkadot.js.
type Blake2b256 = blake2::Blake2b<U32>;

/// zstd magic prefix. Substrate wraps `:code` in zstd whenever the raw
/// blob would otherwise exceed the `CODE_BLOB_BOMB_LIMIT`; detecting the
/// prefix here is enough to surface "compressed: true" in the UI.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Per-topic broadcast capacity. Sized so a browser tab that backgrounds
/// for half a minute at a 3s cadence doesn't force a lag skip — bigger
/// than strictly necessary is cheap (single clones of small domain types)
/// and smaller loses updates on legitimate stalls.
const BROADCAST_CAPACITY: usize = 64;

/// Default budget for a single RPC round-trip. Generous enough that a slow
/// node under load still answers, short enough that a hung WebSocket
/// surfaces as an error before the browser's own loader times out.
pub const RPC_CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Budget for a `OnlineClient::from_url` handshake. Stricter than the
/// per-call budget: a handshake that hasn't completed in 5 s is almost
/// certainly stuck on TCP/TLS and retrying is cheaper than waiting.
pub const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Budget for storage iterations (e.g. walking `System::Account`). A
/// 10k-entry scan at ~5 ms/key already approaches a minute on a loaded
/// node; we set the ceiling slightly above what a healthy scan needs so a
/// stuck iterator still fails the request.
pub const RPC_ITER_TIMEOUT: Duration = Duration::from_secs(60);

/// Dispatch the subxt handshake based on the URL scheme. `from_insecure_url`
/// is required for plaintext `ws://` / `http://` endpoints — subxt rejects
/// them from `from_url` by default. In-cluster traffic (ClusterIP, no TLS
/// terminator on a hop that never leaves the pod network) opts in here;
/// `wss://` / `https://` keep the stricter path and its safety net against
/// accidental plaintext connections to public nodes.
async fn connect_client(
    endpoint: &str,
) -> Result<SubxtClient, subxt::error::OnlineClientError> {
    if endpoint.starts_with("ws://") || endpoint.starts_with("http://") {
        SubxtClient::from_insecure_url(endpoint).await
    } else {
        SubxtClient::from_url(endpoint).await
    }
}

/// Wrap an RPC future with [`RPC_CALL_TIMEOUT`]. A timeout is routed to
/// [`DataError::Transport`] so [`RpcClient::subxt`]'s retry-on-transport
/// path can recover from a mid-call freeze without the caller having to
/// invalidate manually.
pub async fn with_timeout<F, T>(op: &'static str, fut: F) -> DataResult<T>
where
    F: Future<Output = DataResult<T>>,
{
    match tokio::time::timeout(RPC_CALL_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(DataError::Transport(format!(
            "{op} timed out after {RPC_CALL_TIMEOUT:?}"
        ))),
    }
}

/// Longer-budget variant for storage iterations. Same timeout→`Transport`
/// mapping as [`with_timeout`] so a retrying caller still triggers the
/// reconnect path.
pub async fn with_iter_timeout<F, T>(op: &'static str, fut: F) -> DataResult<T>
where
    F: Future<Output = DataResult<T>>,
{
    match tokio::time::timeout(RPC_ITER_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(DataError::Transport(format!(
            "{op} timed out after {RPC_ITER_TIMEOUT:?}"
        ))),
    }
}

/// Race any future against [`RPC_CALL_TIMEOUT`] without constraining its
/// output type. Used where the raw subxt error still needs to be matched
/// downstream (e.g. `is_block_not_found`) — returning [`DataResult<T>`]
/// would force a lossy error conversion at the call site.
pub async fn race_timeout<F, T>(op: &'static str, fut: F) -> DataResult<T>
where
    F: Future<Output = T>,
{
    match tokio::time::timeout(RPC_CALL_TIMEOUT, fut).await {
        Ok(v) => Ok(v),
        Err(_) => Err(DataError::Transport(format!(
            "{op} timed out after {RPC_CALL_TIMEOUT:?}"
        ))),
    }
}

// `SubstrateConfig` matches both supported runtimes (Allfeat mainnet and
// Melodie testnet): same `AccountId32`, `MultiAddress`, `MultiSignature`,
// `u32` block numbers, `BlakeTwo256`. Only the codegen `runtime_types::*`
// differ per chain — the connection layer is uniform. If a future runtime
// diverges on the `Config` axis, switch to a custom `subxt::Config` impl
// here and everything downstream picks up the new types through the
// `subxt()` accessor.
pub type SubxtClient = OnlineClient<SubstrateConfig>;

pub struct RpcClient {
    endpoint: String,
    /// Shared with the finalized-head watcher task so both the main
    /// caller path and the watcher see the same connection slot: a
    /// watcher-driven invalidation (stream error) is immediately visible
    /// to the next `subxt()` caller, and conversely a manual `invalidate()`
    /// forces the watcher to reconnect on its next iteration.
    inner: Arc<RwLock<Option<SubxtClient>>>,
    caches: Caches,
    /// Latest finalized block number observed by the subscription task.
    /// `None` until the first notification arrives (fresh connect, or right
    /// after an `invalidate`); consumers fall back to `at_current_block`
    /// during that window.
    finalized_head_tx: watch::Sender<Option<u64>>,
    finalized_head_rx: watch::Receiver<Option<u64>>,
    /// Latest best (non-finalized) block number observed by the best-block
    /// subscription. Sits one or two blocks above `finalized_head_rx` during
    /// normal operation; consumers that need the true chain tip (block-list
    /// pagination, block-by-number lookups) read this instead of the
    /// finalized head so pending blocks are reachable.
    best_head_tx: watch::Sender<Option<u64>>,
    best_head_rx: watch::Receiver<Option<u64>>,
    /// Live-stream fan-out. Populated by the same watch task once it starts
    /// receiving finalized headers; each WS client gets a `Receiver` via
    /// `subscribe_*`. Enrichment is gated on `receiver_count() > 0` so an
    /// idle explorer doesn't pay a handful of RPC calls per block for
    /// nobody.
    blocks_tx: broadcast::Sender<Block>,
    transfers_tx: broadcast::Sender<Transfer>,
    ats_feed_tx: broadcast::Sender<AtsFeedItem>,
    /// Guard against double-spawn. Set to `true` when the watcher loop is
    /// running, cleared when it exits. `ensure_watcher_running` CAS's from
    /// `false` → `true` so concurrent `subxt()` callers racing after a
    /// disconnect can't spawn two supervisors.
    watcher_alive: Arc<AtomicBool>,
    /// Chain-derived block time in seconds, cached after the first read of
    /// `Timestamp::MinimumPeriod`. Substrate convention: a block is produced
    /// every `2 × MinimumPeriod`. Cleared on `invalidate` so a reconnect
    /// against a runtime-upgraded node re-reads fresh metadata.
    block_time_secs_cache: Arc<RwLock<Option<u64>>>,
    /// Full runtime identity read from the chain's `Core_version` API
    /// (spec/impl names, every numeric field in `sp_version::RuntimeVersion`).
    /// Cached after first fetch and cleared on `invalidate` so a runtime
    /// upgrade picks up fresh values on reconnect. Cloning out of the lock
    /// is cheap — the struct is a handful of `String`s and `u32`s.
    runtime_identity_cache: Arc<RwLock<Option<RuntimeIdentity>>>,
    /// `:code` fingerprint at the finalized head: blake2_256 hash +
    /// byte length + zstd-compression flag. Cleared on `invalidate`
    /// so a runtime upgrade re-fetches fresh values. Historical reads
    /// (via `at_block`) go straight through without touching this
    /// cache — the runtime page is the only caller and its "at block"
    /// form paginates by hand.
    runtime_code_cache: Arc<RwLock<Option<RuntimeCodeInfo>>>,
    /// SS58 address-format prefix, seeded from
    /// [`crate::network::NetworkSpec::ss58_prefix`]. Hardcoded per network —
    /// we intentionally don't read `ss58Format` from `system_properties` at
    /// runtime: the value is constant for a chain's lifetime, dev chains
    /// often skip the `properties` block entirely, and one less RPC round
    /// trip keeps the first-paint path simpler.
    ss58_prefix: u16,
    /// Network identifier this client serves (`NetworkSpec::id`). The
    /// metadata-decoder API in [`crate::data::metadata`] keys per-network
    /// blobs by this id, so the watcher task — which doesn't carry a
    /// `ChainCtx` — can still resolve the correct runtime bundle from
    /// the client itself.
    network_id: &'static str,
    /// Codegen runtime module this client reads through, seeded from
    /// [`crate::network::NetworkSpec::runtime_kind`]. Mappers and
    /// projections that need to address `runtime::allfeat::*` vs
    /// `runtime::melodie::*` dispatch on it via [`Self::runtime_kind`].
    runtime_kind: RuntimeKind,
}

/// Exit discriminator for the finalized-head stream consumer. Drives the
/// outer restart loop's decision: reset backoff on natural EOF, escalate
/// on error, stop entirely when the last watch receiver is gone.
enum StreamExit {
    /// Stream ended cleanly (`None` from the iterator) — unlikely in
    /// practice but treated as a soft restart.
    Eof,
    /// Stream setup or an item yielded an error → connection is suspect,
    /// invalidate the shared slot and reconnect with backoff.
    Errored,
    /// Every watch receiver has been dropped, so the `RpcClient` is gone.
    /// Nothing to restart; exit the loop.
    HeadRxDropped,
}

/// Initial backoff between restart attempts. Small enough that a fast
/// recovery doesn't look stuck, large enough to avoid tight-looping on
/// a node that rejects every connect.
const WATCHER_INITIAL_BACKOFF: Duration = Duration::from_millis(250);

/// Upper bound on the backoff window. Beyond this, a human should
/// intervene — capping prevents a 2-hour sleep after many failures while
/// still giving the backend room to breathe.
const WATCHER_MAX_BACKOFF: Duration = Duration::from_secs(30);

impl RpcClient {
    /// Build a client bound to `endpoint`. `network_id`, `ss58_prefix`
    /// and `runtime_kind` come from the matching
    /// [`crate::network::NetworkSpec`] fields and are the hardcoded source
    /// of truth for the per-network metadata bundle, address encoding,
    /// and codegen-runtime dispatch — see [`Self::network_id`] /
    /// [`Self::ss58_prefix`] / [`Self::runtime_kind`] for the rationale.
    pub fn new(
        endpoint: impl Into<String>,
        network_id: &'static str,
        ss58_prefix: u16,
        runtime_kind: RuntimeKind,
    ) -> Self {
        let (finalized_head_tx, finalized_head_rx) = watch::channel(None);
        let (best_head_tx, best_head_rx) = watch::channel(None);
        let (blocks_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (transfers_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (ats_feed_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            endpoint: endpoint.into(),
            inner: Arc::new(RwLock::new(None)),
            caches: Caches::new(),
            finalized_head_tx,
            finalized_head_rx,
            best_head_tx,
            best_head_rx,
            blocks_tx,
            transfers_tx,
            ats_feed_tx,
            watcher_alive: Arc::new(AtomicBool::new(false)),
            block_time_secs_cache: Arc::new(RwLock::new(None)),
            runtime_identity_cache: Arc::new(RwLock::new(None)),
            runtime_code_cache: Arc::new(RwLock::new(None)),
            ss58_prefix,
            network_id,
            runtime_kind,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Resolve the subxt client, connecting on first successful call. A
    /// failure clears the slot so the next caller retries — no permanent
    /// bad state after a transient node outage. On a connect-time
    /// `DataError::Transport`, we invalidate and retry once before
    /// returning the error, matching the Phase 8 robustness contract.
    pub async fn subxt(&self) -> DataResult<SubxtClient> {
        match self.connect_or_reuse().await {
            Err(DataError::Transport(_)) => {
                // First attempt failed mid-handshake — drop whatever was
                // half-initialised and try once more. Two failures in a row
                // bubbles up to the caller.
                self.invalidate().await;
                self.connect_or_reuse().await
            }
            other => other,
        }
    }

    async fn connect_or_reuse(&self) -> DataResult<SubxtClient> {
        // Fast path: client already initialised — clone out of the read
        // lock. `SubxtClient` is internally `Arc<…>`, so cloning is cheap
        // and doesn't hold the lock across any await boundaries.
        if let Some(client) = self.inner.read().await.as_ref() {
            return Ok(client.clone());
        }

        // Slow path: acquire the write lock, re-check in case someone else
        // beat us to it, then connect.
        let mut guard = self.inner.write().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        // Enforce a stricter timeout on the handshake itself: a WS connect
        // that hasn't finished TLS/metadata negotiation in a few seconds is
        // almost always stuck (DNS stall, silent drop) and the caller is
        // better served by a fast `Transport` error than a 30 s hang.
        let connect = connect_client(&self.endpoint);
        let client = match tokio::time::timeout(RPC_CONNECT_TIMEOUT, connect).await {
            Ok(r) => r.map_err(DataError::from)?,
            Err(_) => {
                return Err(DataError::Transport(format!(
                    "connect to {} timed out after {RPC_CONNECT_TIMEOUT:?}",
                    self.endpoint
                )));
            }
        };
        *guard = Some(client.clone());
        // Release the write lock before spawning the watcher so the task
        // can acquire the read lock on its own schedule without deadlocking
        // against whatever reentrant path `subxt()` was called from.
        drop(guard);
        self.ensure_watcher_running();
        Ok(client)
    }

    /// Drop the cached client so the next caller re-connects. Also resets
    /// the finalized-head watch to `None` — without that, a consumer would
    /// keep reading a stale value from a connection that no longer exists.
    pub async fn invalidate(&self) {
        let mut guard = self.inner.write().await;
        *guard = None;
        // Ignore the result: if every receiver is dropped, the UI already
        // stopped reading; nothing to do.
        let _ = self.finalized_head_tx.send(None);
        let _ = self.best_head_tx.send(None);
        // Runtime upgrades can change `MinimumPeriod`, so a fresh
        // connection must re-read the constant rather than reuse the
        // previous run's value.
        *self.block_time_secs_cache.write().await = None;
        // Same reasoning for the runtime identity: a runtime upgrade bumps
        // `spec_version` (and rarely `spec_name`), so the cached shape must
        // be refreshed after reconnect. The `:code` fingerprint changes on
        // every upgrade too, so we clear it in the same write pass.
        *self.runtime_identity_cache.write().await = None;
        *self.runtime_code_cache.write().await = None;
    }

    /// Latest finalized head observed by the subscription task, or `None`
    /// while it's catching up. Cheap (single atomic load) — hot paths can
    /// call this per request without worrying about contention.
    pub fn finalized_head(&self) -> Option<u64> {
        *self.finalized_head_rx.borrow()
    }

    /// Latest best (possibly non-finalized) head observed by the best-block
    /// subscription, or `None` while it's catching up. One or two blocks
    /// above `finalized_head` during normal operation — use this for
    /// anything that should surface pending blocks (list pagination,
    /// block-by-number lookups) rather than `finalized_head`.
    pub fn best_head(&self) -> Option<u64> {
        *self.best_head_rx.borrow()
    }

    /// Block time in seconds as declared by the on-chain runtime, not the
    /// hardcoded [`crate::network::NetworkSpec`] default. Reads
    /// `Timestamp::MinimumPeriod` and applies the Substrate convention
    /// (block time = 2 × `MinimumPeriod`), caching the result so the
    /// networks endpoint can call it per request without a round-trip.
    /// Cleared on `invalidate` so a reconnect after a runtime upgrade
    /// picks up a fresh value.
    pub async fn block_time_secs(&self) -> DataResult<u64> {
        if let Some(v) = *self.block_time_secs_cache.read().await {
            return Ok(v);
        }
        let api = self.subxt().await?;
        let at = race_timeout("at_current_block", api.at_current_block())
            .await?
            .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?;
        // The constant address is runtime-typed even though the SCALE
        // payload is `u64` on both chains — dispatch on the tag so each
        // arm references its own codegen module.
        let min_period_ms: u64 = match self.runtime_kind {
            RuntimeKind::Allfeat => at
                .constants()
                .entry(
                    crate::data::rpc::runtime::allfeat::constants()
                        .timestamp()
                        .minimum_period(),
                )
                .map_err(|e| DataError::Decode(format!("decode Timestamp::MinimumPeriod: {e}")))?,
            RuntimeKind::Melodie => at
                .constants()
                .entry(
                    crate::data::rpc::runtime::melodie::constants()
                        .timestamp()
                        .minimum_period(),
                )
                .map_err(|e| DataError::Decode(format!("decode Timestamp::MinimumPeriod: {e}")))?,
        };
        // Substrate convention: `MinimumPeriod = SlotDuration / 2`, so the
        // target block time is `2 × MinimumPeriod`. Dividing at the end
        // avoids a fractional-second loss when MinimumPeriod is odd ms.
        let secs = (min_period_ms.saturating_mul(2)) / 1000;
        *self.block_time_secs_cache.write().await = Some(secs);
        Ok(secs)
    }

    /// Runtime identity (`sp_version::RuntimeVersion`) read from the
    /// chain's `Core_version` runtime API. Subxt's
    /// `OfflineClientAtBlockT::spec_version` exposes the spec-version
    /// scalar but hides the rest behind a private decode, so we call the
    /// raw runtime API and SCALE-decode the full shape ourselves — the
    /// same layout subxt uses internally.
    ///
    /// `at` selects the block to read against (finalized head when
    /// `None`). The head path is cached per `RpcClient` and cleared on
    /// `invalidate`; historical reads always round-trip so the UI's
    /// "At block…" form reflects a real chain state rather than a
    /// potentially stale cache.
    ///
    /// Decodes `state_version` when the runtime emits it (every post-V15
    /// `sp_version::RuntimeVersion` does). Pre-V15 runtimes truncate the
    /// SCALE tail before that field; we surface the absence as `None`
    /// rather than inventing a zero.
    pub async fn runtime_identity(&self, at: Option<u64>) -> DataResult<RuntimeIdentity> {
        if at.is_none() {
            if let Some(v) = self.runtime_identity_cache.read().await.as_ref() {
                return Ok(v.clone());
            }
        }
        let api = self.subxt().await?;
        let at_block = match at {
            Some(n) => race_timeout("at_block", api.at_block(n))
                .await?
                .map_err(|e| DataError::Rpc(format!("at_block({n}): {e}")))?,
            None => race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?,
        };
        let bytes = with_timeout("Core_version", async {
            at_block
                .runtime_apis()
                .call_raw("Core_version", None)
                .await
                .map_err(|e| DataError::Rpc(format!("Core_version: {e}")))
        })
        .await?;
        let identity = decode_core_version(&bytes)?;
        if at.is_none() {
            *self.runtime_identity_cache.write().await = Some(identity.clone());
        }
        Ok(identity)
    }

    /// Genesis block hash as rendered by polkadot.js / subxt (`0x` + 64
    /// hex). Cheap (in-memory read on the subxt client) so no caching
    /// layer beyond the already-cached `SubxtClient` itself.
    pub async fn genesis_hash(&self) -> DataResult<String> {
        let api = self.subxt().await?;
        Ok(hex_bytes(api.genesis_hash().as_ref()))
    }

    /// Raw `:code` bytes at `at` (finalized head when `None`). Used by
    /// the `/runtime/wasm` download endpoint. Intentionally uncached —
    /// the blob is 1–2 MiB per upgrade and the download is a
    /// user-initiated cold path; caching would inflate resident memory
    /// for a click that happens once per release at best. The hot
    /// "fingerprint" path ([`Self::runtime_code_info`]) caches the hash
    /// instead, so identity reads never round-trip.
    pub async fn runtime_wasm_bytes(&self, at: Option<u64>) -> DataResult<Vec<u8>> {
        let api = self.subxt().await?;
        let at_block = match at {
            Some(n) => race_timeout("at_block", api.at_block(n))
                .await?
                .map_err(|e| DataError::Rpc(format!("at_block({n}): {e}")))?,
            None => race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?,
        };
        with_iter_timeout("runtime_wasm_code", async {
            at_block
                .storage()
                .runtime_wasm_code()
                .await
                .map_err(|e| DataError::Rpc(format!("state_getStorage(:code): {e}")))
        })
        .await
    }

    /// `:code` fingerprint at `at` (or the finalized head when `None`).
    /// Fetches the raw storage blob once, blake2_256-hashes it, and
    /// decides whether it's zstd-compressed by matching the four-byte
    /// magic. The blob is 1–2 MiB for every current runtime so we drop
    /// it as soon as the hash is computed — keeping it in memory would
    /// cost many MiB per process without earning the caller anything.
    pub async fn runtime_code_info(&self, at: Option<u64>) -> DataResult<RuntimeCodeInfo> {
        if at.is_none() {
            if let Some(v) = self.runtime_code_cache.read().await.as_ref() {
                return Ok(v.clone());
            }
        }
        let api = self.subxt().await?;
        let at_block = match at {
            Some(n) => race_timeout("at_block", api.at_block(n))
                .await?
                .map_err(|e| DataError::Rpc(format!("at_block({n}): {e}")))?,
            None => race_timeout("at_current_block", api.at_current_block())
                .await?
                .map_err(|e| DataError::Rpc(format!("at_current_block: {e}")))?,
        };
        // Budget this with the iteration timeout: the read is a single
        // key but the blob is a couple of megabytes, so a saturated node
        // can take noticeably longer than a small-value `fetch_raw`.
        let code = with_iter_timeout("runtime_wasm_code", async {
            at_block
                .storage()
                .runtime_wasm_code()
                .await
                .map_err(|e| DataError::Rpc(format!("state_getStorage(:code): {e}")))
        })
        .await?;
        let mut hasher = Blake2b256::new();
        hasher.update(&code);
        let digest = hasher.finalize();
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&digest);
        let size_bytes = code.len().min(u32::MAX as usize) as u32;
        let compressed = code.len() >= ZSTD_MAGIC.len() && code[..ZSTD_MAGIC.len()] == ZSTD_MAGIC;
        let info = RuntimeCodeInfo {
            hash: hex_bytes(&hash_bytes),
            size_bytes,
            compressed,
        };
        if at.is_none() {
            *self.runtime_code_cache.write().await = Some(info.clone());
        }
        Ok(info)
    }

    /// SS58 address-format prefix for the network this client serves.
    /// Hardcoded at construction from [`crate::network::NetworkSpec::ss58_prefix`]
    /// — we don't fetch `ss58Format` from `system_properties` because the
    /// value is effectively immutable per chain and dev chains routinely
    /// ship without a `properties` block, which would otherwise force a
    /// fallback path on every boot.
    pub fn ss58_prefix(&self) -> u16 {
        self.ss58_prefix
    }

    /// Network id this client serves
    /// ([`crate::network::NetworkSpec::id`]). Used by code paths that
    /// don't carry a `ChainCtx` (the watcher task in particular) to look
    /// up the matching metadata bundle in [`crate::data::metadata`].
    pub fn network_id(&self) -> &'static str {
        self.network_id
    }

    /// Codegen runtime module this client reads through. Hardcoded at
    /// construction from [`crate::network::NetworkSpec::runtime_kind`];
    /// mappers and projections dispatch on it to pick the matching
    /// `runtime::{allfeat,melodie}::*` symbols.
    pub fn runtime_kind(&self) -> RuntimeKind {
        self.runtime_kind
    }

    /// Clone of the finalized-head watch channel. Consumers that need to
    /// react to new heads (indexer live worker, status endpoint cache,
    /// future best-block reconciliation) ride the same supervisor that
    /// already feeds `finalized_head()` — no extra subscription, no extra
    /// reconnect policy to maintain.
    pub fn watch_finalized_head(&self) -> watch::Receiver<Option<u64>> {
        self.finalized_head_rx.clone()
    }

    /// Hot + finalized caches fronting every `ChainData` method served by
    /// this client. See [`super::cache`] for the layering policy.
    pub fn caches(&self) -> &Caches {
        &self.caches
    }

    /// Subscribe to the per-block `Block` feed. The returned receiver starts
    /// empty; items land as the watch task publishes enriched blocks. The
    /// sender stays alive for the [`RpcClient`]'s lifetime so re-subscribing
    /// after a disconnect "just works" as soon as the next connect fires.
    pub fn subscribe_blocks(&self) -> broadcast::Receiver<Block> {
        self.blocks_tx.subscribe()
    }

    /// Subscribe to the per-block `Transfer` feed (`Balances.Transfer`
    /// events extracted from each finalized block).
    pub fn subscribe_transfers(&self) -> broadcast::Receiver<Transfer> {
        self.transfers_tx.subscribe()
    }

    /// Subscribe to the ATS feed (one item per newly-registered ATS version
    /// detected between consecutive finalized heads).
    pub fn subscribe_ats_feed(&self) -> broadcast::Receiver<AtsFeedItem> {
        self.ats_feed_tx.subscribe()
    }

    /// Eagerly start the finalized-head supervisor so `/readyz` can flip
    /// to 200 without waiting for a first API caller.
    ///
    /// `--mode=server` pods don't run the indexer, so nothing inside the
    /// process touches subxt until an external request arrives. That
    /// caused a chicken-and-egg: readiness stayed 503 → the Service had
    /// no endpoints → no external request would ever land. Calling this
    /// from `main` breaks the cycle; the supervisor reconnects on its
    /// own with backoff if the RPC isn't reachable yet.
    pub fn warm_up(&self) {
        self.ensure_watcher_running();
    }

    /// Spawn the finalized-head supervisor task, unless one is already
    /// running. The supervisor owns its own reconnect loop with
    /// exponential backoff (see [`run_watcher_supervisor`]); callers only
    /// have to guarantee "at least one supervisor is up", which this CAS
    /// handles safely for concurrent racers.
    fn ensure_watcher_running(&self) {
        // If the supervisor is already alive, don't spawn another one.
        // `compare_exchange(false, true)` returns Ok iff we flipped it —
        // that's the race-safe "acquire the spawn permit" primitive.
        if self
            .watcher_alive
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let endpoint = self.endpoint.clone();
        let inner = self.inner.clone();
        let head_tx = self.finalized_head_tx.clone();
        let head_rx = self.finalized_head_rx.clone();
        let best_head_tx = self.best_head_tx.clone();
        let blocks_tx = self.blocks_tx.clone();
        let transfers_tx = self.transfers_tx.clone();
        let ats_feed_tx = self.ats_feed_tx.clone();
        let alive = self.watcher_alive.clone();
        let ss58_prefix = self.ss58_prefix;
        let network_id = self.network_id;

        tokio::spawn(async move {
            run_watcher_supervisor(
                endpoint,
                inner,
                head_tx,
                head_rx,
                best_head_tx,
                blocks_tx,
                transfers_tx,
                ats_feed_tx,
                network_id,
                ss58_prefix,
            )
            .await;
            // Supervisor exits only when every watch receiver has been
            // dropped (the `RpcClient` itself is gone). Clear the flag for
            // symmetry — even though nothing should observe it by now.
            alive.store(false, Ordering::SeqCst);
        });
    }
}

/// Auto-restarting stream supervisor. Owns the reconnect policy so
/// the main `subxt()` path stays ignorant of the watcher's lifecycle.
///
/// Loop contract:
///
/// 1. Acquire (or reuse) a live `SubxtClient` via the shared `inner`
///    slot. On transport failure, sleep `backoff` and retry.
/// 2. Drive one pass of [`run_streams`] on that client. This races a
///    best-block enrichment loop against a lightweight finalized-head
///    watcher — whichever exits first ends the iteration, and the outer
///    loop reconnects both.
/// 3. On [`StreamExit::Errored`], invalidate the shared slot so the next
///    main-path caller also reconnects, then sleep and retry.
/// 4. On [`StreamExit::Eof`], reset backoff and loop immediately —
///    natural stream ends are transient (metadata refresh, chainHead
///    reset) and usually reconnect successfully.
/// 5. On [`StreamExit::HeadRxDropped`], the `RpcClient` has been dropped;
///    exit cleanly.
#[allow(clippy::too_many_arguments)]
async fn run_watcher_supervisor(
    endpoint: String,
    inner: Arc<RwLock<Option<SubxtClient>>>,
    head_tx: watch::Sender<Option<u64>>,
    head_rx: watch::Receiver<Option<u64>>,
    best_head_tx: watch::Sender<Option<u64>>,
    blocks_tx: broadcast::Sender<Block>,
    transfers_tx: broadcast::Sender<Transfer>,
    ats_feed_tx: broadcast::Sender<AtsFeedItem>,
    network_id: &'static str,
    ss58_prefix: u16,
) {
    let mut backoff = WATCHER_INITIAL_BACKOFF;
    loop {
        // Cheap cross-check: if the watch has no receivers, the owning
        // `RpcClient` was dropped before we even connected. Nothing to do.
        if head_tx.is_closed() {
            return;
        }

        let client = match ensure_watcher_client(&endpoint, &inner).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, backoff = ?backoff, "watcher: connect failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(WATCHER_MAX_BACKOFF);
                continue;
            }
        };

        match run_streams(
            &client,
            &head_tx,
            head_rx.clone(),
            &best_head_tx,
            &blocks_tx,
            &transfers_tx,
            &ats_feed_tx,
            network_id,
            ss58_prefix,
        )
        .await
        {
            StreamExit::HeadRxDropped => return,
            StreamExit::Errored => {
                // The current client is probably cooked; drop it from the
                // shared slot so subsequent `subxt()` callers reconnect
                // through their usual retry path.
                {
                    let mut g = inner.write().await;
                    *g = None;
                }
                let _ = head_tx.send(None);
                let _ = best_head_tx.send(None);
                tracing::warn!(backoff = ?backoff, "watcher: stream errored, reconnecting");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(WATCHER_MAX_BACKOFF);
            }
            StreamExit::Eof => {
                // Reset backoff: a clean end isn't a degradation signal.
                backoff = WATCHER_INITIAL_BACKOFF;
            }
        }
    }
}

/// Lock-free-ish path to retrieve (or establish) the shared client from
/// inside the supervisor. Mirrors [`RpcClient::connect_or_reuse`] but
/// without the retry-once-on-transport branch — the outer supervisor
/// already handles retry via its restart loop.
async fn ensure_watcher_client(
    endpoint: &str,
    inner: &Arc<RwLock<Option<SubxtClient>>>,
) -> DataResult<SubxtClient> {
    if let Some(c) = inner.read().await.as_ref() {
        return Ok(c.clone());
    }
    let mut g = inner.write().await;
    if let Some(c) = g.as_ref() {
        return Ok(c.clone());
    }
    let client = match tokio::time::timeout(RPC_CONNECT_TIMEOUT, connect_client(endpoint)).await {
        Ok(r) => r.map_err(DataError::from)?,
        Err(_) => {
            return Err(DataError::Transport(format!(
                "watcher connect to {endpoint} timed out after {RPC_CONNECT_TIMEOUT:?}"
            )))
        }
    };
    *g = Some(client.clone());
    Ok(client)
}

/// Single pass running the two live subscriptions side by side.
///
/// The explorer needs fresh timestamps (for the hero's "next block"
/// countdown) *and* a truthful `finalized` flag on each block. Subxt
/// exposes them as two separate subscriptions: `stream_best_blocks`
/// yields the tip as soon as it's imported (so `timestamp_ms` is close
/// to wall-clock), while `stream_blocks` only yields once GRANDPA has
/// finalized. Riding both lets the best-block loop enrich blocks /
/// transfers / ATS (one data pipeline, no duplicated `at_block` work),
/// while the finalized-head watcher does nothing but bump a `watch`
/// channel — the best-block enrichment reads from it to set the
/// `finalized` flag, and the frontend's block-by-number dedup flips
/// a "pending" row to "finalized" as the head advances.
///
/// `tokio::select!` ends the pass as soon as either side exits so the
/// supervisor's reconnect policy runs once for both subscriptions.
#[allow(clippy::too_many_arguments)]
async fn run_streams(
    client: &SubxtClient,
    head_tx: &watch::Sender<Option<u64>>,
    head_rx: watch::Receiver<Option<u64>>,
    best_head_tx: &watch::Sender<Option<u64>>,
    blocks_tx: &broadcast::Sender<Block>,
    transfers_tx: &broadcast::Sender<Transfer>,
    ats_feed_tx: &broadcast::Sender<AtsFeedItem>,
    network_id: &'static str,
    ss58_prefix: u16,
) -> StreamExit {
    tokio::select! {
        exit = run_finalized_head_watch(client, head_tx, blocks_tx, ss58_prefix) => exit,
        exit = run_best_stream(client, head_rx, best_head_tx, blocks_tx, transfers_tx, ats_feed_tx, network_id, ss58_prefix) => exit,
    }
}

/// Finalized-head subscription. Forwards the number into `head_tx` so
/// `finalized_head()` readers and the best-block enrichment loop can tag new
/// blocks correctly, and re-emits the just-finalized block on `blocks_tx`
/// with `finalized: true` so the frontend's block-by-number dedup flips the
/// existing "pending" row to "finalized". The re-enrichment goes through
/// [`mappers::map_block`] to match the shape emitted by the best-stream —
/// the finalized-block LRU on the provider makes repeat reads free.
async fn run_finalized_head_watch(
    client: &SubxtClient,
    head_tx: &watch::Sender<Option<u64>>,
    blocks_tx: &broadcast::Sender<Block>,
    ss58_prefix: u16,
) -> StreamExit {
    let mut stream = match client.stream_blocks().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "finalized head watch: stream setup failed");
            return StreamExit::Errored;
        }
    };

    while let Some(next) = stream.next().await {
        let block = match next {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(error = %e, "finalized head watch: stream dropped");
                return StreamExit::Errored;
            }
        };
        let number = block.number();
        if head_tx.send(Some(number)).is_err() {
            return StreamExit::HeadRxDropped;
        }

        // Re-emit as finalized only if someone's listening — an idle explorer
        // costs zero extra RPC. A dropped pin or mapper error just skips the
        // update; the stale "pending" row stays put until the next tick.
        if blocks_tx.receiver_count() == 0 {
            continue;
        }
        let at = match client.at_block(number).await {
            Ok(at) => at,
            Err(e) => {
                tracing::debug!(error = %e, number, "finalized re-emit skip: at_block");
                continue;
            }
        };
        match mappers::map_block(&at, number, ss58_prefix).await {
            Ok(b) => {
                let _ = blocks_tx.send(b);
            }
            Err(e) => tracing::debug!(error = %e, number, "finalized re-emit skip: map_block"),
        }
    }
    StreamExit::Eof
}

/// Best-head enrichment loop. Drives the per-block data pipeline: map
/// the block, emit transfers, detect new ATS versions. Runs against
/// `stream_best_blocks` so the hero's countdown sees fresh timestamps
/// — the trade-off is that a re-orged best block may briefly surface
/// a transfer or ATS item that subsequently disappears, which is
/// vanishingly rare on a GRANDPA chain like Allfeat.
///
/// The `finalized` flag on each emitted block is computed against the
/// last-known finalized head from [`run_finalized_head_watch`], so a
/// "pending" block in the UI flips to "finalized" the moment the head
/// catches up (the frontend store dedupes by block number and adopts
/// the latest shape).
///
/// Enrichment is gated on `receiver_count() > 0` per topic so an idle
/// explorer stays RPC-cheap.
#[allow(clippy::too_many_arguments)]
async fn run_best_stream(
    client: &SubxtClient,
    head_rx: watch::Receiver<Option<u64>>,
    best_head_tx: &watch::Sender<Option<u64>>,
    blocks_tx: &broadcast::Sender<Block>,
    transfers_tx: &broadcast::Sender<Transfer>,
    ats_feed_tx: &broadcast::Sender<AtsFeedItem>,
    network_id: &'static str,
    ss58_prefix: u16,
) -> StreamExit {
    let mut stream = match client.stream_best_blocks().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "best watch: stream setup failed");
            return StreamExit::Errored;
        }
    };

    // Tracks `NextAtsId` across ticks so we can derive "how many new
    // versions landed in this block" without rescanning events.
    // Initialised lazily on the first block we fetch `at` for.
    let mut last_next_ats_id: Option<u64> = None;

    while let Some(next) = stream.next().await {
        let block = match next {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(error = %e, "best watch: stream dropped");
                return StreamExit::Errored;
            }
        };
        let number = block.number();

        // Publish the best head before any early-exit: the provider's
        // block-list pagination and block-by-number guards key off this
        // value even when the broadcast fan-outs have no subscribers.
        let _ = best_head_tx.send(Some(number));

        let has_blocks = blocks_tx.receiver_count() > 0;
        let has_transfers = transfers_tx.receiver_count() > 0;
        let has_ats = ats_feed_tx.receiver_count() > 0;
        if !(has_blocks || has_transfers || has_ats) {
            continue;
        }

        // Pin the block for the mappers. Best blocks can be pruned under a
        // re-org before we get to enrich; a dropped pin turns into a skip
        // and the next best header catches us back up.
        let at = match client.at_block(number).await {
            Ok(at) => at,
            Err(e) => {
                tracing::debug!(error = %e, number, "enrich skip: at_block");
                continue;
            }
        };

        let finalized_head = head_rx.borrow().unwrap_or(0);

        if has_blocks {
            match mappers::map_block(&at, finalized_head, ss58_prefix).await {
                Ok(b) => {
                    let _ = blocks_tx.send(b);
                }
                Err(e) => tracing::debug!(error = %e, number, "enrich skip: map_block"),
            }
        }

        // Transfers and extrinsics share the same extrinsic list
        // and the same event set, so fetch events once and reuse the
        // phase index across both mappers. Early-return each failure
        // independently so one ladder stays flat.
        if has_transfers {
            if let Err(e) = enrich_transfers(&at, transfers_tx, network_id, ss58_prefix).await {
                tracing::debug!(error = %e, number, "enrich skip: transfers");
            }
        }

        if has_ats {
            match mappers::fetch_next_ats_id(&at).await {
                Ok(current) => {
                    let previous = last_next_ats_id.unwrap_or(current);
                    if current > previous {
                        let delta = (current - previous).min(u32::MAX as u64) as u32;
                        match mappers::build_ats_feed(
                            client,
                            &at,
                            delta,
                            0,
                            network_id,
                            ss58_prefix,
                        )
                        .await
                        {
                            Ok(feed) => {
                                // `build_ats_feed` returns newest-first.
                                // Emit chronologically so the consumer's
                                // prepend-on-receive stays monotonic.
                                for item in feed.into_iter().rev() {
                                    let _ = ats_feed_tx.send(item);
                                }
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, number, "enrich skip: ats_feed")
                            }
                        }
                    }
                    last_next_ats_id = Some(current);
                }
                Err(e) => tracing::debug!(error = %e, number, "enrich skip: next_ats_id"),
            }
        }
    }
    StreamExit::Eof
}

/// Fetch + map transfers for a single finalized block and fan out every item
/// to the broadcast channel. Extracted so `spawn_finalized_watch` doesn't
/// have to nest `match`es six levels deep — each failure here is already
/// logged by the caller with `block_number` context.
async fn enrich_transfers(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    transfers_tx: &broadcast::Sender<Transfer>,
    network_id: &str,
    ss58_prefix: u16,
) -> DataResult<()> {
    let timestamp_ms = mappers::fetch_timestamp(at).await?;
    let events = mappers::fetch_block_events(at).await?;
    let events_by_phase = mappers::index_events_by_phase(&events)?;
    let xs =
        mappers::map_extrinsics(at, timestamp_ms, &events_by_phase, network_id, ss58_prefix)
            .await?;
    for t in mappers::map_transfers(&xs, &events_by_phase, ss58_prefix)? {
        let _ = transfers_tx.send(t);
    }
    Ok(())
}

/// Decode a `Core_version` blob into [`RuntimeIdentity`]. Mirrors
/// `sp_version::RuntimeVersion`: spec/impl names (`String`), the four
/// `u32` versions, the `apis` registry, and — post-V15 — a single
/// `state_version: u8` at the tail. Pre-V15 runtimes truncate there,
/// so we attempt the trailing byte via a second `Decode` pass and fall
/// back to `None` on a short buffer instead of failing the whole call.
fn decode_core_version(bytes: &[u8]) -> DataResult<RuntimeIdentity> {
    #[derive(codec::Decode)]
    #[codec(crate = subxt::ext::codec)]
    struct CoreVersionHeader {
        spec_name: String,
        impl_name: String,
        authoring_version: u32,
        spec_version: u32,
        impl_version: u32,
        _apis: Vec<([u8; 8], u32)>,
        transaction_version: u32,
    }
    let mut cursor = bytes;
    let header = CoreVersionHeader::decode(&mut cursor)
        .map_err(|e| DataError::Decode(format!("decode Core_version header: {e}")))?;
    // `cursor` advances past the consumed prefix; a one-byte tail is
    // the post-V15 `state_version` field. An exhausted cursor means a
    // pre-V15 runtime, which we surface as `None` rather than forging
    // a `0` that would pollute the UI.
    let state_version = u8::decode(&mut cursor).ok();
    Ok(RuntimeIdentity {
        spec_name: header.spec_name,
        impl_name: header.impl_name,
        authoring_version: header.authoring_version,
        spec_version: header.spec_version,
        impl_version: header.impl_version,
        transaction_version: header.transaction_version,
        state_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use subxt::ext::codec::Encode;

    /// A brand-new client hasn't had the subscription task fire yet, so the
    /// watch must be `None` — callers rely on that sentinel to pick the RPC
    /// fallback path in `finalized_head_number`.
    #[tokio::test]
    async fn finalized_head_is_none_before_first_notification() {
        let client = RpcClient::new("ws://127.0.0.1:9", "allfeat", 42, RuntimeKind::Allfeat);
        assert_eq!(client.finalized_head(), None);
    }

    /// `Core_version` with a post-V15 tail: the extra `state_version` byte
    /// must round-trip through the decoder. Guards the fall-through path
    /// below — if a legitimate `state_version` byte ever starts being
    /// silently dropped, this test fails.
    #[test]
    fn decode_core_version_reads_state_version_when_present() {
        let mut bytes = Vec::new();
        "allfeat".to_string().encode_to(&mut bytes);
        "allfeat-node".to_string().encode_to(&mut bytes);
        1u32.encode_to(&mut bytes); // authoring_version
        1_001_004u32.encode_to(&mut bytes); // spec_version
        0u32.encode_to(&mut bytes); // impl_version
        Vec::<([u8; 8], u32)>::new().encode_to(&mut bytes);
        25u32.encode_to(&mut bytes); // transaction_version
        1u8.encode_to(&mut bytes); // state_version
        let id = decode_core_version(&bytes).expect("decode succeeds");
        assert_eq!(id.spec_name, "allfeat");
        assert_eq!(id.impl_name, "allfeat-node");
        assert_eq!(id.spec_version, 1_001_004);
        assert_eq!(id.transaction_version, 25);
        assert_eq!(id.state_version, Some(1));
    }

    /// Pre-V15 runtimes omit the trailing `state_version` byte. The decoder
    /// must report `None` for that field rather than failing the whole call
    /// — we'd otherwise fail `/api/v1/networks/{id}/runtime` on older
    /// archives. The struct before `state_version` is otherwise identical.
    #[test]
    fn decode_core_version_without_state_version_tail_yields_none() {
        let mut bytes = Vec::new();
        "polkadot".to_string().encode_to(&mut bytes);
        "parity-polkadot".to_string().encode_to(&mut bytes);
        0u32.encode_to(&mut bytes);
        1_000_000u32.encode_to(&mut bytes);
        0u32.encode_to(&mut bytes);
        Vec::<([u8; 8], u32)>::new().encode_to(&mut bytes);
        7u32.encode_to(&mut bytes);
        let id = decode_core_version(&bytes).expect("decode succeeds");
        assert_eq!(id.state_version, None);
    }

    /// `invalidate` must clear both the connection slot and the watch so a
    /// stale finalized head can't survive a disconnect.
    #[tokio::test]
    async fn invalidate_resets_watch_to_none() {
        let client = RpcClient::new("ws://127.0.0.1:9", "allfeat", 42, RuntimeKind::Allfeat);
        // Simulate the subscription having published at some point in the
        // past. We push through the tx side directly; in production only
        // the spawned task writes here.
        client.finalized_head_tx.send(Some(42)).unwrap();
        assert_eq!(client.finalized_head(), Some(42));

        client.invalidate().await;
        assert_eq!(
            client.finalized_head(),
            None,
            "invalidate must reset the watch or consumers read a stale head",
        );
    }

    /// Transport-only failures must blow up after the single retry — two
    /// failed attempts surface the original error to the caller. Port 9 is
    /// reserved (RFC 863, "discard"), so the connect fails fast and we
    /// exercise the retry arm without a real dev node.
    #[tokio::test]
    async fn subxt_retries_once_on_transport_error_then_gives_up() {
        let client = RpcClient::new("ws://127.0.0.1:9", "allfeat", 42, RuntimeKind::Allfeat);
        let err = client.subxt().await.expect_err("port 9 must refuse");
        assert!(
            matches!(err, DataError::Transport(_)),
            "expected Transport after retry, got {err:?}",
        );
    }
}
