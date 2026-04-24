//! Integration coverage for the `account_balances` snapshot pipeline
//! and the indexed `account_by_address` / `top_accounts` reads.
//!
//! The indexer rebuilds `account_balances` from `System::Account` reads
//! at each block's hash (see
//! [`crate::indexer::projections::accounts::collect_touched`] +
//! [`crate::data::rpc::mappers::accounts::fetch_accounts_at`]). These
//! tests prove that for accounts we fully control (fresh, non-genesis
//! keys) the DB's row equals the chain's state byte-for-byte after
//! finalization.
//!
//! * `balance_matches_system_account` — a known transfer into a fresh
//!   recipient. DB and chain must agree on free/reserved/nonce.
//! * `signer_balance_matches_after_fee` — the signer side of a signed
//!   extrinsic (which the old delta pipeline double-counted, because
//!   both `Balances::Withdraw` for the prepaid fee and
//!   `TransactionPayment::TransactionFeePaid` landed in the aggregate).
//!   Under the snapshot pipeline the signer's DB row is just
//!   `System::Account` so this test would catch any regression back to
//!   event-delta accumulation.
//! * `top_accounts_ordered` — the `(free + reserved) DESC` index must
//!   back a monotonically-decreasing listing.
//! * `nonce_advances_on_signed_extrinsic` — a fresh key's nonce must
//!   climb to exactly 1 after its first signed extrinsic.
//!
//! All are `#[ignore]` so `cargo test` without the dev node +
//! Postgres-test stack still passes; CI runs them with `--ignored`.

#![cfg(all(feature = "ssr", not(feature = "mock")))]

mod common;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use allfeat_explorer::data::indexed::IndexedProvider;
use allfeat_explorer::data::rpc::runtime::allfeat;
use allfeat_explorer::data::rpc::{RpcClient, RpcProvider};
use allfeat_explorer::data::ChainData;
use allfeat_explorer::indexer::live::LiveWorker;
use allfeat_explorer::indexer::sink;
use allfeat_explorer::network::{ChainCtx, MELODIE};
use subxt::utils::{AccountId32, MultiAddress};
use subxt_signer::sr25519::{dev, Keypair};
use subxt_signer::SecretUri;

use common::{dev_node_url, fresh_db, fresh_lookups, lookup_cell, wait_for_cursor, TEST_NETWORK};

fn melodie_rpc_provider(client: Arc<RpcClient>) -> RpcProvider {
    let mut clients: HashMap<&'static str, Arc<RpcClient>> = HashMap::new();
    clients.insert(MELODIE.id, client);
    RpcProvider::new(clients)
}

fn melodie_ctx() -> ChainCtx {
    ChainCtx::new(&MELODIE, 0)
}

/// Derive a fresh (non-genesis) keypair from a dev-style URI. The URIs
/// we use never collide with `//Alice`, `//Bob`, etc. — they're
/// guaranteed to start at zero balance + zero nonce on every dev chain.
fn fresh_keypair(uri: &str) -> Keypair {
    let parsed = SecretUri::from_str(uri).expect("URI parses");
    Keypair::from_uri(&parsed).expect("keypair derives")
}

/// Read `System::Account.{free, reserved, nonce}` at the chain's current
/// finalized head via subxt. Tests use this as the on-chain oracle
/// against [`IndexedProvider::account_by_address`].
async fn fetch_rpc_state(client: &RpcClient, address: &AccountId32) -> (u128, u128, u32) {
    let api = client.subxt().await.expect("subxt client");
    let at = api
        .at_current_block()
        .await
        .expect("at_current_block for rpc oracle");
    let value = at
        .storage()
        .try_fetch(allfeat::storage().system().account(), (*address,))
        .await
        .expect("fetch system.account")
        .expect("account must exist after receiving a transfer");
    let info = value.decode().expect("decode AccountInfo");
    (info.data.free, info.data.reserved, info.nonce)
}

