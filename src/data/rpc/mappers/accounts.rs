//! Account-level mapping: SS58 parsing, single-account fetch, the
//! top-N richest-account scan that powers `/accounts`, and the per-block
//! snapshot fan-out the indexer uses to rebuild `account_balances` from
//! `System::Account` reads (rather than accumulating event deltas).

use futures::{stream, StreamExt, TryStreamExt};
use subxt::client::OnlineClientAtBlock;
use subxt::utils::AccountId32;
use subxt::SubstrateConfig;

use crate::data::error::{DataError, DataResult};
use crate::data::rpc::client::{with_iter_timeout, with_timeout};
use crate::data::rpc::runtime::{allfeat, melodie};
use crate::data::ss58::encode_ss58;
use crate::domain::{Account, Balance};
use crate::network::RuntimeKind;

/// Bounded concurrency for the per-block `System::Account` fan-out. A
/// conservative cap — each block typically touches a handful of accounts
/// and stretching the parallelism further trades RPC load for marginal
/// wall-clock gains. Lifted deliberately (over a hot loop) so the indexer
/// doesn't hammer the node during a large backfill.
const SNAPSHOT_CONCURRENCY: usize = 2;

/// Absolute on-chain account state at a specific block height, read
/// directly from `System::Account`. The indexer writes one of these per
/// touched account per block into `account_balances`, replacing the
/// older delta-accumulation pipeline whose drift was impossible to keep
/// bounded as the pallet ecosystem grew (staking, treasury, identity,
/// vesting, multisig, custom Allfeat pallets, modern `fungible::hold`…).
///
/// Reaped accounts (`System::Account` returned `None` — existential
/// deposit dropped, sufficients hit zero) are represented by an all-zero
/// snapshot. The sink relies on the schema's `GREATEST(nonce, EXCLUDED.nonce)`
/// clause to keep a previously-seen nonce from regressing when the
/// runtime clears the reaped account's row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub free: u128,
    pub reserved: u128,
    pub frozen: u128,
    pub nonce: u32,
}

/// Runtime-neutral projection of `frame_system::AccountInfo` after the
/// codegen-typed decode. Each `match runtime_kind` arm folds the codegen
/// `AccountInfo<u32, AccountData<u128>>` into this view before crossing
/// the arm boundary so downstream mapping doesn't have to be duplicated
/// per runtime.
#[derive(Clone, Copy, Debug)]
struct AccountInfoView {
    free: u128,
    reserved: u128,
    frozen: u128,
    nonce: u32,
}

/// Parse an SS58-encoded address into an `AccountId32`. Returns `None` for any
/// malformed input (wrong checksum, wrong length, non-SS58). Callers surface
/// that as `Ok(None)` so the UI shows the standard "not found" page.
pub fn parse_ss58(addr: &str) -> Option<AccountId32> {
    addr.parse::<AccountId32>().ok()
}

/// Compose a `crate::domain::Account` from the account id + the runtime-neutral
/// `AccountInfoView` projected from `System::Account`. `first_seen_ms` /
/// `last_active_ms` are left at `0`: neither is tracked by the runtime, and
/// fabricating values from mock data on the live path would lie to the UI.
/// Pages render them as "—" today.
fn map_account_info(id: &AccountId32, info: &AccountInfoView, ss58_prefix: u16) -> Account {
    // On a chain with a fixed supply and no staking, total = free + reserved.
    // `frozen` is a lock (transfer restriction), not a separate bucket — it
    // overlaps with `free` and shouldn't be double-counted.
    let total = info.free.saturating_add(info.reserved);
    Account {
        address: encode_ss58(&id.0, ss58_prefix),
        balance: Balance {
            total,
            transferable: info.free,
            reserved: info.reserved,
        },
        nonce: info.nonce,
        first_seen_ms: 0,
        last_active_ms: 0,
    }
}

