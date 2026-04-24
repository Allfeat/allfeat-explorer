//! Errors surfaced by the data layer.
//!
//! Providers map their backend-specific failures onto this type so callers
//! (server functions, eventually) only need to handle one shape regardless
//! of whether data came from the mock generator or the on-chain RPC client.

use thiserror::Error;

#[derive(Clone, Debug, Error)]
pub enum DataError {
    /// Transport-level failure: DNS, TCP, TLS, connection refused, read timeout.
    #[error("transport error: {0}")]
    Transport(String),

    /// RPC-level failure from the node or subxt: bad params, method not
    /// found, internal error, runtime incompatibility.
    #[error("rpc error: {0}")]
    Rpc(String),

    /// Could not decode a response payload into the expected shape
    /// (SCALE decode failure, unexpected enum variant, etc.).
    #[error("decode error: {0}")]
    Decode(String),

    /// Response was well-formed but a required invariant was violated
    /// (e.g. missing block hash for a number that should exist).
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// Caller-supplied input was rejected *before* any backend work (e.g.
    /// a cursor whose grammar the parser didn't accept). Surfaces as a
    /// `400` at the API boundary. Reserving a dedicated variant means
    /// `InvalidPayload` stays a server-side integrity failure.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// No RPC endpoint is configured for the requested network.
    #[error("no rpc endpoint configured for network '{0}'")]
    NetworkUnconfigured(String),

    /// Backend doesn't support this query yet (scaffolded but unimplemented).
    #[error("unimplemented: {0}")]
    Unimplemented(&'static str),

    /// The query is valid but unavailable on the requested network — e.g. a
    /// mainnet-only pallet asked about on a testnet chain. Surfaces as a 404
    /// at the API boundary so the UI can render a neutral "not available
    /// here" page instead of an error toast.
    #[error("not supported on network '{network}': {what}")]
    NotSupported {
        network: &'static str,
        what: &'static str,
    },
}

impl DataError {
    pub fn decode(msg: impl Into<String>) -> Self {
        Self::Decode(msg.into())
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidPayload(msg.into())
    }
}

#[cfg(feature = "ssr")]
impl From<subxt::Error> for DataError {
    fn from(e: subxt::Error) -> Self {
        DataError::Rpc(e.to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<subxt::error::OnlineClientError> for DataError {
    fn from(e: subxt::error::OnlineClientError) -> Self {
        // `OnlineClientError` is raised during connection setup (URL parse,
        // RPC handshake, metadata fetch) — classify as transport since it
        // fails before any query executes.
        DataError::Transport(e.to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<subxt::rpcs::Error> for DataError {
    fn from(e: subxt::rpcs::Error) -> Self {
        // The legacy RPC client reports both handshake-time failures (URL
        // validation, connect refused) and per-call failures under one type.
        // `InsecureUrl` and client-construction failures are clearly transport;
        // per-call errors land here too but the node's response body is the
        // stringified `e`, which keeps the error still readable either way.
        match e {
            subxt::rpcs::Error::InsecureUrl(_) | subxt::rpcs::Error::Client(_) => {
                DataError::Transport(e.to_string())
            }
            _ => DataError::Rpc(e.to_string()),
        }
    }
}

pub type DataResult<T> = Result<T, DataError>;
