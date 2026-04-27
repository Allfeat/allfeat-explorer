//! Live RPC integration tests against a dev node.
//!
//! All tests in this file are `#[ignore]` by default: they require a running
//! node on `ws://127.0.0.1:9944` (override the Allfeat-flavoured suite via
//! `ALLFEAT_RPC_URL`, the Melodie-flavoured suite via `MELODIE_RPC_URL`).
//! CI runs `cargo test` (no network); developers opt in with
//! `cargo test -- --ignored`.
//!
//! The bulk of the file targets an Allfeat dev node; a smaller
//! Melodie-tagged section at the bottom (`live_melodie_*`) exercises the
//! sibling codegen + metadata blob against a Melodie dev node, so a
//! regression that silently routes one runtime through the other's
//! decoders fails here rather than at a pallet-specific page far
//! downstream. The two suites can't share a node — pick one and run the
//! matching subset.
//!
//! Keep these tests smoky and cheap — the richer assertions live in the unit
//! tests under `#[cfg(test)]` inside the mapper modules.

// Integration suite only covers the RPC path, so skip it entirely when the
// mock feature is on (the rpc modules aren't compiled in that build).
#![cfg(all(feature = "ssr", not(feature = "mock")))]

use std::collections::HashMap;
use std::sync::Arc;

use allfeat_explorer::data::filters::{
    AccountAtsFilters, AtsFeedFilters, AtsFilters, BlockFilters, TransferFilters,
};
use allfeat_explorer::data::rpc::{RpcClient, RpcProvider};
use allfeat_explorer::data::ChainData;
use allfeat_explorer::domain::PageRequest;
use allfeat_explorer::network::{ChainCtx, RuntimeKind, ALLFEAT, MELODIE};

fn endpoint() -> String {
    std::env::var("ALLFEAT_RPC_URL").unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string())
}

/// Build an `RpcProvider` keyed for the Allfeat spec, pointing at the configured
/// dev node. All Block-flavoured integration tests share this constructor so
/// the test surface mirrors how `AppState::from_config` wires the real server.
fn allfeat_provider() -> RpcProvider {
    let mut clients: HashMap<&'static str, Arc<RpcClient>> = HashMap::new();
    clients.insert(ALLFEAT.id, Arc::new(RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat)));
    RpcProvider::new(clients)
}

fn allfeat_ctx() -> ChainCtx {
    // `now_ms` doesn't matter for RPC-backed reads — the provider derives all
    // chain quantities from the node, not from `ctx.now_ms`. Pass 0.
    ChainCtx::new(&ALLFEAT, 0)
}

/// Connects to the dev node, then resolves the genesis hash via subxt. Proves
/// the client wiring + metadata are usable end-to-end.
#[tokio::test]
#[ignore]
async fn live_connect_and_fetch_genesis() {
    let client = RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat);
    let api = client
        .subxt()
        .await
        .expect("connect to dev node (is ws://127.0.0.1:9944 up?)");

    let genesis = api.genesis_hash();
    assert_ne!(
        genesis.as_ref(),
        &[0u8; 32],
        "genesis hash must be non-zero"
    );
}

/// Fetches the latest finalized block via the public `OnlineClient` API — the
/// same path the provider will use for head queries. Asserts the header
/// round-trips.
#[tokio::test]
#[ignore]
async fn live_latest_block_fetches() {
    let client = RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat);
    let api = client.subxt().await.expect("connect to dev node");

    let at = api
        .at_current_block()
        .await
        .expect("fetch latest finalized block");
    // Dev nodes start at 0 and advance quickly; just assert the call returned
    // a valid reference by touching the number.
    let _ = at.block_number();
}

