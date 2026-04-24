# Plan de durcissement — Sécurité & hardening

> **Document de travail vivant.** L'explorer est un service public en lecture
> seule — pas de cookies, pas d'auth, pas d'écriture on-chain côté serveur.
> Le threat model est donc : DDoS / abus de ressources, exfiltration via
> prompt injection (logs), et vulnérabilités via dépendances tierces.

## Suivi global

| # | Phase                                                       | Priorité | Statut |
|---|-------------------------------------------------------------|----------|--------|
| 1 | Origin check sur `/ws`                                      | 🟠 moy.  | ⬜     |
| 2 | Rate limiting des connexions WS par IP                      | 🔴 haute | ⬜     |
| 3 | Bornage des frames inbound par session                      | 🔴 haute | ⬜     |
| 4 | Headers de sécurité HTTP                                    | 🟠 moy.  | ⬜     |
| 5 | `/healthz` + `/metrics` (opérationnel)                      | 🟠 moy.  | ⬜     |
| 6 | Guardrails de déploiement (TLS + reverse proxy docs)        | 🟡 bas.  | ⬜     |
| 7 | Supply chain : `cargo-deny` + `cargo-audit` en CI           | 🟡 bas.  | ⬜     |

Légende : ⬜ à faire · 🟡 en cours · ✅ terminé · ⚠️ bloqué

---

## Phase 1 — Origin check sur `/ws`

### Problème
`src/live/server.rs::ws_handler` accepte **n'importe quelle origine**. Pas de
cookies aujourd'hui → impact réel faible. Mais :
- Un futur attrait d'auth (par ex. wallet connect, NFT gating) changerait la
  donne du jour au lendemain.
- Indexeurs tiers peuvent abuser du stream sans annoncer leur origine.

### Correction
Liste blanche configurable via env :
```rust
// src/server/config.rs
pub struct ServerConfig {
    ...
    pub ws_allowed_origins: Vec<String>,  // ex. ["https://explorer.allfeat.com"]
}
```

Dans `ws_handler`, extraire le header `Origin` et rejeter avec `403` si
non-match :
```rust
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<WsQuery>,
) -> Response {
    if !origin_allowed(&headers, &state.config.ws_allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ...
}
```

Défaut en dev : `ws_allowed_origins = vec!["*"]` pour ne pas casser
`cargo leptos dev`. En prod : `WS_ALLOWED_ORIGINS=https://explorer.allfeat.com`.

### Fichiers touchés
- `src/server/config.rs` : nouveau champ + parsing env
- `src/live/server.rs` : handler + helper `origin_allowed`
- `src/main.rs` : rien (la config est déjà propagée)

### Tests
- Unit : matrice origin × config (match exact, wildcard, rejected).
- Intégration : `curl -H "Origin: https://evil.example"` reçoit 403.

### Risque
Faible. Risque d'ops : oublier de setter l'env en prod = "*"  implicite.
Logger un WARN au boot si `ws_allowed_origins` contient "*".

---

## Phase 2 — Rate limiting connexions WS

### Problème
Aucune limite sur le nombre d'upgrades `/ws` par IP. Un bot peut ouvrir des
milliers de sessions : chaque session = 1 sink task + 1 receiver + heartbeat +
jusqu'à 3 forwarders (= 6 tokio tasks). 10k sessions = 60k tasks + 10k `Arc`
de broadcast Receiver = ~quelques centaines de MB.

### Correction

**Option A — `tower-governor`** (middleware axum natif) :
```toml
tower-governor = "0.4"
```
```rust
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};

let governor = Arc::new(GovernorConfigBuilder::default()
    .per_second(5)
    .burst_size(10)
    .finish()
    .unwrap());

let ws_router = Router::new()
    .route("/ws", get(ws_handler))
    .layer(GovernorLayer { config: governor })
    .with_state(app_state.clone());
```
Par défaut : clé = IP source (via `X-Forwarded-For` si derrière reverse
proxy).

**Option B — home-grown** : `DashMap<IpAddr, AtomicU32>` qui décrémente au
`Drop` de la session. Plus flexible mais plus de code à maintenir.

**Recommandation** : A, gain de code minimum, battle-tested.

### Paramètres à calibrer
- Connections per second = 5, burst 10 : raisonnable pour une lib explorer
  légitime (1 tab = 1 connexion).
- Per-IP cap simultanée (ex. 50 sockets) : à vérifier si
  `tower-governor` le supporte natif, sinon ajouter un `DashMap` compteur
  complémentaire.

### Fichiers touchés
- `Cargo.toml` : `tower-governor`
- `src/main.rs` : ajout du layer