/// Fetch a single `System::Account` entry and translate it to
/// [`crate::domain::Account`]. Returns `None` for unknown addresses (runtime
/// reaps zero-balance accounts, so "missing" is expected, not an error).
pub async fn fetch_account(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    address: &str,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Option<Account>> {
    let Some(id) = parse_ss58(address) else {
        return Ok(None);
    };
    let info = match runtime_kind {
        RuntimeKind::Allfeat => {
            let value = with_timeout("fetch_account", async {
                at.storage()
                    .try_fetch(allfeat::storage().system().account(), (id,))
                    .await
                    .map_err(|e| DataError::Rpc(format!("fetch system.account({address}): {e}")))
            })
            .await?;
            let Some(value) = value else {
                return Ok(None);
            };
            let info = value
                .decode()
                .map_err(|e| DataError::Decode(format!("decode AccountInfo: {e}")))?;
            account_info_view(&info)
        }
        RuntimeKind::Melodie => {
            let value = with_timeout("fetch_account", async {
                at.storage()
                    .try_fetch(melodie::storage().system().account(), (id,))
                    .await
                    .map_err(|e| DataError::Rpc(format!("fetch system.account({address}): {e}")))
            })
            .await?;
            let Some(value) = value else {
                return Ok(None);
            };
            let info = value
                .decode()
                .map_err(|e| DataError::Decode(format!("decode AccountInfo: {e}")))?;
            account_info_view(&info)
        }
    };
    Ok(Some(map_account_info(&id, &info, ss58_prefix)))
}

/// Read-only field projection across both runtimes' `frame_system::AccountInfo`
/// codegen types. Both runtimes lay the struct out identically — `nonce`
/// at the top, balances under `data` — but the codegen produces distinct
/// Rust types per module. Implementing one trait per type lets the
/// per-runtime decode arms collapse into a single
/// [`account_info_view`] call instead of needing a runtime-suffixed
/// helper apiece.
trait AccountInfoFields {
    fn free(&self) -> u128;
    fn reserved(&self) -> u128;
    fn frozen(&self) -> u128;
    fn nonce(&self) -> u32;
}

impl AccountInfoFields
    for allfeat::runtime_types::frame_system::AccountInfo<
        u32,
        allfeat::runtime_types::pallet_balances::types::AccountData<u128>,
    >
{
    fn free(&self) -> u128 { self.data.free }
    fn reserved(&self) -> u128 { self.data.reserved }
    fn frozen(&self) -> u128 { self.data.frozen }
    fn nonce(&self) -> u32 { self.nonce }
}

impl AccountInfoFields
    for melodie::runtime_types::frame_system::AccountInfo<
        u32,
        melodie::runtime_types::pallet_balances::types::AccountData<u128>,
    >
{
    fn free(&self) -> u128 { self.data.free }
    fn reserved(&self) -> u128 { self.data.reserved }
    fn frozen(&self) -> u128 { self.data.frozen }
    fn nonce(&self) -> u32 { self.nonce }
}

fn account_info_view<T: AccountInfoFields>(info: &T) -> AccountInfoView {
    AccountInfoView {
        free: info.free(),
        reserved: info.reserved(),
        frozen: info.frozen(),
        nonce: info.nonce(),
    }
}

/// Hard cap on the `System::Account` scan so a prod chain with millions of
/// accounts can't freeze the explorer in a single `/accounts` load. When the
/// real account set exceeds this, the top-N below is computed on the first
/// `MAX_SCAN_ACCOUNTS` keys the node surfaces — which is deterministic (storage
/// iter order) but biased; Phase 7 will introduce an off-chain index that
/// tracks top-N incrementally to remove the cap. The bound is logged at WARN
/// when hit so operators notice.
const MAX_SCAN_ACCOUNTS: usize = 10_000;

/// Iterate `System::Account` and return the top-N richest accounts by total
/// balance (free + reserved), sorted descending.
///
/// The iteration walks the storage map since there's no index by balance
/// on-chain; it stops at [`MAX_SCAN_ACCOUNTS`] so a single request can't scan
/// a 100k-account chain. The Phase 7 caching layer will turn this into an
/// incrementally-maintained snapshot.
pub async fn fetch_top_accounts(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    count: u32,
    runtime_kind: RuntimeKind,
    ss58_prefix: u16,
) -> DataResult<Vec<Account>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    with_iter_timeout("fetch_top_accounts", async {
        // Each arm walks its own runtime-typed iterator and lowers the
        // codegen `AccountInfo` into a runtime-neutral
        // `(AccountId32, AccountInfoView)` before pushing into `accounts`.
        let mut accounts: Vec<Account> = Vec::new();
        let mut truncated = false;
        match runtime_kind {
            RuntimeKind::Allfeat => {
                let mut entries = at
                    .storage()
                    .iter(allfeat::storage().system().account(), ())
                    .await
                    .map_err(|e| DataError::Rpc(format!("iter system.account: {e}")))?;
                while let Some(kv) = entries.next().await {
                    if accounts.len() >= MAX_SCAN_ACCOUNTS {
                        truncated = true;
                        break;
                    }
                    let kv = kv
                        .map_err(|e| DataError::Rpc(format!("iter system.account next: {e}")))?;
                    let (id,) = kv
                        .key()
                        .map_err(|e| DataError::Decode(format!("system.account key: {e}")))?
                        .decode()
                        .map_err(|e| {
                            DataError::Decode(format!("decode system.account key: {e}"))
                        })?;
                    let info = kv
                        .value()
                        .decode()
                        .map_err(|e| DataError::Decode(format!("decode AccountInfo: {e}")))?;
                    let view = account_info_view(&info);
                    accounts.push(map_account_info(&id, &view, ss58_prefix));
                }
            }
            RuntimeKind::Melodie => {
                let mut entries = at
                    .storage()
                    .iter(melodie::storage().system().account(), ())
                    .await
                    .map_err(|e| DataError::Rpc(format!("iter system.account: {e}")))?;
                while let Some(kv) = entries.next().await {
                    if accounts.len() >= MAX_SCAN_ACCOUNTS {
                        truncated = true;
                        break;
                    }
                    let kv = kv
                        .map_err(|e| DataError::Rpc(format!("iter system.account next: {e}")))?;
                    let (id,) = kv
                        .key()
                        .map_err(|e| DataError::Decode(format!("system.account key: {e}")))?
                        .decode()
                        .map_err(|e| {
                            DataError::Decode(format!("decode system.account key: {e}"))
                        })?;
                    let info = kv
                        .value()
                        .decode()
                        .map_err(|e| DataError::Decode(format!("decode AccountInfo: {e}")))?;
                    let view = account_info_view(&info);
                    accounts.push(map_account_info(&id, &view, ss58_prefix));
                }
            }
        }
        if truncated {
            tracing::warn!(
                scanned = accounts.len(),
                limit = MAX_SCAN_ACCOUNTS,
                "top_accounts: System::Account scan truncated; top-N may miss high-balance tail",
            );
        }
        accounts.sort_by_key(|a| std::cmp::Reverse(a.balance.total));
        accounts.truncate(count as usize);
        Ok(accounts)
    })
    .await
}

