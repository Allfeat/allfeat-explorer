//! Phase 6 — integration coverage for the ATS projection + the
//! `IndexedProvider` ATS routes.
//!
//! The plan (`docs/indexing-plan.md` §7 — "Phase 6 — ATS") asks for
//! three scenarios:
//!
//! * `registry_matches_rpc_scan` — create a handful of fresh ATS
//!   entries on the dev node, then for each one reached via the RPC
//!   mapper (`build_ats_record`) verify `IndexedProvider::ats_by_id`
//!   returns the same owner / created_block / version_count. Using
//!   fresh entries keeps the test tractable on any dev node — we never
//!   assume a pre-existing ATS.
//!
//! * `version_feed_pagination` — the feed query must tile
//!   non-overlapping pages: `page(0..N) ∪ page(N..2N) == feed(2N)` with
//!   no duplicates. A fresh registry entry with at least one update
//!   gives us two feed rows to slice across pages.
//!
//! * `stats_counters_consistent` — after N creations, `ats_stats`
//!   reports `total = N` and `total_versions ≥ N` — a count that agrees
//!   with the underlying `COUNT(*)` / `SUM(version_count)`. Guards
//!   against drift between the incremental `version_count` bumps and
//!   the aggregate queries.
//!
//! All tests are `#[ignore]` so a `cargo test` without the dev node +
//! Postgres-test stack still passes; CI runs them with `--ignored`.

#![cfg(all(feature = "ssr", not(feature = "mock")))]

mod common;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use allfeat_explorer::data::filters::AtsFeedFilters;
use allfeat_explorer::data::indexed::IndexedProvider;
use allfeat_explorer::data::rpc::runtime::allfeat;
use allfeat_explorer::data::rpc::{RpcClient, RpcProvider};
use allfeat_explorer::data::ChainData;
use allfeat_explorer::domain::PageRequest;
use allfeat_explorer::indexer::live::LiveWorker;
use allfeat_explorer::indexer::sink;
use allfeat_explorer::network::{ChainCtx, RuntimeKind, ALLFEAT};
use subxt::utils::{AccountId32, MultiAddress};
use subxt_signer::sr25519::{dev, Keypair};
use subxt_signer::SecretUri;

use common::{dev_node_url, fresh_db, fresh_lookups, lookup_cell, wait_for_cursor, TEST_NETWORK};

fn allfeat_rpc_provider(client: Arc<RpcClient>) -> RpcProvider {
    let mut clients: HashMap<&'static str, Arc<RpcClient>> = HashMap::new();
    clients.insert(ALLFEAT.id, client);
    RpcProvider::new(clients)
}

fn allfeat_ctx() -> ChainCtx {
    ChainCtx::new(&ALLFEAT, 0)
}

/// Derive a fresh keypair from a dev-style URI. Unique across tests
/// so parallel runs don't collide on the same nonce.
fn fresh_keypair(uri: &str) -> Keypair {
    let parsed = SecretUri::from_str(uri).expect("URI parses");
    Keypair::from_uri(&parsed).expect("keypair derives")
}

/// Fund a fresh key with enough AFT to pay for several ATS deposits.
/// Returns the block number funding was finalized at so callers can
/// bound their `wait_for_cursor` loop.
///
/// `funder` is taken explicitly so parallel tests don't collide on the
/// same nonce queue — the dev node rejects concurrent same-nonce
/// submissions with a priority error. Each test picks a different dev
/// funder (alice/bob/charlie) to stay lock-free under `cargo test`'s
/// default threaded harness.
async fn fund(
    api: &subxt::client::OnlineClient<subxt::SubstrateConfig>,
    funder: &Keypair,
    recipient: &AccountId32,
) -> u64 {
    // 100 AFT is well over the sum of a handful of version deposits
    // plus the inclusion fees — any fresh key is ready to create + update
    // ATS entries immediately.
    let amount: u128 = 100_000_000_000_000;
    let tx = allfeat::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(*recipient), amount);
    submit_signed(api, funder, tx).await.block_num
}

/// Outcome of a signed submission: the finalized block number + the
/// decoded runtime events. Callers that need to scrape an
/// event-carried id (e.g. `AtsCreated.ats_id` when multiple tests race
/// on `NextAtsId`) use the events; the block number is handed off to
/// `wait_for_cursor` to gate on the indexer catching up.
struct Submitted {
    block_num: u64,
    events: subxt::extrinsics::ExtrinsicEvents<subxt::SubstrateConfig>,
}