### Tests
- `ab -c 100 -n 1000 http://localhost:3000/ws` (avec upgrade header) doit
  voir majorité de 429.

### Risque
Faible — en dev, configurer un `per_second` haut pour éviter les trolls.

---

## Phase 3 — Bornage des frames inbound

### Problème
`src/live/server.rs::run_receiver` accepte une frame texte arbitrairement
grosse et la passe à `serde_json::from_str`. Un client malveillant peut :
- Envoyer des frames de 100 MB → parsing JSON coûteux
- Spammer `Subscribe` / `Unsubscribe` → spawn/abort churn

### Correction

1. **Taille max par frame** : axum `WebSocket` accepte `WebSocketUpgrade::max_frame_size`
   et `max_message_size`. Par défaut c'est déjà raisonnable (64 KiB) mais
   vérifier et ajuster :
   ```rust
   ws.max_frame_size(4 * 1024)
     .max_message_size(8 * 1024)
     .on_upgrade(...)
   ```

2. **Rate-limit interne par session** : compteur de frames dans
   `run_receiver` :
   ```rust
   const MAX_FRAMES_PER_SEC: u32 = 20;
   let mut budget = MAX_FRAMES_PER_SEC;
   let mut window_start = Instant::now();

   while let Some(frame) = ws_rx.next().await {
       if window_start.elapsed() > Duration::from_secs(1) {
           window_start = Instant::now();
           budget = MAX_FRAMES_PER_SEC;
       }
       if budget == 0 {
           let _ = sink_tx.send(encode(&ServerMsg::Error { message: "rate limited".into() })).await;
           return;  // ferme la session
       }
       budget -= 1;
       ...
   }
   ```

3. **Cap dur nb de subscribe** : déjà partiellement géré par
   `forwarders.contains_key(&topic)` (3 topics max). Ajouter un compteur
   d'abort total (`total_subscriptions_ever`) pour catch un pattern
   `sub/unsub` en boucle.

### Fichiers touchés
- `src/live/server.rs` : `ws_handler` + `run_receiver`

### Tests
- Client pathologique qui spamme `Subscribe` → doit être rate-limited en
  < 1 s.

### Risque
Faible.

---

## Phase 4 — Headers de sécurité HTTP

### Problème
La réponse HTML rendue par Leptos n'a aucun `Content-Security-Policy`, pas
de `X-Content-Type-Options: nosniff`, pas de `Referrer-Policy`. Vecteurs XSS
potentiels ne sont pas mitigés (même si Leptos échappe automatiquement).

### Correction
Layer `tower_http::set_header::SetResponseHeaderLayer` sur le router leptos :
```rust
use tower_http::set_header::SetResponseHeaderLayer;
use http::{header, HeaderValue};

let security_headers = ServiceBuilder::new()
    .layer(SetResponseHeaderLayer::if_not_present(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    ));
```

**CSP** : plus délicat avec Leptos + hydrate (qui exige `script-src 'self'
'wasm-unsafe-eval'`). Préparer un template :
```
default-src 'self';
script-src 'self' 'wasm-unsafe-eval';
connect-src 'self' ws: wss:;
style-src 'self' 'unsafe-inline';
img-src 'self' data:;
font-src 'self' data:;
frame-ancestors 'none';
```
Tester en dev sans casser hydrate (`'unsafe-inline'` peut être requis au
début, resserrer ensuite).

### Fichiers touchés
- `Cargo.toml` : `tower-http = { version = "0.6", features = ["set-header"] }`
- `src/main.rs` : ajout des layers sur `leptos_router`

### Tests
- Manuel : DevTools → Network → headers de la réponse `/` contiennent les
  headers.
- Automatisé : `curl -I http://localhost:3000/` et assertion bash/Playwright.

### Risque
CSP peut casser le hydrate si mal configurée. Tester en dev avant merge.

---

## Phase 5 — `/healthz` et `/metrics`

### Problème
- Pas de `/healthz` : liveness probe kube n'a rien à tester que le port 3000
  accepte une connexion TCP.
- Pas de `/metrics` : observabilité nulle. On ne peut pas monitorer les
  ratios cache hit/miss, la taille du broadcast, le nombre de WS actifs.

### Correction

**Health** :
```rust
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    // Deep check : valider qu'au moins un RPC endpoint répond.
    // Light check : ok si le process est up.
    axum::http::StatusCode::OK
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    // Vérifier qu'au moins un RpcClient a un finalized_head valide.
    #[cfg(not(feature = "mock"))]
    {
        let any_ready = state.provider.as_rpc()
            .map(|p| p.clients.values().any(|c| c.finalized_head().is_some()))
            .unwrap_or(false);
        if !any_ready {
            return (StatusCode::SERVICE_UNAVAILABLE, "no ready backend").into_response();
        }
    }
    StatusCode::OK.into_response()
}
```