/// Fetch `System::Account` at the block `at` refers to for every entry in
/// `accounts`, preserving input order. Reaped (`None`) entries come back
/// as all-zero [`AccountSnapshot`]s so the caller can UPSERT them
/// unconditionally — the sink's `GREATEST` on `nonce` prevents a reap
/// from regressing a previously-observed counter.
///
/// Fetches run concurrently with a fixed cap of [`SNAPSHOT_CONCURRENCY`]
/// in-flight storage reads. `buffered` (not `buffer_unordered`) keeps
/// the output aligned with `accounts`, which matters for tests and for
/// any future caller that wants to zip the result back against the
/// touched-accounts list.
pub async fn fetch_accounts_at(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    accounts: &[[u8; 32]],
    runtime_kind: RuntimeKind,
) -> DataResult<Vec<([u8; 32], AccountSnapshot)>> {
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    stream::iter(accounts.iter().copied())
        .map(|bytes| async move {
            let id = AccountId32::from(bytes);
            let snapshot = match runtime_kind {
                RuntimeKind::Allfeat => {
                    let value = at
                        .storage()
                        .try_fetch(allfeat::storage().system().account(), (id,))
                        .await
                        .map_err(|e| {
                            DataError::Rpc(format!(
                                "fetch system.account({}): {e}",
                                hex_short(&bytes)
                            ))
                        })?;
                    match value {
                        Some(v) => {
                            let info = v.decode().map_err(|e| {
                                DataError::Decode(format!(
                                    "decode AccountInfo for {}: {e}",
                                    hex_short(&bytes)
                                ))
                            })?;
                            let view = account_info_view(&info);
                            AccountSnapshot {
                                free: view.free,
                                reserved: view.reserved,
                                frozen: view.frozen,
                                nonce: view.nonce,
                            }
                        }
                        None => AccountSnapshot::default(),
                    }
                }
                RuntimeKind::Melodie => {
                    let value = at
                        .storage()
                        .try_fetch(melodie::storage().system().account(), (id,))
                        .await
                        .map_err(|e| {
                            DataError::Rpc(format!(
                                "fetch system.account({}): {e}",
                                hex_short(&bytes)
                            ))
                        })?;
                    match value {
                        Some(v) => {
                            let info = v.decode().map_err(|e| {
                                DataError::Decode(format!(
                                    "decode AccountInfo for {}: {e}",
                                    hex_short(&bytes)
                                ))
                            })?;
                            let view = account_info_view(&info);
                            AccountSnapshot {
                                free: view.free,
                                reserved: view.reserved,
                                frozen: view.frozen,
                                nonce: view.nonce,
                            }
                        }
                        None => AccountSnapshot::default(),
                    }
                }
            };
            Ok::<_, DataError>((bytes, snapshot))
        })
        .buffered(SNAPSHOT_CONCURRENCY)
        .try_collect()
        .await
}