/// Genesis is the one block whose invariants are independent of dev-node
/// activity, so it's the cheapest end-to-end check that the RPC mapper
/// stitches the header together correctly.
#[tokio::test]
#[ignore]
async fn live_block_0_is_genesis() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let block = provider
        .block_by_number(ctx, 0)
        .await
        .expect("RPC succeeds")
        .expect("genesis block (#0) must exist on a running dev node");

    assert_eq!(block.number, 0, "block 0 should report number 0");
    assert_eq!(
        block.parent_hash,
        format!("0x{}", "00".repeat(32)),
        "genesis parent_hash must be the zero hash",
    );
    assert!(
        block.hash.starts_with("0x") && block.hash.len() == 2 + 64,
        "hash should be 0x + 64 hex chars, got {}",
        block.hash,
    );
    assert!(block.finalized, "genesis is always finalized");
}

/// `latest_blocks(N)` should agree with `RpcClient` directly on what the head
/// is and return a contiguous descending window. Strict number assertions
/// (`block.number == head`) would race the node, so we just check the shape.
#[tokio::test]
#[ignore]
async fn live_head_matches_best() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let page = provider
        .latest_blocks(
            ctx,
            PageRequest {
                count: 3,
                cursor: None,
            },
            BlockFilters::default(),
        )
        .await
        .expect("latest_blocks succeeds");
    assert!(
        !page.items.is_empty(),
        "head window should never be empty on a live node",
    );

    // The window is built newest-first from a single `latest_finalized_block_ref`
    // snapshot, so the numbers must form a contiguous descending sequence.
    for pair in page.items.windows(2) {
        assert_eq!(
            pair[0].number,
            pair[1].number + 1,
            "blocks must descend by 1: got {} then {}",
            pair[0].number,
            pair[1].number,
        );
    }

    // And every block in that window is at-or-below the finalized head, so
    // each must report `finalized = true`.
    for block in &page.items {
        assert!(
            block.finalized,
            "block #{} returned by latest_blocks must be finalized",
            block.number,
        );
    }

    // The new envelope carries `total` (chain head + 1) for cheap client-side
    // rendering. It must be populated and consistent with the window.
    let total = page.page_info.total.expect("total should be populated");
    assert!(
        total > page.items[0].number,
        "total={total} should exceed newest block #{}",
        page.items[0].number,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3 — Extrinsics
// ─────────────────────────────────────────────────────────────────────────────

/// Every non-genesis Allfeat block has `timestamp.set` as its first extrinsic
/// (the Substrate timestamp inherent runs in index 0). Proves the mapper
/// decodes the inherent via the shared metadata path and surfaces a `now`
/// field in the decoded args.
#[tokio::test]
#[ignore]
async fn live_block_1_has_timestamp_set_inherent() {
    use allfeat_explorer::domain::ExtrinsicArgs;

    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    // Block 1 is the earliest block that carries a timestamp inherent (#0 has
    // no extrinsics). Using 1 keeps the assertion stable across dev-node runs.
    let xs = provider
        .extrinsics_in_block(ctx, 1)
        .await
        .expect("extrinsics_in_block(1) succeeds");
    assert!(
        !xs.is_empty(),
        "block 1 must contain at least the timestamp inherent",
    );

    let first = &xs[0];
    assert_eq!(first.index, 0, "timestamp inherent sits at index 0");
    assert_eq!(first.module, "Timestamp");
    assert_eq!(first.call, "set");
    assert!(!first.signed, "timestamp inherent is unsigned");
    assert!(first.signer.is_none());
    match &first.args {
        ExtrinsicArgs::Decoded { fields } => {
            let now = fields
                .iter()
                .find(|f| f.name.as_deref() == Some("now"))
                .expect("Timestamp.set exposes a `now` field in runtime metadata");
            let parsed: u64 = now
                .value
                .parse()
                .unwrap_or_else(|_| panic!("`now` not a plain integer: {:?}", now.value));
            assert!(parsed > 0, "Timestamp.set decoded to zero: {parsed}");
        }
        other => panic!("expected Decoded args, got {other:?}"),
    }
}

/// Roundtrip the canonical `"<block>-<index>"` id: fetch block 1's
/// extrinsics, then re-fetch by id and verify the two agree field-for-field.
/// Covers `extrinsic_by_id` parsing and mapper stability.
#[tokio::test]
#[ignore]
async fn live_extrinsic_by_id_roundtrip() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let xs = provider
        .extrinsics_in_block(ctx, 1)
        .await
        .expect("extrinsics_in_block(1) succeeds");
    let first = xs.into_iter().next().expect("at least one extrinsic");
    let id = first.id.clone();

    let refetched = provider
        .extrinsic_by_id(ctx, &id)
        .await
        .expect("extrinsic_by_id succeeds")
        .expect("id from extrinsics_in_block must resolve");

    assert_eq!(refetched, first, "roundtrip must be identity");
}