/// Submit `tx_payload` signed by `signer`, wait for finalization, and
/// return the hosting block's number plus the dispatched events. A
/// failed dispatch panics with the runtime error — the test then
/// surfaces the dev-node's response instead of a generic "nothing
/// happened on the indexer side".
async fn submit_signed<Call>(
    api: &subxt::client::OnlineClient<subxt::SubstrateConfig>,
    signer: &Keypair,
    tx_payload: Call,
) -> Submitted
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
    let in_block = progress
        .wait_for_finalized()
        .await
        .expect("transaction reaches a finalized block");
    let events = in_block
        .wait_for_success()
        .await
        .expect("tx succeeded on chain");
    let block_hash = in_block.block_hash();
    let block = api
        .at_block(block_hash)
        .await
        .expect("pin finalized block for number resolution");
    Submitted {
        block_num: block.block_number() as u64,
        events,
    }
}

/// Find the `Ats.AtsCreated` event in a submission and return its
/// `ats_id`. Panics if the create extrinsic didn't emit one (the
/// dispatch succeeded but no event — metadata drift worth surfacing).
fn ats_created_id(sub: &Submitted) -> u64 {
    sub.events
        .find_first::<allfeat::ats::events::AtsCreated>()
        .expect("decode AtsCreated events")
        .expect("create extrinsic emits AtsCreated")
        .ats_id
}

/// Create a fresh ATS signed by `signer`. Returns `(block_num, ats_id)`
/// — the id is scraped from `AtsCreated` so parallel tests racing on
/// `NextAtsId` each get their own exact id instead of computing one
/// from a stale read of the next-id storage value.
async fn create_ats(
    api: &subxt::client::OnlineClient<subxt::SubstrateConfig>,
    signer: &Keypair,
    commitment: [u8; 32],
) -> (u64, u64) {
    let tx = allfeat::tx().ats().create(commitment, 1);
    let sub = submit_signed(api, signer, tx).await;
    let ats_id = ats_created_id(&sub);
    (sub.block_num, ats_id)
}

/// Update an existing ATS (bumping it to the next version index).
async fn update_ats(
    api: &subxt::client::OnlineClient<subxt::SubstrateConfig>,
    signer: &Keypair,
    ats_id: u64,
    commitment: [u8; 32],
) -> u64 {
    let tx = allfeat::tx().ats().update(ats_id, commitment, 1);
    submit_signed(api, signer, tx).await.block_num
}

/// Create N fresh ATS entries from a single signer. Returns the
/// `(block_num, ats_id)` pairs in creation order. Ids are scraped
/// from the AtsCreated event so parallel tests racing on NextAtsId
/// each get the exact id their tx landed under.
async fn create_many(
    api: &subxt::client::OnlineClient<subxt::SubstrateConfig>,
    signer: &Keypair,
    tag: u8,
    n: u64,
) -> Vec<(u64, u64)> {
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        // Commitments must differ so each tx is distinct; use the
        // per-test tag + an increment so a failed test's on-chain
        // state is easy to correlate with the test case.
        let mut commitment = [0x00; 32];
        commitment[0] = tag;
        commitment[1] = (i & 0xff) as u8;
        out.push(create_ats(api, signer, commitment).await);
    }
    out
}

