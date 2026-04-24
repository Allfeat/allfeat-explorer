//! SQL reads backing the account-related `ChainData` methods.
//!
//! `account_balances` stores the per-block absolute snapshots the live
//! and backfill workers write after reading `System::Account` at each
//! block's hash (see
//! [`crate::indexer::projections::accounts::collect_touched`] and
//! [`crate::indexer::sink::apply_account_snapshots`]). The address the
//! UI hands us is SS58; we round-trip through `AccountId32` so the
//! query matches the raw BYTEA PK without the DB having to store an
//! indexed SS58 copy.
//!
//! `first_seen_ms` / `last_activity_ms` are denormalised onto
//! `account_balances` (migration 004) so the reads here don't have to
//! JOIN `blocks` just to render wall clock. The single-row read is a
//! primary-key lookup; the top-N read hits
//! `account_balances_total_idx` and never needs a secondary table.

use sqlx::PgPool;
use subxt::utils::AccountId32;

use crate::data::error::{DataError, DataResult};
use crate::data::ss58::encode_ss58_bytes;
use crate::domain::{Account, Balance};

/// Column list shared by every account read. `NUMERIC` columns ride TEXT
/// on the wire — same trick the extrinsics/transfers queries use to
/// avoid the `bigdecimal`/`rust_decimal` sqlx features.
const ACCOUNT_COLUMNS: &str = "ab.account, \
                               ab.free::text, ab.reserved::text, ab.frozen::text, \
                               ab.nonce, \
                               ab.first_seen_ms, \
                               ab.last_activity_ms";

/// Raw tuple returned by the SELECT. Kept in lockstep with
/// [`row_to_account`] via positional binding; a shuffle here must be
/// reflected there.
type Row = (
    Vec<u8>, // account
    String,  // free::text
    String,  // reserved::text
    String,  // frozen::text
    i64,     // nonce
    i64,     // first_seen_ms
    i64,     // last_active_ms
);

/// Fetch one account by its SS58 string on `network_sid`. Returns
/// `None` when either the address is malformed or no row has been
/// materialised yet (the account has never emitted a balance event in
/// an indexed block on that chain).
///
/// We never fall back to RPC here — the banner flags indexer lag and
/// masking that with an RPC call would trade correctness for "always
/// shows something" and hide real bugs in the projection.
pub async fn account_by_address(
    pool: &PgPool,
    network_sid: i16,
    address: &str,
    ss58_prefix: u16,
) -> DataResult<Option<Account>> {
    let Some(id) = address.parse::<AccountId32>().ok() else {
        return Ok(None);
    };
    let bytes: &[u8; 32] = id.as_ref();

    let sql = format!(
        "SELECT {ACCOUNT_COLUMNS} \
         FROM account_balances ab \
         WHERE ab.network_id = $1 AND ab.account = $2"
    );
    let row: Option<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(&bytes[..])
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "account_by_address(net={network_sid}/{address}): {e}"
            ))
        })?;
    Ok(row.and_then(|r| row_to_account(r, ss58_prefix)))
}

/// Top-N accounts on `network_sid` by total balance (`free + reserved`),
/// descending. The `account_balances_total_idx` index backs this so
/// pagination remains O(count) regardless of table size.
pub async fn top_accounts(
    pool: &PgPool,
    network_sid: i16,
    count: u32,
    ss58_prefix: u16,
) -> DataResult<Vec<Account>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {ACCOUNT_COLUMNS} \
         FROM account_balances ab \
         WHERE ab.network_id = $1 \
         ORDER BY (ab.free + ab.reserved) DESC \
         LIMIT $2"
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(network_sid)
        .bind(count as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| DataError::Rpc(format!("top_accounts(net={network_sid}, {count}): {e}")))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| row_to_account(r, ss58_prefix))
        .collect())
}

/// Decode a DB row into the domain `Account`. Returns `None` for a
/// non-32-byte `account` payload (every row in the table is padded to 32
/// bytes by the projection, so a mismatch is genuine corruption and
/// hiding the row is safer than panicking the feed).
fn row_to_account(row: Row, ss58_prefix: u16) -> Option<Account> {
    let (
        account_bytes,
        free_text,
        reserved_text,
        _frozen_text,
        nonce,
        first_seen_ms,
        last_active_ms,
    ) = row;

    let address = encode_ss58_bytes(&account_bytes, ss58_prefix)?;

    let free = free_text.parse::<u128>().unwrap_or(0);
    let reserved = reserved_text.parse::<u128>().unwrap_or(0);
    // `frozen` is a lock overlay on `free`, not a separate bucket —
    // mirrors the RPC mapper's convention so the DB and RPC paths
    // render identically. Total excludes it to avoid double-counting.
    let total = free.saturating_add(reserved);

    Some(Account {
        address,
        balance: Balance {
            total,
            transferable: free,
            reserved,
        },
        nonce: nonce.max(0) as u32,
        first_seen_ms,
        last_active_ms,
    })
}
