//! One-shot drift-fix for `account_balances`.
//!
//! The per-block snapshot pipeline ([`super::projections::accounts`] +
//! [`super::sink::apply_account_snapshots`]) keeps freshly-touched
//! accounts correct by reading `System::Account` at the block's hash.
//! What it can't do is revisit accounts that haven't been touched
//! since a bug corrupted their row — a drifted balance on an idle
//! account stays drifted until someone spends from it.
//!
//! `reconcile_network` is the operator tool for exactly that case.
//! It walks every row in `account_balances` for the target network,
//! refetches each account's state from `System::Account` at the
//! chain's current finalized head, and UPDATEs the stored totals in
//! place. Timestamp columns (`first_seen_block`, `last_activity_block`)
//! are deliberately left alone: reconciliation is maintenance, not
//! activity.
//!
//! The walk is batched so an archive chain with millions of rows
//! doesn't have to be held in memory all at once, and so progress is
//! visible through `tracing::info!` at sane intervals.

use std::sync::Arc;

use sqlx::PgPool;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::race_timeout;
use crate::data::rpc::mappers::accounts::fetch_accounts_at;
use crate::data::rpc::RpcClient;

use super::sink;

/// Rows per SELECT + per-batch UPDATE pass. Sized to keep a single
/// transaction's lock surface bounded while still amortising the
/// round-trip cost of the batch scan. Matches the shape of the
/// `buffered` fan-out cap downstream — we're RPC-bound, not DB-bound.
const BATCH_SIZE: i64 = 500;

/// Summary of one network's reconciliation pass. Surfaced to the CLI so
/// operators see exactly what the sweep touched without having to diff
/// the table by hand afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub network_id: String,
    pub head_block: u64,
    pub accounts_scanned: u64,
    pub rows_updated: u64,
}

/// Walk every `account_balances` row for `network_id`, refetch each
/// account from `System::Account` at the current finalized head, and
/// UPDATE the stored totals in place.
///
/// Pagination uses a monotonic `account BYTEA` cursor — rows are
/// naturally ordered by the PK's second component, so each batch asks
/// for "the next N accounts greater than the last one we saw". That
/// avoids the OFFSET pitfall (rows shifting beneath us if a concurrent
/// write lands mid-sweep) and scales to chains with hundreds of
/// thousands of accounts.
pub async fn reconcile_network(
    network_id: &'static str,
    network_sid: i16,
    client: Arc<RpcClient>,
    pool: PgPool,
) -> DataResult<ReconcileReport> {
    let api = client.subxt().await?;
    let runtime_kind = client.runtime_kind();
    // `at_current_block` pins to the best head at call time. We
    // deliberately don't wait for finalization here: reconciliation is
    // a maintenance sweep, not an indexing commit, and a reorg after
    // the snapshot would simply be fixed by the next pass (or by the
    // per-block pipeline touching the affected account). The block
    // number it reports is the basis for the `ReconcileReport`'s
    // `head_block` field — operator-facing, not load-bearing.
    let at = race_timeout("reconcile: at_current_block", api.at_current_block())
        .await?
        .map_err(|e| DataError::Rpc(format!("reconcile({network_id}): at_current_block: {e}")))?;
    let head_block = at.block_number() as u64;

    tracing::info!(
        network = network_id,
        head = head_block,
        "reconcile: starting sweep"
    );

    let mut report = ReconcileReport {
        network_id: network_id.to_string(),
        head_block,
        accounts_scanned: 0,
        rows_updated: 0,
    };

    // Start the keyset cursor at the lexicographically-smallest BYTEA
    // value. `account` is a 32-byte array on the wire, so a 32-byte
    // zero vector is the correct "strictly less than every row" seed.
    let mut cursor: Vec<u8> = Vec::new();
    loop {
        let rows: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT account FROM account_balances
              WHERE network_id = $1 AND account > $2
              ORDER BY account
              LIMIT $3",
        )
        .bind(network_sid)
        .bind(&cursor)
        .bind(BATCH_SIZE)
        .fetch_all(&pool)
        .await
        .map_err(|e| {
            DataError::Rpc(format!(
                "reconcile({network_id}): scan account_balances: {e}"
            ))
        })?;

        if rows.is_empty() {
            break;
        }

        // Normalise the SELECT's `Vec<u8>` column into the fixed-width
        // array the rest of the pipeline expects. A corrupted row
        // (wrong byte length) surfaces loudly rather than slipping into
        // the fan-out with a truncated key.
        let accounts: Vec<[u8; 32]> = rows
            .iter()
            .map(|v| {
                let arr: [u8; 32] = v.as_slice().try_into().map_err(|_| {
                    DataError::Decode(format!(
                        "reconcile({network_id}): account column has unexpected length {}",
                        v.len()
                    ))
                })?;
                Ok::<_, DataError>(arr)
            })
            .collect::<DataResult<Vec<_>>>()?;

        cursor = rows
            .last()
            .expect("non-empty by the early-return above")
            .clone();

        let snapshots = fetch_accounts_at(&at, &accounts, runtime_kind).await?;

        let mut tx = pool
            .begin()
            .await
            .map_err(|e| DataError::Rpc(format!("reconcile({network_id}): begin tx: {e}")))?;
        let updated = sink::reconcile_account_balances(&mut tx, network_sid, &snapshots).await?;
        tx.commit()
            .await
            .map_err(|e| DataError::Rpc(format!("reconcile({network_id}): commit tx: {e}")))?;

        report.accounts_scanned = report
            .accounts_scanned
            .saturating_add(accounts.len() as u64);
        report.rows_updated = report.rows_updated.saturating_add(updated);

        tracing::info!(
            network = network_id,
            scanned = report.accounts_scanned,
            updated = report.rows_updated,
            "reconcile: batch complete"
        );

        if (accounts.len() as i64) < BATCH_SIZE {
            break;
        }
    }

    tracing::info!(
        network = network_id,
        head = report.head_block,
        scanned = report.accounts_scanned,
        updated = report.rows_updated,
        "reconcile: sweep complete"
    );
    Ok(report)
}

/// Run [`reconcile_network`] across every configured `(network_id,
/// client)` pair. Failures on one network don't block the others: each
/// network's result is captured independently so operators still see
/// which chains converged and which didn't.
pub async fn reconcile_all(
    networks: Vec<(&'static str, Arc<RpcClient>)>,
    pool: PgPool,
    network_lookup: &super::lookups::NetworkLookup,
) -> Vec<(String, DataResult<ReconcileReport>)> {
    let mut out = Vec::with_capacity(networks.len());
    for (network_id, client) in networks {
        let res = match network_lookup.resolve(network_id) {
            Some(sid) => reconcile_network(network_id, sid, client, pool.clone()).await,
            None => Err(DataError::Rpc(format!(
                "reconcile({network_id}): network_id not registered in networks table"
            ))),
        };
        if let Err(e) = &res {
            tracing::error!(
                network = network_id,
                error = %e,
                "reconcile: network sweep failed"
            );
        }
        out.push((network_id.to_string(), res));
    }
    out
}