/// Submit `tx_payload` signed by `signer`, wait for finalization, then
/// resolve the hosting block's number via `at_block(hash)`. Returns the
/// finalized block number — the caller uses it to gate the
/// `wait_for_cursor` poll on the indexer.
async fn submit_signed<Call>(
    api: &subxt::client::OnlineClient<subxt::SubstrateConfig>,
    signer: &Keypair,
    tx_payload: Call,
) -> u64
where
    Call: subxt::tx::Payload,
{
    let at = api
        .at_current_block()
        .await
        .expect("at_current_block before submit");
    let progress = at
        .transactions()
        .sign_and_submit_then_watch_default(&tx_payload, signer)
        .await
        .expect("sign and submit");
    // `wait_for_finalized` returns a `TransactionInBlock` which carries
    // the finalized block hash. We explicitly turn that into a block
    // number via `at_block` — simpler than reaching into the extrinsic
    // events just to pull the number.
    let in_block = progress
        .wait_for_finalized()
        .await
        .expect("transaction reaches a finalized block");
    // Drain the result into a success check so a failing dispatch
    // surfaces loudly instead of a silent "nothing happened" on the
    // indexer side.
    let _events = in_block
        .wait_for_success()
        .await
        .expect("tx succeeded on chain");
    let block_hash = in_block.block_hash();
    let block = api
        .at_block(block_hash)
        .await
        .expect("pin finalized block for number resolution");
    block.block_number() as u64
}

/// Transfer a known amount from Alice to a brand-new (derived, non-
/// genesis) key, then compare the recipient's DB balance against
/// `System::Account` at the finalized head.
///
/// The non-genesis recipient is what makes the test tractable: starting
/// from zero on both sides means strict equality is achievable without
/// any seed reconciliation. The snapshot pipeline fetches the recipient
/// directly from `System::Account` at the block where the Transfer
/// lands, so DB ≡ chain by construction — the assertion guards against
/// the fan-out silently dropping touched accounts.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn balance_matches_system_account() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let worker = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();
    // Kick the cursor forward so the worker is past any pre-submission
    // block; otherwise the first block we care about could land before
    // the watch-channel ever fires and the test would poll forever.
    wait_for_cursor(&pool, sink::LIVE_CURSOR, 0, Duration::from_secs(15)).await;

    let api = client.subxt().await.expect("connect to dev node");
    let recipient = fresh_keypair("//AccountsPhase5Recipient");
    let recipient_account: AccountId32 = recipient.public_key().into();

    // Pick something well above any plausible existential deposit so
    // the row survives reaping; 10 AFT is a safe ceiling.
    let amount: u128 = 10_000_000_000_000;
    let tx = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(recipient_account), amount);

    let block_num = submit_signed(&api, &dev::alice(), tx).await;
    wait_for_cursor(&pool, sink::LIVE_CURSOR, block_num, Duration::from_secs(30)).await;

    let rpc_provider = Arc::new(melodie_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc_provider,
        lookup_cell(networks.clone()),
    );

    let recipient_ss58 = recipient_account.to_string();
    let db_account = provider
        .account_by_address(melodie_ctx(), &recipient_ss58)
        .await
        .expect("account_by_address succeeds")
        .expect("recipient must exist after the transfer");

    let (rpc_free, rpc_reserved, rpc_nonce) = fetch_rpc_state(&client, &recipient_account).await;

    assert_eq!(
        db_account.balance.transferable, rpc_free,
        "DB.free must match System::Account.data.free for a fresh recipient \
         (rpc={rpc_free}, db={})",
        db_account.balance.transferable,
    );
    assert_eq!(
        db_account.balance.reserved, rpc_reserved,
        "DB.reserved must match System::Account.data.reserved",
    );
    assert_eq!(
        db_account.nonce, rpc_nonce,
        "a fresh never-signing recipient's nonce must remain zero on both sides",
    );
    assert_eq!(
        db_account.balance.transferable, amount,
        "recipient's balance must equal the transferred amount",
    );

    worker.abort();
    let _ = worker.await;
}