/// Unknown ids resolve to `None` rather than erroring — keeps the UI's
/// "not found" path clean.
#[tokio::test]
#[ignore]
async fn live_extrinsic_by_id_missing_returns_none() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    // Way past any realistic dev-node head.
    let missing = provider
        .extrinsic_by_id(ctx, "99999999999-0")
        .await
        .expect("extrinsic_by_id must not error on missing ids");
    assert!(missing.is_none(), "future block should resolve to None");

    let malformed = provider
        .extrinsic_by_id(ctx, "not-a-valid-id")
        .await
        .expect("malformed id is a miss, not an error");
    assert!(malformed.is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5 — Accounts
// ─────────────────────────────────────────────────────────────────────────────

/// Alice is funded at genesis on every dev node, so resolving her SS58 via
/// `account_by_address` must return `Some` with a non-zero free balance and a
/// sensible nonce.
#[tokio::test]
#[ignore]
async fn live_alice_has_balance() {
    use subxt_signer::sr25519::dev;

    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let alice_ss58 = subxt::utils::AccountId32::from(dev::alice().public_key().0).to_string();
    let account = provider
        .account_by_address(ctx, &alice_ss58)
        .await
        .expect("account_by_address succeeds")
        .expect("Alice must exist on a dev node");

    assert_eq!(account.address, alice_ss58);
    assert!(
        account.balance.transferable > 0,
        "Alice should have a non-zero transferable balance on a dev node",
    );
    assert_eq!(
        account.balance.total,
        account
            .balance
            .transferable
            .saturating_add(account.balance.reserved),
        "total must be the free+reserved sum surfaced by the mapper",
    );
}

/// Unknown (but well-formed) addresses must not error. Substrate's
/// `System::Account` is a `ValueQuery` map: unprovisioned keys can surface
/// either as `None` or as a default-valued `AccountInfo` depending on node
/// backend. Both shapes are fine for the UI; what must not happen is an RPC
/// error bubbling into the page.
#[tokio::test]
#[ignore]
async fn live_account_missing_is_zero_or_none() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    // An all-`0xAA` key has no chance of colliding with a dev fixture.
    let unknown = subxt::utils::AccountId32::from([0xAAu8; 32]).to_string();
    let got = provider
        .account_by_address(ctx, &unknown)
        .await
        .expect("account_by_address must not error on unknown addresses");
    if let Some(account) = got {
        assert_eq!(
            account.balance.total, 0,
            "unprovisioned account must have zero total balance",
        );
        assert_eq!(account.nonce, 0, "unprovisioned account must have nonce 0");
    }
}

/// Malformed SS58 input must resolve to `None` (mapped by the parser), not
/// propagate as an RPC error — that keeps the "not found" page clean when
/// someone pastes garbage into the URL.
#[tokio::test]
#[ignore]
async fn live_account_malformed_address_returns_none() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let got = provider
        .account_by_address(ctx, "not-a-real-address")
        .await
        .expect("malformed address must not error");
    assert!(got.is_none());
}

/// `top_accounts` must return a descending-sorted window. Also exercises the
/// full storage iteration path and the sort truncation.
#[tokio::test]
#[ignore]
async fn live_top_accounts_sorted_desc() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let top = provider
        .top_accounts(ctx, 5)
        .await
        .expect("top_accounts succeeds");
    assert!(
        !top.is_empty(),
        "dev node must have at least the Alice/Bob/... fixtures",
    );
    for pair in top.windows(2) {
        assert!(
            pair[0].balance.total >= pair[1].balance.total,
            "top_accounts must be sorted descending by total balance",
        );
    }
    assert!(top.len() <= 5, "count cap must be honoured");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 4 — Transfers
