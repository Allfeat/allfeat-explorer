#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use std::net::SocketAddr;
    use std::sync::Arc;

    #[cfg(not(feature = "mock"))]
    use allfeat_explorer::indexer;
    use allfeat_explorer::live::server::ws_handler;
    use allfeat_explorer::server::api;
    use allfeat_explorer::server::health::{healthz, readyz};
    use allfeat_explorer::server::{AppState, ServerConfig};
    use axum::http::{header, HeaderName, HeaderValue, Method};
    use axum::routing::get;
    use axum::Router;
    use tower_governor::governor::GovernorConfigBuilder;
    use tower_governor::key_extractor::SmartIpKeyExtractor;
    use tower_governor::GovernorLayer;
    use tower_http::cors::{AllowOrigin, CorsLayer};
    use tower_http::set_header::SetResponseHeaderLayer;
    use tower_http::trace::TraceLayer;
    use tracing_subscriber::{fmt, EnvFilter};

    // Install a tracing subscriber first thing so startup errors (config,
    // bind) already flow through the formatter. Default filter promotes
    // our own modules to `debug` without drowning the logs in subxt's
    // own `trace` output; ops can override with `RUST_LOG`.
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,allfeat_explorer=debug")),
        )
        .with_target(true)
        .with_thread_ids(false)
        .init();

    // `--mode=all|indexer|server` picks the deployment role (see
    // `docs/indexing-plan.md` §"Topologie de déploiement"). Parsing is
    // kept deliberately tiny — we expose one flag, and a bad value
    // aborts boot rather than silently falling through to a default
    // an operator didn't ask for.
    #[cfg(not(feature = "mock"))]
    let mode = parse_mode_flag().unwrap_or_else(|e| {
        eprintln!("invalid --mode flag: {e}");
        std::process::exit(1);
    });

    // `--reconcile` is a one-shot maintenance sweep: walk every row in
    // `account_balances`, refetch each account's `System::Account` at
    // the current head, and UPDATE drifted totals in place. Exits when
    // the sweep finishes (or fails). Kept as a flag on the normal
    // binary rather than a separate `--mode` value because it shares
    // the entire config surface (DATABASE_URL, RPC endpoints, indexed
    // networks) with the regular server; no separate bootstrap needed.
    #[cfg(not(feature = "mock"))]
    let reconcile_requested = parse_reconcile_flag();

    let config = ServerConfig::from_env().unwrap_or_else(|e| {
        eprintln!("invalid server config: {e}");
        std::process::exit(1);
    });

    // Phase 7 hardening: `indexer` and `server` modes are meaningless
    // without Postgres — one writes, the other reads. Catching this at
    // boot turns what would otherwise be a silent no-op (stuck workers,
    // empty responses) into an actionable exit message. The `all` mode
    // keeps tolerating a missing DB so onboarding workflows still work
    // against a dev binary.
    #[cfg(not(feature = "mock"))]
    if mode.requires_database() && config.database_url.is_none() {
        eprintln!(
            "--mode={mode} requires DATABASE_URL to be set (indexer writes / server reads Postgres)"
        );
        std::process::exit(2);
    }

    #[cfg(feature = "mock")]
    tracing::info!("data source: mock (compile-time)");
    #[cfg(not(feature = "mock"))]
    tracing::info!(endpoints = config.rpc_endpoints.len(), "data source: rpc");
    if config.ws_origin_wildcard() {
        tracing::warn!(
            "WS_ALLOWED_ORIGINS is unset or '*' — the /api/v1/live endpoint accepts any \
             origin. Set it to your public origin list before exposing the \
             service to the internet (see docs/deployment.md)."
        );
    } else {
        tracing::info!(
            origins = ?config.ws_allowed_origins,
            "ws origin allowlist active"
        );
    }
    if config.api_origin_wildcard() {
        tracing::warn!(
            "API_ALLOWED_ORIGINS is unset or '*' — CORS accepts any origin for \
             /api/v1/*. Set it to your frontend origin list in prod."
        );
    } else {
        tracing::info!(
            origins = ?config.api_allowed_origins,
            "api CORS allowlist active"
        );
    }
    let bind_is_wildcard = config.ws_origin_wildcard();
    let listen_addr: SocketAddr = config.listen_addr;
    let api_allowed_origins = config.api_allowed_origins.clone();
    let api_origin_wildcard = config.api_origin_wildcard();
    let app_state = AppState::from_config(config);

    // Apply pending migrations before anything writes to or reads from
    // Postgres. Owning the schema in-process means operators don't run
    // `sqlx migrate run` out-of-band — a fresh DB boots into a usable
    // state and an already-migrated DB is a no-op (idempotent on
    // `_sqlx_migrations`). Skip when no pool is configured (DB-less
    // `all` runs that fall back to RPC). The whole block is also gated
    // on `not(mock)` because mock builds never touch Postgres — every
    // other DB-using section below carries the same cfg, so an operator
    // running `dev:api` with `DATABASE_URL` happening to be exported in
    // their shell shouldn't get a spurious migration attempt.
    #[cfg(not(feature = "mock"))]
    if let Some(pool) = app_state.db_pool.as_ref() {
        if let Err(e) = allfeat_explorer::server::run_migrations(pool).await {
            eprintln!("failed to apply database migrations: {e}");
            std::process::exit(3);
        }
    }

    // Seed the `networks` lookup *after* migrations so the table
    // exists, and *before* the indexer workers or providers run any
    // query — every sink / queries call binds the SMALLINT network_id
    // resolved from this lookup.
    #[cfg(not(feature = "mock"))]
    if let Err(e) = app_state.seed_lookups().await {
        eprintln!("failed to seed network lookup table: {e}");
        std::process::exit(3);
    }

    // `--reconcile` short-circuits the normal boot. Sweeps every
    // indexed network in order, prints a per-network report, and exits
    // with a non-zero status if any of the sweeps failed. Runs *after*
    // migrations so a fresh DB is valid, but *before* the indexer
    // workers spawn — the sweep and the per-block pipeline both UPDATE
    // the same rows and concurrent runs invite a self-healing race
    // window (live writes stale, reconcile fixes it; next touch
    // re-writes it). Operators who care about strict determinism
    // should run this with the indexer stopped.
    #[cfg(not(feature = "mock"))]
    if reconcile_requested {
        let Some(pool) = app_state.db_pool.clone() else {
            eprintln!("--reconcile requires DATABASE_URL to be set");
            std::process::exit(2);
        };
        let networks: Vec<(&'static str, std::sync::Arc<_>)> = app_state
            .indexed_network_ids
            .iter()
            .filter_map(|id| {
                app_state
                    .indexer_clients
                    .get(*id)
                    .cloned()
                    .map(|c| (*id, c))
            })
            .collect();
        if networks.is_empty() {
            eprintln!("--reconcile: no indexed networks configured (check RPC_ENDPOINT_* env)");
            std::process::exit(4);
        }
        tracing::info!(
            networks = networks.len(),
            "reconcile: starting one-shot account_balances sweep"
        );
        let Some(network_lookup) = app_state.network_lookup.get() else {
            eprintln!("--reconcile: network lookup not initialised");
            std::process::exit(3);
        };
        let results =
            allfeat_explorer::indexer::reconcile::reconcile_all(networks, pool, network_lookup)
                .await;
        let mut any_failed = false;
        for (network_id, res) in &results {
            match res {
                Ok(r) => tracing::info!(
                    network = %network_id,
                    head = r.head_block,
                    scanned = r.accounts_scanned,
                    updated = r.rows_updated,
                    "reconcile: network converged"
                ),
                Err(e) => {
                    any_failed = true;
                    tracing::error!(network = %network_id, error = %e, "reconcile: network failed");
                }
            }
        }
        std::process::exit(if any_failed { 5 } else { 0 });
    }

    // Prometheus recorder pair. Built here — *before* the indexer
    // workers spawn — so any `metrics::counter!` / `gauge!` call the
    // workers emit during their first tick lands in the same registry
    // the `/metrics` route scrapes later. `axum-prometheus` also
    // installs the HTTP-level series (`axum_http_requests_total`, …)
    // for free via the returned layer.
    let (prometheus_layer, metric_handle) = axum_prometheus::PrometheusMetricLayer::pair();

    // Describe + prime the indexer-side series so a fresh scrape
    // returns the names even when the counters haven't moved yet.
    // Mock builds skip this — the mock provider has no indexer.
    #[cfg(not(feature = "mock"))]
    {
        let networks: Vec<&'static str> = app_state.indexed_network_ids.iter().copied().collect();
        allfeat_explorer::server::metrics::register_descriptions(&networks);
    }

    // Spawn the indexer workers before we start serving so a slow
    // first-block index doesn't race with the first page load. Every
    // configured network gets its own live + backfill pair; if the DB
    // is missing we skip entirely and the RPC fallback keeps pages
    // functional.
    #[cfg(not(feature = "mock"))]
    let _indexer_handles = {
        if mode.runs_indexer() {
            match app_state.db_pool.clone() {
                Some(pool) => {
                    let concurrency = app_state.config.indexer_backfill_concurrency;
                    let networks: Vec<(&'static str, std::sync::Arc<_>)> = app_state
                        .indexed_network_ids
                        .iter()
                        .filter_map(|id| {
                            app_state
                                .indexer_clients
                                .get(*id)
                                .cloned()
                                .map(|c| (*id, c))
                        })
                        .collect();
                    tracing::info!(
                        mode = %mode,
                        networks = networks.len(),
                        backfill_concurrency = concurrency,
                        "indexer: starting workers"
                    );
                    match app_state.network_lookup.get() {
                        Some(network_lookup) => indexer::spawn(
                            mode,
                            networks,
                            pool,
                            concurrency,
                            (**network_lookup).clone(),
                            app_state.author_lookup.clone(),
                        ),
                        None => {
                            tracing::error!(
                                mode = %mode,
                                "indexer: network lookup not initialised; workers not spawned"
                            );
                            Vec::new()
                        }
                    }
                }
                None => {
                    tracing::info!(
                        mode = %mode,
                        "indexer: DATABASE_URL unset — workers not spawned, serving via RPC fallback"
                    );
                    Vec::new()
                }
            }
        } else {
            tracing::info!(mode = %mode, "indexer: server-only mode, workers disabled");
            Vec::new()
        }
    };

    // Background task that keeps the derived lag gauges
    // (`indexer_lag_seconds`, `indexer_lag_blocks`,
    // `indexer_backfill_remaining_blocks`) honest, one refresher per
    // indexed network. A stuck worker stops emitting its own metrics,
    // so a dedicated refresher is the only way `/metrics` can surface
    // the stall — exactly when operators need the signal most.
    #[cfg(not(feature = "mock"))]
    let _pose_refreshers: Vec<_> = match (app_state.db_pool.clone(), app_state.network_lookup.get())
    {
        (Some(pool), Some(network_lookup)) => {
            let network_lookup = network_lookup.clone();
            app_state
                .indexed_network_ids
                .iter()
                .filter_map(|id| {
                    let sid = network_lookup.resolve(id)?;
                    let rx = app_state.indexer_clients.get(*id)?.watch_finalized_head();
                    Some(allfeat_explorer::server::metrics::spawn_pose_refresher(
                        id,
                        sid,
                        pool.clone(),
                        rx,
                        allfeat_explorer::server::metrics::POSE_REFRESH_INTERVAL,
                    ))
                })
                .collect()
        }
        _ => Vec::new(),
    };

    #[cfg(not(feature = "mock"))]
    if !mode.runs_http() {
        tracing::info!(mode = %mode, "indexer-only mode: HTTP stack disabled, blocking on shutdown signal");
        shutdown_signal().await;
        return;
    }

    // Listening on 0.0.0.0 + wildcard origins is the classic "directly on
    // the public Internet" foot-gun. A loopback bind is safe because the
    // reverse proxy is the one facing the network; an all-interfaces bind
    // with no origin allowlist almost certainly means TLS isn't terminated
    // in front either.
    if listen_addr.ip().is_unspecified() && bind_is_wildcard {
        tracing::warn!(
            %listen_addr,
            "binding all interfaces AND WS_ALLOWED_ORIGINS=* — this looks \
             like a direct-to-Internet deployment without a reverse proxy. \
             Put TLS + origin checks in front (see docs/deployment.md)."
        );
    }

    // Rate-limiting opt-out for test + local-dev scenarios. Playwright in
    // particular fans out parallel browser workers that blow past the
    // default REST budget on cold cache fetches; the escape hatch keeps
    // the prod layer honest while letting the suite run unthrottled.
    let disable_rate_limit = std::env::var("EXPLORER_DISABLE_RATE_LIMIT")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if disable_rate_limit {
        tracing::warn!(
            "EXPLORER_DISABLE_RATE_LIMIT set — /api/v1/* and /api/v1/live \
             are NOT rate-limited. Intended for local dev / e2e tests only."
        );
    }

    // Per-IP governor for `/api/v1/live` WebSocket upgrades. Five new
    // connections/second with a burst of ten is comfortably above a
    // real user flipping tabs while still crushing a bot dialling us
    // in a tight loop. `SmartIpKeyExtractor` prefers `X-Forwarded-For`
    // / `X-Real-IP` when present, so we key on the actual client IP
    // behind the reverse proxy.
    let ws_governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(5)
            .burst_size(10)
            .finish()
            .expect("static governor config must be valid"),
    );
    let ws_limiter = ws_governor.limiter().clone();

    // REST governor — a much wider budget than WS since a single page
    // load issues a handful of parallel requests (dashboard = 4-5
    // fetches) and the Nuxt SSR proxy compounds that. 100 req/s burst
    // 200 leaves plenty of room for bursty dashboards while still
    // catching an abusive script.
    let api_governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(100)
            .burst_size(200)
            .finish()
            .expect("static governor config must be valid"),
    );
    let api_limiter = api_governor.limiter().clone();

    // Background task that periodically evicts stale entries from both
    // limiters so long-running processes don't grow unbounded.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            ws_limiter.retain_recent();
            api_limiter.retain_recent();
        }
    });

    // CORS for `/api/v1/*`. Permissive in dev (wildcard), allowlist in
    // prod. GET + OPTIONS only — the REST surface is read-only, so no
    // point advertising the other methods in preflights.
    let cors_layer = if api_origin_wildcard {
        CorsLayer::permissive()
    } else {
        let origins: Vec<HeaderValue> = api_allowed_origins
            .iter()
            .filter_map(|s| s.parse::<HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::ACCEPT])
    };

    // REST router — the bulk of the `/api/v1` surface. CORS, rate
    // limiter and prometheus layer all apply here but *not* to the
    // health/metrics routers below, which kubelet polls on a short
    // interval and must always succeed.
    let api_router: Router = {
        let router = api::router(app_state.clone()).layer(cors_layer);
        if disable_rate_limit {
            router
        } else {
            router.layer(GovernorLayer::new(api_governor))
        }
    };

    // Live-stream WebSocket endpoint. Relocated under `/api/v1/live`
    // so the whole public surface lives under one path prefix.
    let ws_router: Router = {
        let router = Router::new()
            .route("/api/v1/live", get(ws_handler))
            .with_state(app_state.clone());
        if disable_rate_limit {
            router
        } else {
            router.layer(GovernorLayer::new(ws_governor))
        }
    };

    // Health / readiness probes. `/healthz` never touches upstream state
    // (kube's liveness probe toggles pod restarts — don't let a transient
    // RPC hiccup nuke the pod). `/readyz` reads provider state, so it
    // needs the shared `AppState`. Kept out of the rate-limited + CORS
    // layers so kubelet polls always resolve.
    let health_router: Router = Router::new()
        .route("/api/v1/healthz", get(healthz))
        .route("/api/v1/readyz", get(readyz))
        .with_state(app_state.clone());

    // Prometheus scrape endpoint. No state required — the handle moved
    // into the closure renders the currently collected metrics on every
    // call. Also kept out of CORS + rate limiting so the Prometheus
    // server can always reach it.
    let metrics_router: Router = Router::new().route(
        "/api/v1/metrics",
        get(move || async move { metric_handle.render() }),
    );

    // HTTP tracing: one span + access log per request. Cheap at INFO and
    // invaluable for spotting stuck handlers (long latency) or bogus
    // routes (404 spam) without reaching for a profiler.
    //
    // Security headers stamped on every response via `if_not_present`.
    // The backend serves JSON + WS only now, so the CSP that gated the
    // Leptos HTML doc is gone; the remaining trio (nosniff / referrer /
    // permissions) are still worth stamping on API responses so they
    // can't be mis-interpreted if a browser happens to navigate to one.
    let app = Router::new()
        .merge(ws_router)
        .merge(health_router)
        .merge(metrics_router)
        .merge(api_router)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
        ))
        .layer(prometheus_layer)
        .layer(TraceLayer::new_for_http());

    tracing::info!(%listen_addr, "listening");
    let listener = tokio::net::TcpListener::bind(&listen_addr).await.unwrap();
    // `into_make_service_with_connect_info::<SocketAddr>()` so the rate
    // limiter's `SmartIpKeyExtractor` can fall back to the peer address
    // when no forwarded header is present (direct dev connections).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .unwrap();
}