#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn registry_matches_rpc_scan() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), ALLFEAT.id, 42, RuntimeKind::Allfeat));
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
    let key = fresh_keypair("//AtsPhase6RegistryMatch");
    let signer_account: AccountId32 = key.public_key().into();
    // Funded from alice — other Phase 6 tests fund from bob / charlie
    // so the three can run under the default threaded harness without
    // racing on the same nonce queue.
    let funded = fund(&api, &dev::alice(), &signer_account).await;
    wait_for_cursor(&pool, sink::LIVE_CURSOR, funded, Duration::from_secs(30)).await;

    let created = create_many(&api, &key, 0xAE, 3).await;
    let last_block = created
        .iter()
        .map(|(b, _)| *b)
        .max()
        .expect("at least one creation");
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        last_block,
        Duration::from_secs(60),
    )
    .await;

    // Oracle: the RPC mapper walks storage + pins each version's
    // creator block to build `AtsRecord`. Both providers expose
    // `ats_by_id` keyed by the chain's `ats_id` (the URL shape the UI
    // uses: `/ats/<ats_id>`), so we look each seeded entry up directly.
    let rpc_provider = Arc::new(allfeat_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [ALLFEAT.id],
        rpc_provider.clone(),
        lookup_cell(networks.clone()),
    );

    for (_, ats_id) in &created {
        let id = u32::try_from(*ats_id).expect("ids in test range fit u32");
        let db_rec = provider
            .ats_by_id(allfeat_ctx(), id)
            .await
            .expect("ats_by_id succeeds")
            .unwrap_or_else(|| panic!("ats id {id} must be indexed"));
        let rpc_rec = rpc_provider
            .ats_by_id(allfeat_ctx(), id)
            .await
            .expect("rpc ats_by_id succeeds")
            .unwrap_or_else(|| panic!("ats id {id} must exist on chain"));

        assert_eq!(db_rec.ats_id, id, "DB returned wrong ats_id for {id}");
        assert_eq!(rpc_rec.ats_id, id, "RPC returned wrong ats_id for {id}");
        assert_eq!(db_rec.owner, rpc_rec.owner, "owner mismatch for {id}");
        assert_eq!(
            db_rec.created_at_block, rpc_rec.created_at_block,
            "created_at_block mismatch for {id}",
        );
        assert_eq!(
            db_rec.version_count, rpc_rec.version_count,
            "version_count mismatch for {id}",
        );
    }

    worker.abort();
    let _ = worker.await;
}

#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn version_feed_pagination() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), ALLFEAT.id, 42, RuntimeKind::Allfeat));
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
    let key = fresh_keypair("//AtsPhase6FeedPagination");
    let signer_account: AccountId32 = key.public_key().into();
    let funded = fund(&api, &dev::bob(), &signer_account).await;
    wait_for_cursor(&pool, sink::LIVE_CURSOR, funded, Duration::from_secs(30)).await;

    // Two records, each updated once → 4 feed rows total. Enough to
    // exercise the `(0..2) ∪ (2..4)` pagination slice without racing a
    // busy dev chain that might have other ATS activity between our
    // creations.
    let created = create_many(&api, &key, 0xBE, 2).await;
    let last_create = created.iter().map(|(b, _)| *b).max().unwrap();
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        last_create,
        Duration::from_secs(60),
    )
    .await;

    let mut last_update: u64 = last_create;
    for (_, ats_id) in &created {
        let mut commitment = [0x77; 32];
        commitment[0] = 0xBE;
        commitment[1] = (*ats_id & 0xff) as u8;
        let block = update_ats(&api, &key, *ats_id, commitment).await;
        last_update = last_update.max(block);
    }
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        last_update,
        Duration::from_secs(60),
    )
    .await;

    let rpc_provider = Arc::new(allfeat_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [ALLFEAT.id],
        rpc_provider,
        lookup_cell(networks.clone()),
    );

    // Ask for more than we created so the feed still includes our rows
    // even if the dev node has ambient ATS activity from other tests
    // running in parallel. We then filter down to just our ids.
    let seeded_ids: std::collections::HashSet<u32> = created
        .iter()
        .map(|(_, id)| u32::try_from(*id).unwrap())
        .collect();

    let full = provider
        .ats_version_feed(
            allfeat_ctx(),
            PageRequest {
                count: 64,
                cursor: None,
            },
            AtsFeedFilters::default(),
        )
        .await
        .expect("full feed ok");
    let ours_full: Vec<_> = full
        .items
        .iter()
        .filter(|f| seeded_ids.contains(&f.ats_id))
        .cloned()
        .collect();
    assert!(
        ours_full.len() >= 4,
        "feed must carry at least our 2 creations + 2 updates (got {})",
        ours_full.len(),
    );

    // Pagination contract: disjoint slices, concatenation equals the
    // prefix they cover. With cursor-based pagination, we chain
    // `page_a.page_info.next_cursor` into the second fetch.
    let page_size: u32 = 3;
    let page_a = provider
        .ats_version_feed(
            allfeat_ctx(),
            PageRequest {
                count: page_size,
                cursor: None,
            },
            AtsFeedFilters::default(),
        )
        .await
        .expect("page_a ok");
    let page_b_cursor = page_a
        .page_info
        .next_cursor
        .clone()
        .expect("page_a must have a next_cursor when more rows exist");
    let page_b = provider
        .ats_version_feed(
            allfeat_ctx(),
            PageRequest {
                count: page_size,
                cursor: Some(page_b_cursor),
            },
            AtsFeedFilters::default(),
        )
        .await
        .expect("page_b ok");
    let concat: Vec<_> = page_a
        .items
        .iter()
        .chain(page_b.items.iter())
        .cloned()
        .collect();

    assert_eq!(
        page_a.items.len(),
        page_size as usize,
        "page_a must be full"
    );
    assert!(
        page_b.items.len() <= page_size as usize,
        "page_b bounded by page_size"
    );

    // No duplicates across pages — a drift between cursor and ORDER BY
    // would show up as the same (ats_id, version) landing on both pages.
    let mut keys = std::collections::HashSet::new();
    for item in &concat {
        let key = (item.ats_id, item.version_index);
        assert!(
            keys.insert(key),
            "duplicate feed entry at {key:?} across pages"
        );
    }

    // Prefix equality: the first `page_a.len + page_b.len` rows of the
    // full feed must equal page_a ∪ page_b, preserving order.
    let expected: Vec<_> = full.items.iter().take(concat.len()).cloned().collect();
    assert_eq!(
        concat, expected,
        "paginated concat must match the corresponding prefix of the full feed",
    );

    worker.abort();
    let _ = worker.await;
}