/// Same shape as `balance_matches_system_account` but from the
/// **signer's** side: fund a fresh key, have it sign its own outgoing
/// transfer, then compare its DB balance against `System::Account`.
///
/// This is the test the previous delta pipeline would have failed. On
/// a signed extrinsic Substrate emits both `Balances::Withdraw` (for
/// the prepaid max-fee) and `TransactionPayment::TransactionFeePaid`
/// (for the actual fee after refund). Summing both as signed deltas
/// left the signer's DB row short by roughly one fee per extrinsic —
/// invisible on single-block assertions (which historically only
/// checked the recipient) but catastrophic across a populated chain.
/// The snapshot pipeline reads `System::Account` directly, so strict
/// equality must hold here.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn signer_balance_matches_after_fee() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let worker = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();
    wait_for_cursor(&pool, sink::LIVE_CURSOR, 0, Duration::from_secs(15)).await;

    let api = client.subxt().await.expect("connect to dev node");

    // Fresh signer starts at zero. Fund it from Alice with enough to
    // cover a transfer + its fee, well above any plausible existential
    // deposit so the row survives reaping.
    let signer = fresh_keypair("//AccountsSignerFeeMatch");
    let signer_account: AccountId32 = signer.public_key().into();
    let funding: u128 = 50_000_000_000_000;
    let fund_tx = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(signer_account), funding);
    let funding_block = submit_signed(&api, &dev::alice(), fund_tx).await;
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        funding_block,
        Duration::from_secs(30),
    )
    .await;

    // Signer now sends a known amount to Bob. The signer's new balance
    // = funding - sent - actual_fee. The old pipeline would subtract
    // (sent + actual_fee + withdraw_for_fee), i.e. roughly one extra fee.
    let bob: AccountId32 = dev::bob().public_key().into();
    let sent: u128 = 2_000_000_000_000;
    let send_tx = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(bob), sent);
    let send_block = submit_signed(&api, &signer, send_tx).await;
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        send_block,
        Duration::from_secs(30),
    )
    .await;

    let rpc_provider = Arc::new(melodie_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc_provider,
        lookup_cell(networks.clone()),
    );
    let signer_ss58 = signer_account.to_string();
    let db_account = provider
        .account_by_address(melodie_ctx(), &signer_ss58)
        .await
        .expect("account_by_address succeeds")
        .expect("signer row exists after signed extrinsic");

    let (rpc_free, rpc_reserved, rpc_nonce) = fetch_rpc_state(&client, &signer_account).await;

    assert_eq!(
        db_account.balance.transferable, rpc_free,
        "signer DB.free must match System::Account.data.free \
         (rpc={rpc_free}, db={}) — a delta-accumulator bug would \
         typically show db = rpc - actual_fee here",
        db_account.balance.transferable,
    );
    assert_eq!(
        db_account.balance.reserved, rpc_reserved,
        "signer DB.reserved must match System::Account.data.reserved",
    );
    assert_eq!(
        db_account.nonce, rpc_nonce,
        "signer DB.nonce must match System::Account.nonce after signing",
    );

    worker.abort();
    let _ = worker.await;
}