// ─────────────────────────────────────────────────────────────────────────────

/// Submit Alice→Bob via subxt's transaction API, wait for finalization, then
/// assert the provider surfaces a matching `Transfer` within its scan window.
/// Exercises both the `Balances.Transfer` event mapping and the extrinsic
/// correlation (phase → id) end-to-end.
#[tokio::test]
#[ignore]
async fn live_alice_to_bob_transfer_shows_up() {
    use allfeat_explorer::data::rpc::runtime::allfeat;
    use subxt_signer::sr25519::dev;

    let client = allfeat_explorer::data::rpc::RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat);
    let api = client.subxt().await.expect("connect to dev node");

    let amount: u128 = 1_234_567_890_000;
    let dest = dev::bob().public_key().into();
    let tx = allfeat::tx().balances().transfer_keep_alive(dest, amount);
    let alice = dev::alice();

    let at = api
        .at_current_block()
        .await
        .expect("at_current_block before submit");
    let events = at
        .transactions()
        .sign_and_submit_then_watch_default(&tx, &alice)
        .await
        .expect("submit Alice→Bob transfer")
        .wait_for_finalized_success()
        .await
        .expect("transfer finalized");

    let transferred = events
        .find_first::<allfeat::balances::events::Transfer>()
        .expect("TxEvents query succeeds")
        .expect("Balances.Transfer present in transfer events");
    assert_eq!(
        transferred.amount, amount,
        "event amount should match the submitted one",
    );

    // Now verify the provider-side mapper picks it up. The latest_transfers
    // scan window (200 blocks) comfortably covers a fresh submission on a
    // dev chain; request a generous count to avoid pagination flakiness.
    let provider = allfeat_provider();
    let page = provider
        .latest_transfers(
            allfeat_ctx(),
            PageRequest {
                count: 50,
                cursor: None,
            },
            TransferFilters::default(),
        )
        .await
        .expect("latest_transfers succeeds");

    let alice_ss58 = subxt::utils::AccountId32::from(dev::alice().public_key().0).to_string();
    let bob_ss58 = subxt::utils::AccountId32::from(dev::bob().public_key().0).to_string();
    let matched = page
        .items
        .iter()
        .find(|t| t.from == alice_ss58 && t.to == bob_ss58 && t.amount == amount);
    assert!(
        matched.is_some(),
        "Alice→Bob transfer not found in mapper output: {:#?}",
        page.items,
    );
    let matched = matched.unwrap();
    let ext = &matched.extrinsic;
    assert!(ext.signed, "the linked extrinsic should be signed by Alice");
    assert_eq!(ext.module, "Balances");
    assert!(ext.call.starts_with("transfer"));
    assert_eq!(
        ext.signer.as_deref(),
        Some(alice_ss58.as_str()),
        "signer must SS58-round-trip back to Alice",
    );
    assert!(ext.fee > 0, "a signed transfer must report a non-zero fee");
    assert!(
        ext.events
            .iter()
            .any(|e| e.module == "TransactionPayment" && e.name == "TransactionFeePaid"),
        "extrinsic events must include TransactionFeePaid (fee source)",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 6 — ATS
// ─────────────────────────────────────────────────────────────────────────────

/// End-to-end lifecycle: submit one ATS as Alice, then add a second version,
/// and verify the provider-side view captures the record + its two versions.
///
/// Covers `ats_by_index(0)`, `ats_list`, `ats_version_feed`, `account_ats`,
/// `account_ats_count`, and `ats_stats` in a single pass so the dev-node
/// state we produce is exercised from every entry point.
#[tokio::test]
#[ignore]
async fn live_alice_ats_create_and_update() {
    use allfeat_explorer::data::rpc::runtime::allfeat;
    use subxt_signer::sr25519::dev;

    let provider = allfeat_provider();
    let ctx = allfeat_ctx();
    let client = allfeat_explorer::data::rpc::RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat);
    let api = client.subxt().await.expect("connect to dev node");

    // Commitment / protocol_version unique per run so if the test is executed
    // repeatedly against the same dev node, the newest entry is always ours.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut commitment = [0u8; 32];
    commitment[..8].copy_from_slice(&nanos.to_be_bytes());
    let protocol_version: u8 = 1;

    // Create.
    let create_tx = allfeat::tx().ats().create(commitment, protocol_version);
    let alice = dev::alice();
    let at = api
        .at_current_block()
        .await
        .expect("at_current_block before create");
    let events = at
        .transactions()
        .sign_and_submit_then_watch_default(&create_tx, &alice)
        .await
        .expect("submit ats.create")
        .wait_for_finalized_success()
        .await
        .expect("ats.create finalized");
    let created = events
        .find_first::<allfeat::ats::events::AtsCreated>()
        .expect("events query succeeds")
        .expect("AtsCreated present after ats.create");
    assert_eq!(created.commitment, commitment);
    assert_eq!(created.protocol_version, protocol_version);
    let new_id = created.ats_id;

    // Update with a second version (different commitment).
    let mut commitment_v2 = commitment;
    commitment_v2[31] ^= 0xFF;
    let update_tx = allfeat::tx()
        .ats()
        .update(new_id, commitment_v2, protocol_version);
    let at = api
        .at_current_block()
        .await
        .expect("at_current_block before update");
    let events = at
        .transactions()
        .sign_and_submit_then_watch_default(&update_tx, &alice)
        .await
        .expect("submit ats.update")
        .wait_for_finalized_success()
        .await
        .expect("ats.update finalized");
    let updated = events
        .find_first::<allfeat::ats::events::AtsUpdated>()
        .expect("events query succeeds")
        .expect("AtsUpdated present after ats.update");
    assert_eq!(updated.ats_id, new_id);
    assert_eq!(updated.version, 1, "update must bump to version 1");

    // `ats_by_index(0)` resolves to the newest entry (our fresh ATS).
    let alice_ss58 = subxt::utils::AccountId32::from(dev::alice().public_key().0).to_string();
    let record = provider
        .ats_by_index(ctx, 0)
        .await
        .expect("ats_by_index(0) succeeds")
        .expect("newest ats must exist after create");
    assert_eq!(record.owner, alice_ss58, "owner must be Alice");
    assert_eq!(
        record.version_count, 2,
        "record must have two versions (initial + update)",
    );
    assert_eq!(record.versions.len(), 2);
    assert_eq!(
        record.versions[0].version_index, 0,
        "versions sorted ascending",
    );
    assert_eq!(record.versions[1].version_index, 1);
    assert_eq!(
        record.versions[0].commitment,
        format!("0x{}", hex::encode(commitment))
    );
    assert_eq!(
        record.versions[1].commitment,
        format!("0x{}", hex::encode(commitment_v2))
    );
    assert!(
        record.versions.iter().all(|v| v.signer == alice_ss58),
        "both versions signed by Alice",
    );
    assert!(
        record.versions.iter().all(|v| v.fee > 0),
        "signed extrinsics must report a non-zero fee",
    );
    assert!(
        record.total_deposit > 0,
        "create + update must hold some deposit",
    );

    // `ats_list` includes it as the first (newest-first) entry.
    let list = provider
        .ats_list(
            ctx,
            PageRequest {
                count: 5,
                cursor: None,
            },
            AtsFilters::default(),
        )
        .await
        .expect("ats_list succeeds");
    assert!(
        list.items.iter().any(|r| r.ats_id == record.ats_id),
        "ats_list must surface the fresh record",
    );
    assert_eq!(
        list.items[0].ats_id, record.ats_id,
        "newest entry must come first",
    );

    // `ats_version_feed` includes both the create and the update rows.
    let feed = provider
        .ats_version_feed(
            ctx,
            PageRequest {
                count: 10,
                cursor: None,
            },
            AtsFeedFilters::default(),
        )
        .await
        .expect("ats_version_feed succeeds");
    let ours: Vec<_> = feed
        .items
        .iter()
        .filter(|f| f.ats_id == record.ats_id)
        .collect();
    assert_eq!(
        ours.len(),
        2,
        "feed must surface both create and update versions",
    );
    assert!(
        ours.iter().any(|f| f.is_initial && !f.is_latest),
        "version 0 must be flagged as initial",
    );
    assert!(
        ours.iter().any(|f| !f.is_initial && f.is_latest),
        "version 1 must be flagged as latest",
    );

    // `account_ats` returns a `Page<AtsRecord>` that carries the owner
    // total in `page_info.total`. The dedicated `account_ats_count`
    // endpoint was removed per the pagination plan — its caller was
    // counting through the list's total instead.
    let alice_ats = provider
        .account_ats(
            ctx,
            &alice_ss58,
            PageRequest {
                count: 10,
                cursor: None,
            },
            AccountAtsFilters::default(),
        )
        .await
        .expect("account_ats succeeds");
    let alice_count = alice_ats.page_info.total.unwrap_or(0);
    assert!(alice_count >= 1, "Alice must own at least our fresh ATS");
    assert!(
        alice_ats.items.iter().any(|r| r.ats_id == record.ats_id),
        "account_ats must include the freshly-created record",
    );

    // `ats_stats` aggregates at least our entry + its two versions.
    let stats = provider.ats_stats(ctx).await.expect("ats_stats succeeds");
    assert!(stats.total >= 1, "stats.total must count ≥ 1 record");
    assert!(
        stats.total_versions >= 2,
        "stats.total_versions must count our two versions: {stats:?}",
    );
    assert!(stats.unique_owners >= 1);
    assert!(stats.total_deposited > 0);
    assert_eq!(stats.protocol_version, 1);
}