#[tokio::test]
#[ignore = "requires a running dev node and postgres-test"]
async fn stats_counters_consistent() {
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let (networks, author_lookup) = fresh_lookups(&pool).await;
    let sid = networks.resolve(TEST_NETWORK).expect("TEST_NETWORK seeded");
    let client = Arc::new(RpcClient::new(dev_node_url(), ALLFEAT.id, 42, RuntimeKind::Allfeat));
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
    let key = fresh_keypair("//AtsPhase6StatsCounters");
    let signer_account: AccountId32 = key.public_key().into();
    let funded = fund(&api, &dev::charlie(), &signer_account).await;
    wait_for_cursor(&pool, sink::LIVE_CURSOR, funded, Duration::from_secs(30)).await;

    let before_stats = {
        let rpc = Arc::new(allfeat_rpc_provider(client.clone()));
        let provider = IndexedProvider::new(
            pool.clone(),
            [ALLFEAT.id],
            rpc,
            lookup_cell(networks.clone()),
        );
        provider
            .ats_stats(allfeat_ctx())
            .await
            .expect("stats before ok")
    };

    let created = create_many(&api, &key, 0xCE, 3).await;
    // Update one of them to push `total_versions` past `total`.
    let updated_block = update_ats(&api, &key, created[0].1, [0xCF; 32]).await;
    let last_block = created
        .iter()
        .map(|(b, _)| *b)
        .max()
        .unwrap()
        .max(updated_block);
    wait_for_cursor(
        &pool,
        sink::LIVE_CURSOR,
        last_block,
        Duration::from_secs(60),
    )
    .await;

    let rpc = Arc::new(allfeat_rpc_provider(client.clone()));
    let provider = IndexedProvider::new(
        pool.clone(),
        [ALLFEAT.id],
        rpc,
        lookup_cell(networks.clone()),
    );
    let after_stats = provider
        .ats_stats(allfeat_ctx())
        .await
        .expect("stats after ok");

    assert!(
        after_stats.total >= before_stats.total + 3,
        "total must grow by at least 3 (before={}, after={})",
        before_stats.total,
        after_stats.total,
    );
    assert!(
        after_stats.total_versions >= before_stats.total_versions + 4,
        "total_versions must grow by at least 4 (3 creates + 1 update) (before={}, after={})",
        before_stats.total_versions,
        after_stats.total_versions,
    );

    // Counter vs aggregate parity: the incremental version_count bumps
    // applied by the sink must agree with the on-DB SUM(version_count).
    let sum_version_count: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(version_count), 0)::bigint FROM ats_registry \
          WHERE network_id = $1",
    )
    .bind(TEST_NETWORK)
    .fetch_one(&pool)
    .await
    .expect("sum(version_count)");
    let registry_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ats_registry WHERE network_id = $1")
            .bind(TEST_NETWORK)
            .fetch_one(&pool)
            .await
            .expect("count(ats_registry)");
    assert_eq!(
        sum_version_count.unwrap_or(0) as u32,
        after_stats.total_versions,
        "ats_stats.total_versions must equal SUM(ats_registry.version_count)",
    );
    assert_eq!(
        registry_count as u32, after_stats.total,
        "ats_stats.total must equal COUNT(ats_registry)",
    );

    worker.abort();
    let _ = worker.await;
}