/// After a couple of transfers to fresh (non-genesis) keys with
/// different amounts, `top_accounts(N)` must come back sorted by
/// `free + reserved DESC` and the bigger transfer must outrank the
/// smaller one.
///
/// Strict equality with the chain would fail here — genesis accounts
/// we don't index would leak in with zero DB balances. Ordering
/// (plus the two fresh keys we control) is enough to prove the index
/// + query are doing their job.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn top_accounts_ordered() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let worker = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();
    wait_for_cursor(&pool, sink::LIVE_CURSOR, 0, Duration::from_secs(15)).await;

    let api = client.subxt().await.expect("connect to dev node");

    let big_key = fresh_keypair("//AccountsPhase5TopBig");
    let small_key = fresh_keypair("//AccountsPhase5TopSmall");
    let big_account: AccountId32 = big_key.public_key().into();
    let small_account: AccountId32 = small_key.public_key().into();

    let big: u128 = 20_000_000_000_000;
    let small: u128 = 5_000_000_000_000;

    let tx_big = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(big_account), big);
    let tx_small = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(small_account), small);

    // Signed by different source accounts so the two submissions don't
    // serialise on the same nonce queue. Their finalization order
    // isn't guaranteed, so we wait on the later block number.
    let b1 = submit_signed(&api, &dev::alice(), tx_big).await;
    let b2 = submit_signed(&api, &dev::bob(), tx_small).await;
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        b1.max(b2),
        Duration::from_secs(30),
    )
    .await;

    let rpc_provider = Arc::new(melodie_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc_provider,
        lookup_cell(networks.clone()),
    );

    // Generous N — the worker may have indexed ambient activity (fee
    // movements, inherents, etc.) beyond our two fresh keys.
    let top = provider
        .top_accounts(melodie_ctx(), 50)
        .await
        .expect("top_accounts succeeds");

    assert!(!top.is_empty(), "top_accounts must return at least one row");
    for pair in top.windows(2) {
        assert!(
            pair[0].balance.total >= pair[1].balance.total,
            "top_accounts must be sorted DESC: saw {} then {}",
            pair[0].balance.total,
            pair[1].balance.total,
        );
    }

    let big_pos = top
        .iter()
        .position(|a| a.address == big_account.to_string());
    let small_pos = top
        .iter()
        .position(|a| a.address == small_account.to_string());
    assert!(
        big_pos.is_some(),
        "bigger recipient must be in top_accounts",
    );
    assert!(
        small_pos.is_some(),
        "smaller recipient must be in top_accounts",
    );
    assert!(
        big_pos.unwrap() < small_pos.unwrap(),
        "bigger transfer must outrank smaller one in top_accounts",
    );

    worker.abort();
    let _ = worker.await;
}

/// A fresh signer's nonce must climb to exactly `ext.nonce + 1 = 1`
/// after a single signed extrinsic lands. Using a fresh key (funded
/// ad-hoc) makes the assertion airtight: the DB's view starts at zero,
/// the chain's view also starts at zero (the key has never signed), so
/// both sides must agree after the first signed extrinsic.
#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn nonce_advances_on_signed_extrinsic() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), 42));
    let worker = LiveWorker::new(
        TEST_NETWORK,
        sid,
        client.clone(),
        pool.clone(),
        author_lookup.clone(),
    )
    .spawn();
    wait_for_cursor(&pool, sink::LIVE_CURSOR, 0, Duration::from_secs(15)).await;

    let api = client.subxt().await.expect("connect to dev node");

    // Fund a fresh key so it can sign its own transfer back. Use a
    // deterministic derivation so the test is reproducible across runs.
    let fresh = fresh_keypair("//AccountsPhase5Signer");
    let fresh_account: AccountId32 = fresh.public_key().into();

    let funding_amount: u128 = 10_000_000_000_000;
    let fund_tx = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(fresh_account), funding_amount);
    let funding_block = submit_signed(&api, &dev::alice(), fund_tx).await;
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        funding_block,
        Duration::from_secs(30),
    )
    .await;

    // Fresh key signs its first extrinsic → on-chain nonce advances
    // from 0 to 1. The DB must report the same post-block nonce.
    let bob: AccountId32 = dev::bob().public_key().into();
    let tiny: u128 = 1_000_000_000_000;
    let signed_tx = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(bob), tiny);
    let signed_block = submit_signed(&api, &fresh, signed_tx).await;
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        signed_block,
        Duration::from_secs(30),
    )
    .await;

    let rpc_provider = Arc::new(melodie_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [MELODIE.id],
        rpc_provider,
        lookup_cell(networks.clone()),
    );

    let fresh_ss58 = fresh_account.to_string();
    let db_account = provider
        .account_by_address(melodie_ctx(), &fresh_ss58)
        .await
        .expect("account_by_address succeeds")
        .expect("fresh signer row exists after signed extrinsic");
    assert_eq!(
        db_account.nonce, 1,
        "DB nonce must equal ext.nonce + 1 = 1 after a single signed extrinsic \
         (saw {})",
        db_account.nonce,
    );

    let (_, _, rpc_nonce) = fetch_rpc_state(&client, &fresh_account).await;
    assert_eq!(
        db_account.nonce, rpc_nonce,
        "DB nonce must agree with System::Account.nonce (rpc={rpc_nonce}, db={})",
        db_account.nonce,
    );

    worker.abort();
    let _ = worker.await;
}
