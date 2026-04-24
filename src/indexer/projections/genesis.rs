//! Genesis bootstrap — read the initial balance table directly from
//! `System::Account` at the genesis block.
//!
//! Why a separate projection: the regular per-block pipeline fetches
//! `System::Account` only for accounts touched by the block's events or
//! extrinsics. Genesis has none of either (no events fire at height 0,
//! the chain spec lays the initial balances down out-of-band) so the
//! event-driven collector never nominates an account for snapshotting.
//! Walking the storage map once at block 0 covers every endowed account
//! in a single pass.
//!
//! Shape: iterate `System::Account` at the genesis block hash, pair each
//! account id with its decoded [`AccountSnapshot`], and feed the list
//! through the same [`crate::indexer::sink::apply_account_snapshots`]
//! the live + backfill workers use. The sink's conflict clause ensures
//! the seed + any later block-0 touches converge deterministically.
//!
//! Replay safety: the workers gate the seed on `fresh == true` for
//! block 0, so this projection runs exactly once per `(network, chain)`
//! combination. Re-running it would be idempotent anyway (snapshots are
//! absolute values, UPSERT replaces), but the RPC iter is expensive so
//! the gate is a worthwhile guard.

use subxt::client::OnlineClientAtBlock;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::with_iter_timeout;
use crate::data::rpc::mappers::accounts::AccountSnapshot;
use crate::data::rpc::runtime::allfeat;

/// Iterate `System::Account` at the block `at` refers to and return one
/// absolute [`AccountSnapshot`] per entry. Intended to be called against
/// the genesis block hash; the sink writes these via the same UPSERT it
/// uses for regular per-block snapshots.
pub async fn project_genesis_accounts(
    at: &OnlineClientAtBlock<SubstrateConfig>,
) -> DataResult<Vec<([u8; 32], AccountSnapshot)>> {
    with_iter_timeout("project_genesis_accounts", async {
        let mut entries = at
            .storage()
            .iter(allfeat::storage().system().account(), ())
            .await
            .map_err(|e| DataError::Rpc(format!("iter system.account (genesis): {e}")))?;

        let mut out = Vec::new();
        while let Some(kv) = entries.next().await {
            let kv =
                kv.map_err(|e| DataError::Rpc(format!("iter system.account (genesis) next: {e}")))?;
            let (id,) = kv
                .key()
                .map_err(|e| DataError::Decode(format!("system.account key (genesis): {e}")))?
                .decode()
                .map_err(|e| {
                    DataError::Decode(format!("decode system.account key (genesis): {e}"))
                })?;
            let info = kv
                .value()
                .decode()
                .map_err(|e| DataError::Decode(format!("decode AccountInfo (genesis): {e}")))?;
            let mut account = [0u8; 32];
            account.copy_from_slice(id.as_ref());
            out.push((
                account,
                AccountSnapshot {
                    free: info.data.free,
                    reserved: info.data.reserved,
                    frozen: info.data.frozen,
                    nonce: info.nonce,
                },
            ));
        }
        Ok(out)
    })
    .await
}