/// Malformed / unknown addresses must return zero/empty, never error.
#[tokio::test]
#[ignore]
async fn live_account_ats_handles_unknown_address() {
    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    let req = || PageRequest {
        count: 10,
        cursor: None,
    };
    let records = provider
        .account_ats(ctx, "not-an-ss58", req(), AccountAtsFilters::default())
        .await
        .expect("must not error on malformed input");
    assert!(records.items.is_empty());
    assert_eq!(
        records.page_info.total.unwrap_or(0),
        0,
        "total must be zero for unknown input",
    );

    let records = provider
        .account_ats(
            ctx,
            &subxt::utils::AccountId32::from([0xBBu8; 32]).to_string(),
            req(),
            AccountAtsFilters::default(),
        )
        .await
        .expect("must not error on unknown address");
    assert!(records.items.is_empty());
    assert_eq!(records.page_info.total.unwrap_or(0), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 7 — Caching
// ─────────────────────────────────────────────────────────────────────────────

/// End-to-end proof that the finalized LRU in front of `block_by_number`
/// catches: genesis is the one block whose invariants don't change between
/// dev-node runs, so we can compare the first fetch (cold miss → real RPC)
/// with the second (LRU hit) and assert the hit is dramatically faster.
///
/// Tight wall-clock numbers would flake on loaded laptops, so we assert the
/// ratio rather than an absolute bound. A relaxed factor still pins the
/// regression we care about: "the provider stopped hitting the cache".
#[tokio::test]
#[ignore]
async fn live_block_by_number_hits_finalized_cache_on_second_call() {
    use std::time::Instant;

    let provider = allfeat_provider();
    let ctx = allfeat_ctx();

    // Warm the subxt `OnceCell` with a throwaway call — otherwise the first
    // timed call pays for the connection handshake, which dwarfs the RPC
    // itself and makes the miss/hit ratio meaningless.
    provider
        .block_by_number(ctx, 0)
        .await
        .expect("warm-up succeeds");

    // Use a fresh provider so both calls go through an empty cache first,
    // then a populated one.
    let provider = allfeat_provider();
    // Prime subxt on the fresh provider too (connection handshake is shared
    // across tests via `SubxtClient::from_url`; giving this provider a
    // chance to open its own client makes the "miss" call a pure cache
    // lookup + one RPC, not a connection cost).
    provider
        .latest_blocks(
            ctx,
            PageRequest {
                count: 1,
                cursor: None,
            },
            BlockFilters::default(),
        )
        .await
        .expect("warm subxt client on fresh provider");

    let t0 = Instant::now();
    let first = provider
        .block_by_number(ctx, 0)
        .await
        .expect("first fetch succeeds");
    let miss = t0.elapsed();

    let t1 = Instant::now();
    let second = provider
        .block_by_number(ctx, 0)
        .await
        .expect("second fetch succeeds");
    let hit = t1.elapsed();

    assert_eq!(first, second, "cached copy must equal the live fetch");
    assert!(first.is_some(), "genesis must exist on a running dev node",);
    assert!(
        hit < miss / 4,
        "second call should be served from the finalized LRU \
         (miss: {miss:?}, hit: {hit:?}) — if this fails the provider \
         bypassed `cached(...)` on block_by_number",
    );
}

/// 50 concurrent callers asking for the same finalized block must collapse
/// to a single underlying RPC thanks to `try_get_with`'s singleflight. We
/// can't cleanly count RPC calls on a real `SubxtClient`, so we rely on
/// the cumulative wall-clock: 50 sequential RPCs would take orders of
/// magnitude longer than 50 coalesced ones.
#[tokio::test]
#[ignore]
async fn live_block_by_number_coalesces_concurrent_misses() {
    use std::time::Instant;

    let provider = Arc::new(allfeat_provider());
    let ctx = allfeat_ctx();

    let t = Instant::now();
    let mut handles = Vec::with_capacity(50);
    for _ in 0..50 {
        let provider = provider.clone();
        handles.push(tokio::spawn(async move {
            provider
                .block_by_number(ctx, 0)
                .await
                .expect("block fetch succeeds")
        }));
    }
    for h in handles {
        let block = h.await.expect("task joined");
        assert!(block.is_some(), "genesis must resolve for every caller");
    }
    let elapsed = t.elapsed();

    // A generous bound: a single-round-trip RPC against a local dev node is
    // sub-50ms. Even accounting for metadata / codec overhead on a cold
    // cache, 50 parallel coalesced misses should finish well under 3s. If
    // singleflight broke and each task issued its own RPC we'd see
    // seconds-per-task serialization against the WS connection.
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "50 concurrent block_by_number(0) calls took {elapsed:?} — \
         singleflight coalescing in `cached(...)` likely regressed",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 8 — Robustness
// ─────────────────────────────────────────────────────────────────────────────

/// A successful connect spawns a finalized-block subscription; against a
/// live dev node producing blocks every few seconds the watch must be
/// populated well before the timeout. A missing value after the wait
/// window would mean the subscription task either failed to spawn or the
/// stream isn't producing — both are Phase 8 regressions.
#[tokio::test]
#[ignore]
async fn live_finalized_head_subscription_publishes() {
    use std::time::{Duration, Instant};

    let client = allfeat_explorer::data::rpc::RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat);
    // Trigger connect → spawns the subscription task as a side effect.
    let _ = client.subxt().await.expect("connect to dev node");

    // The dev node produces blocks on a ~3–6s cadence; give the
    // subscription a generous window before declaring it broken.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(head) = client.finalized_head() {
            // Any positive head value proves the stream delivered at least
            // one item and we published it to the watch.
            assert!(
                head > 0,
                "finalized head should be > 0 after first notification"
            );
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "finalized-head watch still None 30s after connect — \
                 subscription task likely isn't publishing",
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Invalidate after a healthy connect must both re-connect on the next
/// `subxt()` call and reset the watch to `None` — without the reset, a
/// stale finalized head could leak across a disconnect / reconnect cycle
/// and make `Block::finalized` lie for blocks produced during the gap.
#[tokio::test]
#[ignore]
async fn live_invalidate_clears_finalized_head_watch() {
    use std::time::{Duration, Instant};

    let client = allfeat_explorer::data::rpc::RpcClient::new(endpoint(), ALLFEAT.id, 42, RuntimeKind::Allfeat);
    let _ = client.subxt().await.expect("connect to dev node");

    // Wait for the first notification so the watch is populated.
    let deadline = Instant::now() + Duration::from_secs(30);
    while client.finalized_head().is_none() {
        if Instant::now() >= deadline {
            panic!("subscription never published — cannot test invalidate");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    client.invalidate().await;
    assert_eq!(
        client.finalized_head(),
        None,
        "invalidate must reset the finalized-head watch",
    );

    // Next subxt() call must succeed (reconnect) without panicking.
    let _ = client
        .subxt()
        .await
        .expect("reconnect after invalidate must succeed");
}

// ----- Melodie sibling-runtime smoke tests ------------------------------
//
// These tests stand up an `RpcClient` tagged with `RuntimeKind::Melodie`
// so the Melodie codegen module + bundled metadata blob get exercised
// end-to-end. They share no state with the Allfeat block above — point
// `MELODIE_RPC_URL` at a Melodie dev node before running them, and the
// Allfeat suite at an Allfeat dev node. Both suites are `#[ignore]`,
// so a default `cargo test` is unaffected.
//
// Coverage is intentionally minimal: connect + decode a genesis block.
// A regression that silently swapped the Melodie blob for the Allfeat
// one (or routed Melodie traffic through Allfeat codegen types) fails
// here rather than at a pallet-specific page far downstream.

fn melodie_endpoint() -> String {
    std::env::var("MELODIE_RPC_URL").unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string())
}

fn melodie_provider() -> RpcProvider {
    let mut clients: HashMap<&'static str, Arc<RpcClient>> = HashMap::new();
    clients.insert(
        MELODIE.id,
        Arc::new(RpcClient::new(
            melodie_endpoint(),
            MELODIE.id,
            42,
            RuntimeKind::Melodie,
        )),
    );
    RpcProvider::new(clients)
}

fn melodie_ctx() -> ChainCtx {
    ChainCtx::new(&MELODIE, 0)
}

/// Connects to a Melodie dev node and resolves the genesis hash through
/// the Melodie-tagged client. Smoke check that the sibling codegen + the
/// `MELODIE_RUNTIME` metadata bundle are wired together correctly — a
/// stale or mis-routed blob would surface as a connect or decode error
/// here rather than as silent corruption deeper in.
#[tokio::test]
#[ignore]
async fn live_melodie_connect_and_fetch_genesis() {
    let client = RpcClient::new(
        melodie_endpoint(),
        MELODIE.id,
        42,
        RuntimeKind::Melodie,
    );
    let api = client
        .subxt()
        .await
        .expect("connect to Melodie dev node (is MELODIE_RPC_URL up?)");

    let genesis = api.genesis_hash();
    assert_ne!(
        genesis.as_ref(),
        &[0u8; 32],
        "genesis hash must be non-zero"
    );
}

/// Round-trips block 0 through the `RuntimeKind::Melodie` dispatch in
/// every mapper that touches Timestamp/Session/System on the read path.
/// Same shape contract as `live_block_0_is_genesis` — the per-runtime
/// match arms must produce the same structural result for the genesis
/// block on either chain.
#[tokio::test]
#[ignore]
async fn live_melodie_block_0_is_genesis() {
    let provider = melodie_provider();
    let ctx = melodie_ctx();

    let block = provider
        .block_by_number(ctx, 0)
        .await
        .expect("RPC succeeds")
        .expect("genesis block (#0) must exist on a running Melodie dev node");

    assert_eq!(block.number, 0, "block 0 should report number 0");
    assert_eq!(
        block.parent_hash,
        format!("0x{}", "00".repeat(32)),
        "genesis block parent must be all zeros",
    );
}