fn hex_short(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(12);
    for b in &bytes[..4] {
        out.push_str(&format!("{b:02x}"));
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_info_view_fixture(
        free: u128,
        reserved: u128,
        frozen: u128,
        nonce: u32,
    ) -> AccountInfoView {
        AccountInfoView {
            free,
            reserved,
            frozen,
            nonce,
        }
    }

    #[test]
    fn parse_ss58_roundtrips_account_id() {
        let id = AccountId32::from([3u8; 32]);
        let encoded = id.to_string();
        let parsed = parse_ss58(&encoded).expect("well-formed SS58 decodes");
        let parsed_bytes: &[u8; 32] = parsed.as_ref();
        let id_bytes: &[u8; 32] = id.as_ref();
        assert_eq!(parsed_bytes, id_bytes);
    }

    #[test]
    fn parse_ss58_rejects_garbage() {
        assert!(parse_ss58("not-an-address").is_none());
        assert!(parse_ss58("").is_none());
    }

    #[test]
    fn map_account_info_sums_total_and_uses_free_for_transferable() {
        let id = AccountId32::from([9u8; 32]);
        let info = account_info_view_fixture(100, 40, 10, 7);
        // Prefix 42 keeps the expected string aligned with subxt's built-in
        // `Display` so the assertion stays readable.
        let account = map_account_info(&id, &info, 42);
        assert_eq!(account.address, id.to_string());
        assert_eq!(account.nonce, 7);
        assert_eq!(account.balance.transferable, 100);
        assert_eq!(account.balance.reserved, 40);
        // `frozen` is an overlap with `free` (a transfer restriction), not a
        // separate bucket — total must exclude it.
        assert_eq!(account.balance.total, 140);
        assert_eq!(account.first_seen_ms, 0);
        assert_eq!(account.last_active_ms, 0);
    }

    #[test]
    fn map_account_info_handles_zero_balance() {
        let id = AccountId32::from([0u8; 32]);
        let info = account_info_view_fixture(0, 0, 0, 0);
        let account = map_account_info(&id, &info, 42);
        assert_eq!(account.balance.total, 0);
        assert_eq!(account.balance.transferable, 0);
        assert_eq!(account.balance.reserved, 0);
    }

    /// The runtime-specific projections must agree on the `(free, reserved,
    /// frozen, nonce)` they extract — they're the only places where the
    /// codegen-typed `AccountInfo` fields are read, so a regression here
    /// (e.g. a renamed field on one side) would silently corrupt every
    /// per-runtime balance read.
    #[test]
    fn account_info_views_agree_across_runtimes() {
        use allfeat::runtime_types::frame_system::AccountInfo as AllfeatAccountInfo;
        use allfeat::runtime_types::pallet_balances::types::{
            AccountData as AllfeatAccountData, ExtraFlags as AllfeatExtraFlags,
        };
        use melodie::runtime_types::frame_system::AccountInfo as MelodieAccountInfo;
        use melodie::runtime_types::pallet_balances::types::{
            AccountData as MelodieAccountData, ExtraFlags as MelodieExtraFlags,
        };

        let allfeat = AllfeatAccountInfo {
            nonce: 7,
            consumers: 0,
            providers: 1,
            sufficients: 0,
            data: AllfeatAccountData {
                free: 100,
                reserved: 40,
                frozen: 10,
                flags: AllfeatExtraFlags(0),
            },
        };
        let melodie = MelodieAccountInfo {
            nonce: 7,
            consumers: 0,
            providers: 1,
            sufficients: 0,
            data: MelodieAccountData {
                free: 100,
                reserved: 40,
                frozen: 10,
                flags: MelodieExtraFlags(0),
            },
        };

        let a = account_info_view(&allfeat);
        let m = account_info_view(&melodie);
        assert_eq!(a.free, m.free);
        assert_eq!(a.reserved, m.reserved);
        assert_eq!(a.frozen, m.frozen);
        assert_eq!(a.nonce, m.nonce);
    }
}