/// Resolve when the process receives either Ctrl-C or SIGTERM
/// (Unix-only). Used by `axum::serve(...).with_graceful_shutdown(...)` so
/// in-flight requests drain before the socket closes — kube / systemd
/// rollouts stop cutting WS sessions mid-frame.
#[cfg(feature = "ssr")]
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("ctrl_c handler install failed");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler install failed")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, draining in-flight requests");
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // Without the `ssr` feature the crate has no binary entry point —
    // the Rust side is backend-only after the v3 rewrite.
}

/// Pull `--mode=<value>` out of the argv. Accepts both `--mode=X` and
/// `--mode X` forms. Unknown flags are ignored (dev wrappers, runners and
/// plugins all like to pass extra args); missing flag collapses to the
/// default [`IndexerMode::All`].
#[cfg(all(feature = "ssr", not(feature = "mock")))]
fn parse_mode_flag() -> Result<allfeat_explorer::indexer::IndexerMode, String> {
    use allfeat_explorer::indexer::IndexerMode;
    use std::str::FromStr;

    let args: Vec<String> = std::env::args().collect();
    let mut it = args.iter().skip(1); // skip the binary path
    while let Some(arg) = it.next() {
        if let Some(rest) = arg.strip_prefix("--mode=") {
            return IndexerMode::from_str(rest);
        }
        if arg == "--mode" {
            let Some(next) = it.next() else {
                return Err("--mode requires a value".to_string());
            };
            return IndexerMode::from_str(next);
        }
    }
    Ok(IndexerMode::default())
}

/// Return `true` when the operator passed `--reconcile` (boolean flag,
/// no value). Any other argv is ignored — dev runners and wrappers
/// pass their own flags and we don't want the binary to reject them.
#[cfg(all(feature = "ssr", not(feature = "mock")))]
fn parse_reconcile_flag() -> bool {
    std::env::args().any(|a| a == "--reconcile")
}