**Metrics** :
```toml
axum-prometheus = "0.7"
```
```rust
use axum_prometheus::PrometheusMetricLayer;

let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
let metrics_router = Router::new().route("/metrics", get(|| async move {
    metric_handle.render()
}));

let app = Router::new()
    .merge(ws_router)
    .merge(leptos_router)
    .merge(metrics_router)
    .merge(health_router)
    .layer(prometheus_layer);
```

Exposer en plus :
- `cache_hits_total{name=...}` / `cache_misses_total{name=...}`
  → nécessite instrumenter `src/data/rpc/cache.rs::cached`
- `ws_sessions_active` (gauge)
- `rpc_calls_total{method=...}` / `rpc_call_duration_seconds`

### Fichiers touchés
- `Cargo.toml` : `axum-prometheus`
- `src/main.rs` : routers health + metrics
- `src/server/mod.rs` : nouveau module `health` et `metrics`
- `src/data/rpc/cache.rs` : instrumenter `cached(...)` (hit/miss counters)

### Tests
- `curl http://localhost:3000/healthz` → 200
- `curl http://localhost:3000/metrics` → contient `process_resident_memory_bytes`
- Manuel : `/metrics | grep cache_misses`

### Risque
Faible. Attention à ne pas exposer `/metrics` publiquement — le reverse proxy
(nginx, ingress) doit whitelist par IP ou auth basic.

---

## Phase 6 — Guardrails de déploiement

### Problème
`src/main.rs` écoute en clair sur `site_addr = 127.0.0.1:3000`. Rien ne
documente qu'en prod il **faut** un reverse proxy TLS. Un opérateur peut
publier `ws://` sur Internet sans savoir.

### Correction
Purement documentaire :

1. Ajouter `docs/deployment.md` avec :
   - Exemple nginx : terminaison TLS + proxy vers 127.0.0.1:3000 + upgrade
     WS header + rate-limit `limit_req`.
   - Exemple Caddy (plus court).
   - Variables env obligatoires en prod (`WS_ALLOWED_ORIGINS`, endpoints RPC).
   - Recommandation : `systemd` unit avec `NoNewPrivileges`, `PrivateTmp`,
     `User=explorer`.
   - Recommandation Docker : image distroless, user non-root.

2. Ajouter un check au boot qui WARN si :
   - `site_addr` écoute sur `0.0.0.0` **et** pas de proxy détecté (pas de
     `X-Forwarded-For` sur les premières requêtes)
   - `WS_ALLOWED_ORIGINS` n'est pas set

### Fichiers touchés
- `docs/deployment.md` (nouveau)
- `README.md` : renvoyer vers `docs/deployment.md`
- `src/main.rs` : WARN conditionnel

### Risque
Nul.

---

## Phase 7 — Supply chain CI

### Problème
`Cargo.lock` check-in mais pas de CI qui catch :
- CVE dans une dep (`cargo audit`)
- Licence GPL accidentelle (`cargo-deny`)
- Yanked crates

### Correction
GitHub Actions workflow `.github/workflows/audit.yml` :
```yaml
name: audit
on:
  push: { branches: [main, master] }
  pull_request:
  schedule:
    - cron: '0 8 * * 1'  # chaque lundi 8h UTC
jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: taiki-e/install-action@v2
        with: { tool: 'cargo-audit,cargo-deny' }
      - run: cargo audit --deny warnings
      - run: cargo deny check
```

Fichier `deny.toml` minimal :
```toml
[licenses]
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause", "ISC", "Unicode-DFS-2016"]
confidence-threshold = 0.8

[bans]
multiple-versions = "warn"

[advisories]
vulnerability = "deny"
unmaintained = "warn"
```

### Fichiers touchés
- `.github/workflows/audit.yml` (nouveau)
- `deny.toml` (nouveau)

### Tests
- Intentionally add une crate vulnérable (en PR draft), vérifier que la CI
  crache l'audit.

### Risque
Faux positifs (crates marked "unmaintained"). Triages manuels au fil de
l'eau.

---

## Checklist d'exécution

Avant chaque phase :
- [ ] Vecteur d'attaque précisé + démo reproductible en dev
- [ ] Impact ops listé (config env, headers à ajouter à la doc)

Après chaque phase :
- [ ] Démo attaque rejouée → doit échouer
- [ ] Mise à jour `docs/deployment.md` si config ops change
- [ ] Vérifier que rien ne casse en dev (cargo leptos watch)
