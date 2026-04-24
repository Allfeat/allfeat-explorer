# Frontend v3 — Vue / Nuxt 4 rewrite plan

Status: planned. Branche cible : `feat/js-frontend-rewrite` (à créer depuis `master`).

Le chantier Leptos islands (`feat/frontend-islands`) est **abandonné** : la
complexité (WASM, thread-local, `SendWrapper`, `hydrate_islands`, 30+ islands)
dépassait le retour visuel. On bascule sur un **backend Rust stateless
(Axum REST + WebSocket)** consommé par un **frontend Nuxt 4 SSR** qui reprend
la maquette mise à jour (`~/Downloads/allfeat-chain-explorer/project/`).

## Décisions captées

| Sujet | Décision |
|---|---|
| Frontend framework | **Nuxt 4** (dernière stable) + Vue 3.5+ |
| Langage front | **TypeScript strict** (aucun `any`, `noImplicitAny`, `strictNullChecks`) |
| Toolchain nodejs | **Bun** (package manager + runtime Nitro) |
| Styling | **SCSS** via `sass-embedded` (Dart Sass moderne) — **pas de Tailwind** |
| State | **Pinia 3** pour stores globaux, **VueUse** pour composables utils |
| Theme | `prefers-color-scheme` — pas de toggle, pas de localStorage (décision reconduite de v2) |
| Rendering | **SSR** (Nitro) — hydration standard Vue, pas d'îlots |
| Backend | **Axum headless** — REST `/api/v1/*` + WS `/api/v1/live`. Plus aucune dépendance Leptos. |
| Protocole live | **Inchangé** : `src/live/protocol.rs` (ClientMsg/ServerMsg snake_case JSON). Le client TS consomme les mêmes frames. |
| Pont types | **`ts-rs`** (v10) derive sur `src/domain.rs` → génération TS dans `bindings/`. Exécutée via `cargo test --features ts-bindings`. |
| Monorepo layout | Root = Rust. `/web` = Nuxt. `/bindings` = types générés. Un `package.json` racine orchestre via bun scripts. |
| Dev runtime | Axum sur `:8088`, Nuxt sur `:3000`, Nitro proxy `/api/*` → `:8088`. |
| Prod runtime | Axum + Nuxt (Bun) derrière un reverse proxy (décision infra ultérieure). |
| Network selection | Query param `?network=<id>` (cohérent avec la baseline) |
| Page source de vérité | `~/Downloads/allfeat-chain-explorer/project/Allfeat Explorer.html` (maquette) + `styles.css` pour les tokens |

## Monorepo layout cible

```
explorer/
├── Cargo.toml                  # backend Rust (déleptosé)
├── Cargo.lock
├── src/                        # backend Rust (voir §"Backend")
├── artifacts/                  # métadonnées subxt — inchangé
├── migrations/                 # sqlx — inchangé
├── bindings/                   # types TS auto-générés par ts-rs (commité)
│   ├── Block.ts
│   ├── Extrinsic.ts
│   ├── Account.ts
│   ├── AtsRecord.ts
│   ├── AtsStats.ts
│   ├── ...
│   └── index.ts                # barrel re-export
├── web/                        # Nuxt 4 (voir §"Frontend")
│   ├── nuxt.config.ts
│   ├── package.json
│   ├── tsconfig.json
│   ├── bunfig.toml
│   ├── app/
│   │   ├── app.vue
│   │   ├── error.vue
│   │   ├── layouts/
│   │   ├── pages/
│   │   ├── components/
│   │   ├── composables/
│   │   ├── stores/
│   │   ├── types/
│   │   └── assets/styles/
│   └── public/
├── end2end/                    # Playwright — config et tests adaptés (voir §"Testing")
├── docs/
│   ├── frontend-v2-plan.md             # archive (Leptos islands, abandonné)
│   ├── js-frontend-rewrite-plan.md     # ce document
│   └── ...
├── package.json                # root : bun scripts qui orchestrent cargo + web
├── bunfig.toml
├── .gitignore                  # ajoute web/.nuxt, web/.output, web/node_modules
└── README.md                   # section "Development" réécrite
```

Pas de workspace Bun/npm : un seul package JS dans `/web`, le root
`package.json` sert uniquement à orchestrer.

## Backend — stripping Leptos

### Fichiers / modules **supprimés**

- `src/app/` entier (shell, app_root) — remplacé par Nuxt
- `src/pages/` entier — remplacé par `web/app/pages/`
- `src/ui/` entier — remplacé par `web/app/components/`
- `src/live/client.rs`, `src/live/connection.rs`, `src/live/store.rs` — remplacés par composables TS
- `src/format.rs` — formatage côté UI (porte en TS dans `web/app/utils/format.ts`)
- `src/lib.rs` : supprimer modules frontend + `hydrate()` wasm-bindgen

### Fichiers / modules **conservés tels quels**

- `src/domain.rs` — types de domaine (ajout dérive `TS` gated, voir §"Types")
- `src/data/**` — provider trait + mock + rpc
- `src/indexer/**`
- `src/live/protocol.rs` — messages WS (toujours utilisés côté serveur)
- `src/live/merge.rs`, `src/live/server.rs` — WS server handler (à re-router, voir §"API")
- `src/network.rs` — catalogue de chaînes
- `src/server/{config,db,health,metrics,state}.rs`
- `migrations/`, `artifacts/`

### Fichiers **réécrits**

- `src/server/fns/` → renommé `src/server/api/` : plus de `#[server]` macro, handlers Axum natifs avec `axum::extract::{Path, Query, State}` et `axum::Json`. Structure identique (`accounts.rs`, `ats.rs`, `blocks.rs`, `extrinsics.rs`, `networks.rs`).
- `src/server/api/mod.rs` : construit le `Router<AppState>` montant tous les sous-routeurs sous `/api/v1`.
- `src/main.rs` : garde la logique indexer/config/health/metrics, **supprime** tout ce qui touche Leptos (`generate_route_list`, `LeptosRoutes`, `shell`, `leptos_options`, CSP `wasm-unsafe-eval`, file_and_error_handler). Monte le nouveau `api_router` à la place du `leptos_router`.
- `src/lib.rs` : supprime `pub mod app`, `pub mod pages`, `pub mod ui`, `pub mod format`, le bloc `hydrate()`, la directive `recursion_limit`.

### Dépendances **retirées** de `Cargo.toml`

```
leptos, leptos_router, leptos_meta, leptos_axum
wasm-bindgen, js-sys, web-sys
console_error_panic_hook
send_wrapper
```

Feature `hydrate` supprimée en entier. Feature `ssr` devient la cible
principale (binaire). Feature `mock` préservée.

### Dépendances **ajoutées** côté Rust

```
ts-rs = { version = "10", optional = true, features = ["serde-compat", "chrono-impl"] }
```

Feature gate nouvelle :
```
ts-bindings = ["dep:ts-rs"]
```

Section `[package.metadata.leptos]` supprimée intégralement. `cargo leptos`
disparaît du workflow.

## API spec — REST + WebSocket

Toutes les routes sous `/api/v1`. JSON, Content-Type `application/json`,
UTF-8. Le `now_ms` de l'actuel `#[server]` **disparaît** de la surface
publique — le serveur utilise son propre wall clock.

### Networks

```
GET  /api/v1/networks
  → 200 { networks: [{ id, name, kind, testnet, token, block_time_secs, spec_version, endpoint, runtime_version }] }
```

Liste des réseaux activés (mock : tous ; prod : ceux avec `RPC_ENDPOINT_<ID>` set).

### Blocks

```
GET  /api/v1/networks/:network_id/blocks?count=25&from=<u64>
  → 200 Block[]

GET  /api/v1/networks/:network_id/blocks/head
  → 200 { number: u64 }

GET  /api/v1/networks/:network_id/blocks/:number
  → 200 Block | 404
```

### Extrinsics

```
GET  /api/v1/networks/:network_id/extrinsics?count=25
  → 200 Extrinsic[]

GET  /api/v1/networks/:network_id/extrinsics/:id
  → 200 Extrinsic | 404

GET  /api/v1/networks/:network_id/blocks/:number/extrinsics
  → 200 Extrinsic[]
```

### Transfers

```
GET  /api/v1/networks/:network_id/transfers?count=25
  → 200 Transfer[]
```

### Accounts

```
GET  /api/v1/networks/:network_id/accounts?count=20
  → 200 Account[]

GET  /api/v1/networks/:network_id/accounts/:address
  → 200 Account | 404

GET  /api/v1/networks/:network_id/accounts/:address/ats?limit=10
  → 200 AtsRecord[]

GET  /api/v1/networks/:network_id/accounts/:address/ats/count
  → 200 { count: u32 }
```

### ATS

```
GET  /api/v1/networks/:network_id/ats?count=25&from_index=<u32>
  → 200 AtsRecord[]

GET  /api/v1/networks/:network_id/ats/:index
  → 200 AtsRecord | 404

GET  /api/v1/networks/:network_id/ats/feed?count=25&from_index=<u32>
  → 200 AtsFeedItem[]

GET  /api/v1/networks/:network_id/ats/stats
  → 200 AtsStats
```

### Live

```
GET  /api/v1/live
  → 101 WebSocket upgrade
```

Protocole **inchangé** (`src/live/protocol.rs`). Le client émet
`{"type":"subscribe","topic":"blocks"}` etc., le serveur push `{"type":"block","data":{...}}`.

Le `network_id` est déterminé par la query string de l'URL de subscription :
`ws://.../api/v1/live?network=melodie` (pattern déjà en place dans `src/live/server.rs`).

### Health / metrics

```
GET  /api/v1/healthz
GET  /api/v1/readyz
GET  /api/v1/metrics        # Prometheus scrape
```

### Error shape

Réponse d'erreur uniformisée (sérialise `DataError` mappé sur status HTTP) :

```json
HTTP 404 { "error": { "code": "not_found", "message": "block 12345 not found" } }
HTTP 500 { "error": { "code": "internal", "message": "..." } }
HTTP 400 { "error": { "code": "bad_request", "message": "count must be >= 1" } }
```

Defined via `impl IntoResponse for ApiError` dans `src/server/api/error.rs`.

### CORS & sécurité

- CORS : `tower-http::cors::CorsLayer`. En dev, `Any`. En prod, allowlist via env `API_ALLOWED_ORIGINS`.
- Rate limit : le governor `/ws` actuel reste ; étendre à `/api/v1/*` avec un budget plus large (ex. 100 req/s burst 200 par IP).
- Security headers : garder ceux déjà posés par `main.rs` (`X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`). Le CSP actuel est taillé pour Leptos (`wasm-unsafe-eval`) → simplifier / supprimer (l'API ne sert plus de HTML).

## Type bridge — ts-rs

### Rust

1. Ajouter `ts-rs` dans `[dependencies]` avec feature gate `ts-bindings`.
2. Sur chaque type de `src/domain.rs` :
   ```rust
   #[cfg_attr(feature = "ts-bindings", derive(ts_rs::TS))]
   #[cfg_attr(feature = "ts-bindings", ts(export, export_to = "../bindings/"))]
   #[derive(Clone, Debug, ..., Serialize, Deserialize)]
   pub struct Block { ... }
   ```
3. Ajouter un test dédié qui déclenche l'export :
   ```rust
   // tests/ts_export.rs
   #[cfg(feature = "ts-bindings")]
   #[test]
   fn export_bindings() {
       // ts-rs exporte automatiquement à l'exécution des tests
   }
   ```
4. Script dans root `package.json` :
   ```json
   "gen:bindings": "cargo test --features ts-bindings --quiet ts_export"
   ```

### Sortie attendue dans `bindings/`

Un fichier par type : `Block.ts`, `Extrinsic.ts`, `Account.ts`, `Balance.ts`,
`CallResult.ts`, `EventField.ts`, `EventRef.ts`, `ExtrinsicArgs.ts`,
`Transfer.ts`, `AtsRecord.ts`, `AtsVersion.ts`, `Deposit.ts`, `AtsFeedItem.ts`,
`AtsStats.ts`, plus un `index.ts` barrel. Les `u64` / `u128` mappent sur
`string` (ts-rs default pour éviter la perte de précision JS).

### Côté TS

`web/app/types/api.ts` réexporte depuis `../../../bindings/index.ts` :

```ts
export type { Block, Extrinsic, Account, ... } from '@bindings'
```

Alias de path configuré dans `web/tsconfig.json` :
```json
"paths": { "@bindings": ["../bindings/index.ts"] }
```

## Frontend — stack Nuxt 4

### Packages (dernières versions, à verrouiller au scaffold)

```
nuxt            ^4
vue             ^3.5
pinia           ^3
@pinia/nuxt
@vueuse/nuxt    (dernière compatible Vue 3.5)
@vueuse/core
sass-embedded   (peer de nuxt pour .scss)
typescript      ^5.x (strict)
```

Dev deps :
```
@playwright/test
eslint @antfu/eslint-config
```

Pas de Tailwind, pas de UI kit (Naive, Element, ...). Tout composant est
écrit maison pour matcher la maquette au pixel.

### `nuxt.config.ts` — structure

```ts
export default defineNuxtConfig({
  srcDir: 'app/',
  ssr: true,
  typescript: { strict: true, typeCheck: true },

  modules: ['@pinia/nuxt', '@vueuse/nuxt'],

  css: ['~/assets/styles/main.scss'],

  vite: {
    css: {
      preprocessorOptions: {
        scss: {
          api: 'modern-compiler',
          additionalData: '@use "~/assets/styles/tokens" as *;',
        },
      },
    },
  },

  nitro: {
    devProxy: {
      '/api': { target: 'http://127.0.0.1:8088', changeOrigin: true, ws: true },
    },
  },

  runtimeConfig: {
    public: {
      apiBase: '/api/v1',
    },
  },

  app: {
    head: {
      htmlAttrs: { lang: 'en' },
      title: 'Allfeat Explorer',
      meta: [{ name: 'viewport', content: 'width=device-width,initial-scale=1' }],
      link: [
        { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' },
        { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap' },
      ],
    },
  },
})
```

### Arborescence `web/app/`

```
app/
├── app.vue                       # <NuxtLayout><NuxtPage/></NuxtLayout>
├── error.vue                     # 500 / 404 fallback
├── layouts/
│   └── default.vue               # <Header/> <slot/> <Footer/>
├── pages/
│   ├── index.vue                 # Dashboard
│   ├── blocks/
│   │   ├── index.vue             # Blocks list
│   │   └── [number].vue          # Block detail
│   ├── extrinsics/
│   │   ├── index.vue
│   │   └── [id].vue
│   ├── accounts/
│   │   ├── index.vue
│   │   └── [address].vue
│   └── ats/
│       ├── index.vue
│       └── [index].vue
├── components/
│   ├── layout/
│   │   ├── AppHeader.vue
│   │   ├── AppFooter.vue
│   │   ├── Brand.vue
│   │   ├── NetworkSwitch.vue
│   │   └── Breadcrumb.vue
│   ├── ui/
│   │   ├── Hash.vue              # hash ellipsé + tooltip
│   │   ├── Addr.vue              # address ss58 ellipsé
│   │   ├── CopyButton.vue
│   │   ├── Identicon.vue         # conic-gradient, calculé via hash SS58
│   │   ├── StatusPill.vue
│   │   ├── Pill.vue
│   │   ├── Chip.vue
│   │   ├── Kv.vue
│   │   ├── Pagination.vue
│   │   ├── FilterSegment.vue
│   │   ├── Tabs.vue
│   │   ├── TimeAgo.vue
│   │   ├── SkeletonRows.vue
│   │   ├── Spark.vue             # sparkline inline SVG
│   │   └── Icons/                # <IconBlock/>, <IconExtrinsic/>, ...
│   ├── live/
│   │   ├── ConnectionIndicator.vue
│   │   ├── FooterHeadChip.vue
│   │   ├── IndexingBanner.vue
│   │   └── LiveDot.vue
│   ├── hero/
│   │   ├── Hero.vue
│   │   ├── HeroNetworkLine.vue
│   │   ├── HeroLiveCard.vue
│   │   └── HeroCountdown.vue
│   ├── tables/
│   │   ├── BlocksTable.vue
│   │   ├── ExtrinsicsTable.vue
│   │   ├── AccountsTable.vue
│   │   ├── TransfersTable.vue
│   │   └── AtsTimeline.vue
│   └── detail/
│       ├── BlockDetail.vue
│       ├── ExtrinsicDetail.vue
│       ├── AccountDetail.vue
│       └── AtsDetail.vue
├── composables/
│   ├── useApi.ts                 # wrapper $fetch avec apiBase + typage
│   ├── useActiveNetwork.ts       # lit ?network= et résout via /networks
│   ├── useLiveSocket.ts          # singleton WebSocket + reconnect
│   ├── useLiveBlocks.ts          # reactive buffer<Block> (seed + live)
│   ├── useLiveTransfers.ts
│   ├── useLiveAtsFeed.ts
│   ├── useConnectionState.ts
│   └── useWallClock.ts           # tick 1s pour TimeAgo / countdown
├── stores/
│   ├── networks.ts               # enabled networks (loaded once SSR)
│   ├── live.ts                   # buffers + connection state (global, client-only)
│   └── indexing.ts               # status banner
├── types/
│   ├── api.ts                    # re-export des bindings/
│   └── live.ts                   # ClientMsg / ServerMsg côté TS
├── utils/
│   ├── format.ts                 # fmtAft, fmtInt, shortHash, shortAddr (portés de src/format.rs)
│   ├── identicon.ts              # calcul du conic-gradient depuis ss58
│   └── time.ts                   # timeAgo(ts, now)
└── assets/
    └── styles/
        ├── main.scss             # entry: @use tokens, @use reset, @use layout, @use components
        ├── _tokens.scss          # palette + typo + espacements (cf. §"Design system")
        ├── _reset.scss
        ├── _layout.scss          # container, grid helpers, spacing
        ├── _typography.scss
        └── components/
            ├── _header.scss
            ├── _footer.scss
            ├── _panel.scss
            ├── _table.scss
            ├── _pill.scss
            ├── _chip.scss
            ├── _hash.scss
            ├── _kv.scss
            ├── _stat.scss
            ├── _tabs.scss
            ├── _btn.scss
            ├── _seg.scss
            ├── _breadcrumb.scss
            ├── _pagination.scss
            ├── _identicon.scss
            ├── _connection-pill.scss
            ├── _skeleton.scss
            └── _hero.scss
```

## Design system — port SCSS

Source unique : `~/Downloads/allfeat-chain-explorer/project/styles.css` +
le `<style>` inline dans `Allfeat Explorer.html`.

### Tokens (`_tokens.scss`)

Portage direct des `:root` du prototype :

- **Palette brand** : `--cream-50/100/200/300`, `--ink-900/800/700/600/500/400/300`, `--red-500/600/700/800`, `--teal-500/600/700/800`.
- **Palette dark (défaut)** : `--bg`, `--bg-1`, `--bg-1-solid`, `--bg-2`, `--bg-elev`, `--bg-elev-solid`, `--line`, `--line-2`, `--ink`, `--ink-dim`, `--ink-dimmer`, `--ink-muted`, `--accent-pos`, `--accent-neg`, `--chip-bg`, `--chip-bd`, `--hover`, `--link`, `--link-hover`, `--grid-dot`, `--shadow`, `--shadow-lift`, `--glow-teal`, `--glow-red`.
- **Palette light** : même liste, surcharge sous `@media (prefers-color-scheme: light) { :root { ... } }`.
- **Typographie** : `--font-display: 'TASA Orbiter Display', 'Space Grotesk', ...`, `--font-text`, `--font-mono: 'JetBrains Mono', ...`. `@font-face` fallback local sur Space Grotesk (comme le proto).
- **Layout** : `--container-max: 1440px`, paddings responsive (56px / 40px / 24px).

### Composants portés verbatim (classes + comportement)

Depuis le proto (ordre d'apparition dans `styles.css`) :

- `.container` responsive
- `.header`, `.header-inner`, `.brand`, `.brand-word`, `.brand-suffix`, `.nav`, `.network-switch`, `.network-dot`, `.icon-btn`, `.price-chip`
- `.searchbar-wrap`, `.searchbar`, `.search-kind`, `.search-input`, `.search-kbd`
- `.panel`, `.panel-head`, `.panel-body`
- `.table`, `.table.compact`
- `.pill.success|fail|pending|neutral`
- `.chip`, `.chip.module`, `.chip.call`
- `.hash`, `.hash-dim`, `.copy-btn`
- `.kv`
- `.stat` (hero cards)
- `.tabs`, `.tab`, `.tab .count`
- `.btn`, `.btn.primary`, `.btn.ghost`, `.btn.sm`, `.icon-square`
- `.dotgrid`, `.bar`, `.seg`
- `.identicon`, `.identicon-lg`
- `.accent-line` (bannière tricolore rouge/cream/teal)
- `.footer`, `.footer-inner`, `.footer-col`
- `.page-title`, `.breadcrumb`
- `.live-dot` (+ `@keyframes pulseDot`)

Les classes restent **nominales** — on ne renomme pas. Chaque composant
Vue attache les classes du proto pour que les règles CSS s'y appliquent
sans mapping ad-hoc.

### Additions non-présentes dans `styles.css`

À porter depuis le `<style>` inline du HTML :
- `.nav-more-menu` (dropdown nav responsive)
- `.connection-pill`, `.connection-pill--{connecting,connected,reconnecting,offline}`
- `.row-fade-in` (+ `@keyframes row-highlight-fade` pour le flash bleu)
- `.skeleton-row`, `.skeleton-shimmer` (+ `@keyframes skeleton-shimmer`)

## Page inventory — maquette → Nuxt

Mapping des pages repérées dans `Allfeat Explorer.html` (via `data.js` + la
hiérarchie de l'ancien `src/pages/` baseline) :

| Maquette (HTML) | Nuxt route | Source de données |
|---|---|---|
| Dashboard (hero + latest blocks + latest extrinsics + ats widget) | `/` | `GET /blocks`, `/extrinsics`, `/ats/feed`, `/ats/stats`, `/blocks/head` |
| Blocks list | `/blocks` | `GET /blocks?count=25&from=...` + WS `topic:blocks` |
| Block detail (KV + tabs extrinsics/events/logs) | `/blocks/:number` | `GET /blocks/:number` + `/blocks/:number/extrinsics` |
| Extrinsics list | `/extrinsics` | `GET /extrinsics?count=25` |
| Extrinsic detail | `/extrinsics/:id` | `GET /extrinsics/:id` |
| Accounts top list | `/accounts` | `GET /accounts?count=20` |
| Account detail (balance card + tabs extrinsics/transfers/ats) | `/accounts/:address` | `GET /accounts/:address` + `/accounts/:address/ats` |
| ATS timeline + stats | `/ats` | `GET /ats/feed` + `/ats/stats` |
| ATS detail (overview + technical + version history) | `/ats/:index` | `GET /ats/:index` |
| 404 | auto (Nuxt `error.vue`) | — |

## Live layer — architecture côté client

### `useLiveSocket.ts` (composable singleton)

- Une seule `WebSocket` par onglet, attachée à `?network=<id>` courant.
- Gère reconnect exponentiel (250ms → 5s cap), état `ConnState` dans un
  `ref` partagé.
- Sur changement d'`activeNetwork` → close + reopen sur la nouvelle URL,
  purge des buffers.
- Envoie `{type:'subscribe',topic:'blocks'}` + transfers + ats_feed au
  `onopen`.
- Route les frames entrantes vers les stores Pinia (`live.pushBlock`, etc.).
- Dispose : `onBeforeUnmount` sur la première utilisation racine ; hors de
  ça le socket vit pour la session.

### `stores/live.ts` (Pinia)

```ts
interface LiveStore {
  blocks: Block[]            // newest-first, cap 25
  transfers: Transfer[]      // newest-first, cap 25
  atsFeed: AtsFeedItem[]     // newest-first, cap 25
  connection: ConnState
  seedBlocks(initial: Block[]): void
  pushBlock(b: Block): void
  clear(): void              // sur switch réseau
  ...
}
```

Dedup sur `block.number`, `transfer.extrinsic.id`, `atsFeed.(ats_id, version_index)`.
Cap configurable par store (défaut 25).

### `useLiveBlocks()` — API publique

```ts
const { blocks, loading, error } = useLiveBlocks({
  initialCount: 25,   // seed via useFetch
})
```

Fait un `useFetch` initial pour SSR (HTML rendu avec données réelles), puis
monte l'abonnement WS au `onMounted` client-only. Fusionne seed + live en
une seule ref triée/dédupée.

### Composables similaires

- `useLiveTransfers()`, `useLiveAtsFeed()` : même pattern
- `useConnectionState()` : lit `stores.live.connection`
- `useWallClock()` : `ref<number>` tick 1s (client-only), pour `TimeAgo`, countdown hero
- `useActiveNetwork()` : lit `?network=` + cross-check `stores.networks.enabled[]`

## Phases d'exécution

### Phase 0 — Bascule propre (~1h)

- [ ] `git stash push -u -m "leptos-islands-wip"` sur `feat/frontend-islands`
- [ ] `git checkout master && git checkout -b feat/js-frontend-rewrite`
- [ ] Commit initial : ce plan (`docs/js-frontend-rewrite-plan.md`)
- [ ] Setup root `package.json` + `bunfig.toml` + `.gitignore` (exclure `web/.nuxt`, `web/.output`, `web/node_modules`, `bindings/*.ts`… en fait on commit les bindings, à confirmer)

### Phase 1 — Backend : strip Leptos (~1 j)

- [ ] Supprimer modules frontend (`src/app/`, `src/pages/`, `src/ui/`, `src/format.rs`, `src/live/{client,connection,store}.rs`)
- [ ] Patch `src/lib.rs` : retirer `pub mod app/pages/ui/format`, le bloc `hydrate()`, `recursion_limit`
- [ ] Retirer deps Leptos de `Cargo.toml` + section `[package.metadata.leptos]`
- [ ] Patch `src/main.rs` : retirer `leptos_axum`, `generate_route_list`, `LeptosRoutes`, `shell`, CSP Leptos. Le binaire doit déjà démarrer avec `/healthz`, `/readyz`, `/metrics`, `/ws`.
- [ ] Validation : `cargo build --features ssr,mock --bin allfeat-explorer` → OK. `curl http://127.0.0.1:3000/healthz` (ou port final) → 200.

### Phase 2 — API REST (~1,5 j)

- [ ] Créer `src/server/api/` avec `mod.rs`, `error.rs`, `accounts.rs`, `ats.rs`, `blocks.rs`, `extrinsics.rs`, `networks.rs`, `health.rs`.
- [ ] `ApiError` dans `error.rs` : variantes `NotFound`, `BadRequest`, `Internal`. `impl IntoResponse`. `From<DataError>` → mapping status.
- [ ] Chaque handler : `async fn list_blocks(State(state), Path(network_id), Query(params)) -> Result<Json<Vec<Block>>, ApiError>`.
- [ ] `api::router()` → `Router<AppState>` monté sur `/api/v1`.
- [ ] WS handler migré de `/ws` vers `/api/v1/live` (renommer route uniquement ; handler interne inchangé).
- [ ] CORS dev-only via `CorsLayer::permissive()`, prod via env `API_ALLOWED_ORIGINS`.
- [ ] Validation : `curl http://127.0.0.1:8088/api/v1/networks` → JSON ; `wscat` sur `/api/v1/live?network=melodie` → ping/subscribe.
- [ ] Port backend passe de `:3000` à `:8088` (libère `:3000` pour Nuxt).

### Phase 3 — ts-rs bindings (~0,5 j)

- [ ] Ajouter `ts-rs` + feature `ts-bindings` à `Cargo.toml`.
- [ ] Annoter chaque type de `src/domain.rs` avec `#[cfg_attr(feature = "ts-bindings", derive(TS), ts(export, export_to = "../bindings/"))]`.
- [ ] `tests/ts_export.rs` qui importe les types pour forcer la génération.
- [ ] `mkdir bindings && touch bindings/index.ts` (barrel, écrit à la main ou via post-script).
- [ ] Script root : `bun run gen:bindings` → exécute `cargo test --features ts-bindings --quiet ts_export`.
- [ ] Validation : `bindings/Block.ts` existe et contient la bonne shape (u64/u128 → string). Commit les bindings dans le repo.

### Phase 4 — Scaffold Nuxt (~0,5 j)

- [x] Scaffold `web/` à la main (équivalent de `bunx nuxi init`) : `package.json`, `nuxt.config.ts`, `tsconfig.json`, `bunfig.toml`, `app/{app,error}.vue`, `app/layouts/default.vue`, `app/pages/index.vue`, `app/assets/styles/{main,_tokens}.scss`.
- [x] Deps déclarées dans `web/package.json` : nuxt ^4, vue ^3.5, pinia ^3, @pinia/nuxt, @vueuse/{nuxt,core}, sass-embedded, typescript, vue-tsc.
- [x] `nuxt.config.ts` : srcDir `app/`, SSR, TS strict + typeCheck, modules Pinia+VueUse, CSS entry, SCSS `modern-compiler` + additionalData tokens (guard anti-self-reference), devProxy `/api → :8088`, head fonts.
- [x] Alias `@bindings` → `../bindings/index.ts` via `alias` dans `nuxt.config.ts` (vite + tsconfig généré).
- [x] `flake.nix` : ajout de `bun`, retrait des deps Leptos (cargo-leptos, wasm-bindgen-cli, leptosfmt, binaryen, dart-sass, pnpm).
- [x] Validation HTTP proxy : `bun run dev:api` (backend `:8088`) + `bun run dev:web` (Nuxt `:3000`), `curl http://127.0.0.1:3000/api/v1/networks` renvoie la liste des réseaux mockés. `vue-tsc` 0 erreurs.
- [x] Limitation captée : Nuxt 4 / Nitro 2.13 n'exposent pas le `upgrade` event aux middlewares de proxy en dev — `nitro.devProxy` et `vite.server.proxy` ne forwardent pas les WebSockets. Solution Phase 8 : `runtimeConfig.public.wsBase` pointe directement vers `ws://127.0.0.1:8088/api/v1` en dev (déjà déclaré dans `nuxt.config.ts`). En prod, le reverse proxy traite HTTP + WS uniformément (override via `NUXT_PUBLIC_WS_BASE`).

### Phase 5 — Design system SCSS (~1 j)

- [x] Porter `_tokens.scss` intégralement (palette + type + theme dark/light) depuis le `<style>` inline du HTML maquette (source la plus récente ; `styles.css` est un snapshot antérieur). `:root` + `[data-theme="dark"]` + `@media (prefers-color-scheme: light)` + override explicite `[data-theme="light"]`.
- [x] Porter `_reset.scss` (box-sizing, `html/body`, `a`, `button`, scrollbar), `_typography.scss` (`@font-face` fallback Space Grotesk, h1-h4, `.mono`, `.ui-label`), `_layout.scss` (`.container` + breakpoints 1200/768, `.grid/.row/.gap-*`, `.dotgrid`, `.divider-*`, utility classes, keyframes `fadeUp/fadeIn` + `.reveal.d-{1..6}`).
- [x] Porter tous les `components/*.scss` **verbatim** (classes identiques à la maquette) : `_header.scss` (brand + nav + nav-more-menu responsive + network-switch + price-chip + searchbar), `_footer.scss` (+ `.accent-line` en `display:none` comme la maquette), `_panel.scss` (base + `.interactive` + `.hero-card`), `_table.scss` (row hover accent via box-shadow inset), `_pill.scss`, `_chip.scss`, `_hash.scss` + `.copy-btn`, `_kv.scss`, `_stat.scss`, `_tabs.scss`, `_btn.scss` + `.icon-square`, `_seg.scss`, `_breadcrumb.scss`, `_identicon.scss`, `_hero.scss` (page-title + waveform-hero + bar + live-dot + live-num + ticker + spark + keyframes pulseDot/caretBlink/tick/barFill/shimmer).
- [x] Nouveaux partials non-présents dans la maquette, créés dans le même langage visuel : `_pagination.scss`, `_connection-pill.scss` (4 états `connecting/connected/reconnecting/offline` avec keyframes dédiées), `_skeleton.scss` (`.skeleton-row`, `.skeleton-shimmer`, `.row-fade-in` avec flash teal).
- [x] `css: ['~/assets/styles/main.scss']` déjà en place. **Révision** vs le plan initial : le bloc Vite `additionalData` qui injectait `@use tokens as *;` dans chaque partial a été supprimé. Raison : `_tokens.scss` ne contient que des CSS custom properties (pas de vars/mixins SCSS), donc `as *` n'expose rien d'utile, et surtout l'injection dupliquait le bloc `:root {}` dans chaque unité de compilation `<style scoped>` (Vue réécrit le sélecteur avec `[data-v-hash]` → déclarations inopérantes, poids inutile). Les composants lisent les tokens via `var(--X)` directement. Si un besoin futur de symbols SCSS partagés apparaît (breakpoints-as-mixins etc.), on créera un `_vars.scss` dédié à injecter.
- [x] `main.scss` orchestre l'ordre via `@use` : tokens → reset → typography → layout → tous les components. Chaque partial sort donc exactement une fois dans le bundle global.
- [x] Validation : `pages/index.vue` rend `.container > .panel > .panel-head (h3 + .tag + .pill.success) + .panel-body > .kv (.k/.v mono)`. Dev server lancé via `nix develop --command bun run dev` → CSS compilé inclut toutes les classes attendues (header-inner, brand-word, nav-more-menu, waveform-hero, connection-pill, skeleton-row, row-fade-in, live-dot, page-title, pagination, etc.). `bun run typecheck` → 0 erreur.
- [x] `app/layouts/default.vue` remanié pour exposer `<header class="header"><div class="container header-inner">...</div></header>` + footer équivalent, afin que les tokens soient visibles pendant les phases 6-8 (avant le remplacement complet par `<AppHeader/>` en Phase 7).

### Phase 6 — Primitives UI (~1,5 j)

Ordre de portage (du plus simple au plus composé) :

1. [x] `Hash.vue`, `Addr.vue`, `CopyButton.vue` — Hash/Addr gèrent head/tail/copy/dim/link optionnel ; Addr embarque un `<Identicon/>` par défaut ; CopyButton utilise `navigator.clipboard` + flash check ~900ms, try/catch silencieux si perms refusées.
2. [x] `Identicon.vue` + `utils/identicon.ts` (conic-gradient déterministe via `hashAngle`). Props `size`/`large`.
3. [x] `Pill.vue` (`success/fail/pending/neutral` + dot), `Chip.vue` (`default/module/call`), `StatusPill.vue` (mapping `result` + `finalized` → variant + label, logique ported verbatim), `Kv.vue` + helper `KvRow.vue` multi-root pour simplifier l'API côté pages (`<KvRow label="…">content</KvRow>`).
4. [x] `Tabs.vue` URL-driven via `?tab=<id>` (query key configurable), `navigateTo({replace:true})` pour éviter la pollution d'historique. `activeId` tombe sur le premier item si query absent/inconnu. Émet `@change`. `count` optionnel par item.
5. [x] `FilterSegment.vue` même pattern que Tabs (`?filter=`) mais rendu avec la classe `.seg`.
6. [x] `Pagination.vue` URL-driven via `?page=`, `navigateTo` en mode push (back button rewind les pages). `utils/pagination.ts` calcule les slots (numéros + `'ellipsis'`) : short-circuit ≤7 pages, sinon `1 … [left..right] … total` avec `siblings=1`.
7. [x] `TimeAgo.vue` + `composables/useWallClock.ts` (singleton 1-Hz, `useState('wall-clock')` SSR-safe + `setInterval` unique démarré au premier appel client) + `utils/time.ts` (`timeAgo(ts, now)` pur, porté du maquette).
8. [x] `SkeletonRows.vue` — prop `rows` (nombre) + `columns` (nombre → `repeat(N, 1fr)`, ou liste de tracks CSS pour matcher une vraie table).
9. [x] `Breadcrumb.vue` (dans `components/layout/`, pas dans `ui/`) — items `{label, to?}`, séparateur `.sep` inséré entre items.

Décisions captées au portage :
- `components: [{ path: '~/components', pathPrefix: false }]` ajouté à `nuxt.config.ts` : `ui/Hash.vue` s'auto-importe `<Hash/>` (pas `<UiHash/>`), cohérent avec les noms de la maquette.
- `useWallClock` garde son `setInterval` pour la session client entière : cycler mount/unmount coûterait plus cher sur les pages data-heavy où plusieurs `<TimeAgo/>` vivent simultanément.
- SSR-safety wall clock : `useState` scope-la par requête ; l'intervalle est tracké via un module-level `clientInterval` (OK côté client, jamais atteint côté server).
- `CopyButton` stop-propagate son click : fonctionne dans des rows cliquables sans déclencher la navigation de la row.
- Smoke test : `pages/index.vue` expose un panneau « Primitives probe » qui instancie chaque primitive une fois ; remplacé par le dashboard en Phase 9. Validation : `bun run typecheck` → 0 erreur ; `curl http://127.0.0.1:3000/` rend verbatim les classes `.pill.{success,fail,pending,neutral}`, `.chip.{module,call}`, `.hash`/`.hash-dim`, `.identicon`/`.identicon-lg`, `.tabs`/`.tab.active`, `.seg` (première option active), `.pagination` + `.current` + `aria-current="page"`, `.skeleton-row` × 3, `.breadcrumb`, `.copy-btn` ; `<TimeAgo/>` rend « N sec/min/hr ago ».

### Phase 7 — Layout : header + footer + network switch (~1 j)

- [x] `AppHeader.vue` — brand (NuxtLink → `/`), `.nav` verbatim (Overview/ATS/Blocks/Extrinsics/Accounts/Events/Runtime) avec icônes portées du proto (`NavIcon.vue`), dropdown More géré par CSS (`data-collapse-at` + `data-show-at` à xl/lg/xxl — pas de mesure JS), header-spacer, searchbar en dessous avec placeholder rotatif (SEARCH_HINTS, cycle 2.6s, client-only).
- [x] `NetworkSwitch.vue` — Pinia store `stores/networks.ts` seedé par `plugins/networks.server.ts` sur SSR, consommé via `composables/useActiveNetwork.ts` (lit `?network=` + fallback `defaultId`). Trigger `.network-switch` (dot + name + kind) + menu déroulant scoped (hover, active, check). Click item → `navigateTo({ path, query: {...route.query, network: id}, replace: true })` pour ne pas polluer l'historique et préserver `?tab`/`?filter`/`?page` existants. `onClickOutside` via `@vueuse/core`.
- [x] `AppFooter.vue` — `.accent-line` (caché par CSS comme la maquette), `.footer-inner` 4 colonnes (brand / Networks dynamiques via store / Developers / Resources), bande basse `.footer-bottom` qui embarque `<ConnectionIndicator state="connecting"/>` + `<FooterHeadChip :head="null" :finalized="null"/>` — les deux sont purement présentationnels, le wiring live arrive en Phase 8.
- [x] `components/live/` bootstrapped : `LiveDot.vue` (dot pulsing piloté par `currentColor`), `ConnectionIndicator.vue` (`.connection-pill.connection-pill--{connecting,connected,reconnecting,offline}`), `FooterHeadChip.vue` (BLOCK / FINALIZED mono, séparateur nbsp fin).
- [x] `IndexingBanner.vue` — seed via `useFetch('/indexing/status')` (baseURL = `apiBaseUrl()`), re-poll 5s client-only via `@vueuse/core::useIntervalFn`. Masqué quand l'état de l'active network est `Healthy` ; variantes info/warn/error mappées sur `Backfilling/CatchingUp/Offline`. En mock mode le backend renvoie `[]` → banner jamais visible, cohérent.
- [x] `Breadcrumb.vue` déjà en place (Phase 6).
- [x] Swap `layouts/default.vue` : `<AppHeader/> <IndexingBanner/> <main><slot/></main> <AppFooter/>`. Les stubs inline du Phase 5 sont supprimés.
- [x] `pages/index.vue` ramené à un placeholder Phase 7 (breadcrumb + panel "Layout shell"). Le probe complet Phase 6 sera remplacé par le vrai dashboard en Phase 9.
- [x] Validation : `bun run typecheck` → 0 erreur. `bun run dev:api` (port 8088) + `bun run dev:web` (port 3000) démarrent proprement. `curl http://127.0.0.1:3000/` → 200, 13k HTML, contient `.header`, `.nav`, `.nav-more-wrap`, `.network-switch` (name "Allfeat" / dot mainnet par défaut), `.searchbar`, `.footer-inner`, `.footer-bottom`. `curl http://127.0.0.1:3000/?network=melodie` → 200, network-name = "Melodie" + dot `.testnet`. `curl /api/v1/indexing/status` (via proxy Vite) → `[]`. Seuls warnings Vue Router pour `/ats`, `/blocks`, `/extrinsics`, `/accounts` (routes Phase 9 pas encore matérialisées — NuxtLinks dans le nav).

Décisions captées au portage :
- **SSR `$fetch` et le proxy Vite** : première tentative utilisait `$fetch('/api/v1/networks')` dans le plugin server ; Nitro dispatchait la requête en interne (pas de passage par le proxy Vite → pas d'arrivée sur le backend `:8088`), Vue Router répondait "No match found" et ré-rendait la page, re-déclenchant le plugin → boucle infinie. Solution : `runtimeConfig.apiOrigin = 'http://127.0.0.1:8088'` (server-only, overridable via `NUXT_API_ORIGIN`) + `composables/useApi.ts` qui calcule `baseURL = server ? apiOrigin+apiBase : apiBase`. `apiFetch` (wrapper `$fetch`) + `apiBaseUrl()` (helper pour `useFetch`). Le client continue d'utiliser le chemin relatif `/api/v1/*` proxied par Vite.
- **Forme de la réponse `/networks`** : le backend renvoie `{networks: NetworkSpec[]}` (pas un tableau nu), cohérent avec le plan — le plugin unwrap explicitement via un type dédié `NetworksResponse`. Les autres endpoints (blocks/extrinsics/accounts/ats) renvoient des tableaux bruts, à traiter au cas par cas en Phase 9.
- **Nav responsive, pas de mesure JS** : le split primary ↔ More est purement CSS (`data-collapse-at="xl|lg|xxl"` sur les items primary, `data-show-at="xl|lg|xxl"` sur les items More, media-queries inversées à 1280/1100px). Les deux listes sont rendues simultanément, le CSS décide qui s'affiche. Même design que la maquette React d'origine, beaucoup plus simple qu'un ResizeObserver.
- **Search submit neutralisé** : le formulaire appelle `e.preventDefault()` en Phase 7 — pas de router logic (block/extrinsic/address detection) tant que les pages cibles n'existent pas. Sera complété quand les routes atterrissent (Phase 9).
- **`ConnectionIndicator` et `FooterHeadChip` purement présentationnels** : props seulement, pas de `useConnectionState()` / `useLiveBlocks()` en Phase 7. La valeur par défaut (`connecting` / `null`) produit un rendu SSR honnête qui ne promet pas de live feed. Phase 8 remplace ces props par les refs du store `live` sans toucher aux composants.
- **Banner en `useFetch` + `useIntervalFn`** : `useFetch` pour seed SSR, `useIntervalFn(refresh, 5_000)` côté client uniquement. Sur mock, le backend renvoie `[]` → `status === null` → `visible === false` → aucun markup émis, le banner n'existe même pas dans le DOM.
- **Pages `/ats`, `/blocks`, `/extrinsics`, `/accounts` absentes** : les NuxtLinks du header génèrent des warnings "No match found" en dev. Tolérable — ils disparaîtront naturellement quand les routes Phase 9 existeront. Pas de stub intermédiaire.

### Phase 8 — Live layer (~1 j)

- [x] `types/live.ts` — miroir TS de `src/live/protocol.rs` (`Topic`/`ClientMsg`/`ServerMsg`/`ConnState` + constante `ALL_TOPICS`). Non émis par ts-rs car ce sont des types de wire, pas de domaine.
- [x] `stores/live.ts` Pinia : buffers `blocks`/`transfers`/`atsFeed` newest-first cap 25, state `connection`, getters `head`/`finalizedHead`, actions `seed*`/`push*`/`clearLive`/`setConnection`. Dedup par numéro (re-org aware pour blocks, clé `extrinsic.id` pour transfers, `ats_id:version_index` pour atsFeed). Comparaison de block numbers via `BigInt` (u64 safe).
- [x] `composables/useLiveSocket.ts` — singleton module-level (socket partagé entre tous les callers). Connect → subscribe aux 3 topics, `switchNetwork(id)` purge buffers + rouvre, `dispose()` ferme proprement. Reconnect exponentiel 250 ms → 5000 ms cap. Répond `pong` aux `ping` serveur. Guard HMR (`import.meta.hot.dispose`) qui ferme le socket pour éviter les fuites inter-reloads.
- [x] `composables/useLiveBlocks.ts` / `useLiveTransfers.ts` / `useLiveAtsFeed.ts` — **async** composables. `await useFetch(...)` pour obtenir le seed SSR, puis `store.seed*(data.value)` synchrone + `watch(data)` pour reseeder sur `?network=` switch. Retournent `{ blocks|transfers|items, pending, error }` avec les refs venant de `storeToRefs(store)`.
- [x] `composables/useConnectionState.ts` — thin wrapper : `storeToRefs(useLiveStore()).connection`. `useWallClock.ts` inchangé (déjà Phase 6).
- [x] `plugins/live.client.ts` — `defineNuxtPlugin` client-only qui `watch(activeId, socket.switchNetwork, { immediate: true })`. Ouvre le WS dès la première nav client, le referme/rouvre sur changement de réseau, reste ouvert pour toute la session.
- [x] `AppFooter.vue` branché : `ConnectionIndicator :state="connection"` (depuis `useConnectionState`) + `FooterHeadChip :head="head" :finalized="finalizedHead"` (depuis `storeToRefs(live).head/finalizedHead`). Rend `BLOCK — / FINALIZED —` quand le store est vide (cas SSR si la page ne consomme pas `useLiveBlocks`).
- [x] `layouts/default.vue` : `await useLiveBlocks({ count: 25 })` au niveau du layout. Raison : le footer vit dans le layout, donc il rend avant que le setup de la page appelle `useLiveBlocks`. Un appel layout-level seed le store *avant* que `<AppFooter/>` ne lise `head`/`finalizedHead`. `useFetch` déduplique par URL, donc les pages qui appellent aussi `useLiveBlocks` ne re-fetchent pas.
- [x] `pages/index.vue` : probe Phase 8 avec deux panels live (blocks + transfers), consommant `useLiveBlocks` et `useLiveTransfers`. Rows `.row-fade-in` pour valider le flash à l'arrivée d'un nouveau frame WS côté client. Placeholder remplacé par le vrai dashboard en Phase 9.
- [x] Validation : `bun run typecheck` → 0 erreur. `curl http://127.0.0.1:3000/?network=allfeat` → SSR contient `BLOCK 2 215 917 / FINALIZED 2 215 915` (numéros réels), 10 lignes de blocks rendues (pas de skeleton). `curl http://127.0.0.1:3000/?network=melodie` → `Melodie` en network switch, `BLOCK 682 256 / FINALIZED 682 254`. Payload Nuxt contient `"live":N` (store Pinia sérialisé pour hydration). WS backend validé via `bun run /tmp/ws-smoke.ts` : `{"type":"block","data":{"number":"2215926",...}}` reçu sur `/api/v1/live?network=allfeat`.

Décisions captées au portage :
- **Async composables + seed synchrone** : les composables `useLive*` sont `async` pour pouvoir `await useFetch(...)`. Sans `await`, la page rend avant que `data.value` ne soit peuplé → footer montre `—`. Après l'await, un `if (data.value.length) store.seed*(data.value)` synchrone garantit que le store est seedé avant le render pass.
- **Seed au niveau du layout, pas de la page** : première tentative avec `useLiveBlocks` uniquement dans `pages/index.vue` → le footer SSR rendait `BLOCK —` car le layout (qui contient `<AppFooter/>`) setup avant la page. En déplaçant le `await useLiveBlocks()` au top du layout, le store est peuplé avant que le footer ne lise son state. Les pages gardent leur propre appel pour du seed plus large (ex. `count: 25`) si nécessaire — `useFetch` dédup par URL.
- **Socket singleton module-level, plugin client-only driver** : le state (`let socket: WebSocket | null`) vit au niveau du module `useLiveSocket.ts`, pas dans le store. Ça permet au composable d'exposer une API `{ switchNetwork, dispose }` stateless côté consommateur tout en gardant une unique connexion par onglet. Le plugin `live.client.ts` est le seul consommateur qui appelle `switchNetwork` ; tout le reste passe par le store Pinia.
- **Wire type case mismatch** : `Topic::AtsFeed` sérialise en `"ats_feed"` mais `ServerMsg::AtsItem` sérialise en `"ats_item"` (deux formes différentes côté wire, confirmées par les tests de protocole Rust). Les types TS reprennent ces deux formes distinctes sans tenter d'unifier.
- **HMR guard** : Vite hot-reload le module `useLiveSocket.ts` → le `let socket = null` se réinitialise mais la vraie WebSocket côté navigateur reste ouverte (fuite). `import.meta.hot.dispose` la ferme avant que le module ne disparaisse ; le plugin ré-importe au prochain tick et rouvre proprement.
- **`immediate: activeId.value !== null`** dans `useFetch` : évite une requête vers `/networks/unknown/blocks` quand le store networks n'est pas encore chargé (cas extrême, essentiellement paranoia). Une fois le plugin `networks.server.ts` résolu, `activeId.value` est toujours non-null.
- **Connection state SSR = 'connecting'** : comportement honnête (le plugin `.client.ts` n'a pas encore tourné). Flippe à `'connected'` après hydratation + open WS. Vu comme acceptable — la transition est < 100ms en dev, invisible à l'œil.

### Phase 9 — Pages (~3 j)

Ordre d'exécution effectivement suivi :

1. [x] `pages/blocks/index.vue` + `components/tables/BlocksTable.vue` (pilote) — pagination URL-driven via `?page=`, filtre segmenté via `?filter=` (all/finalized/with-ext), deux `useFetch` parallèles (`/blocks/head` gelé par réseau + `/blocks?count=25&from=…`). Head gelé sur switch réseau uniquement → la pagination ne glisse pas quand la chaîne avance pendant la session.
2. [x] `pages/blocks/[number].vue` + `components/detail/BlockDetail.vue` — KV overview (Timestamp/Status/Hash/Parent/State root/Extrinsics root/Author/Ref time/Proof size/Spec/Size) + Summary panel (extrinsics, events, weight bar, block time target) + Tabs `?tab=extrinsics|events` (pas de tab Log/State traces car données fabriquées côté maquette). Prev/Next via `NuxtLink` + BigInt math pour u64-safe.
3. [x] `pages/extrinsics/index.vue` + `components/tables/ExtrinsicsTable.vue` + `pages/extrinsics/[id].vue` + `components/detail/ExtrinsicDetail.vue` — liste filtrée (all/signed/failed), pas de pagination côté liste car l'API ne supporte pas encore de cursor (`latest N only`, cappé à 100). Détail avec KV + Result side-panel + Tabs `?tab=parameters|events|raw`. Le tab Parameters déroule le `ExtrinsicArgs` tagged union (Transfer/Timestamp/Raw) via un helper `argsVariant` — pas de `as any` en template.
4. [x] `pages/accounts/index.vue` + `components/tables/AccountsTable.vue` + `pages/accounts/[address].vue` + `components/detail/AccountDetail.vue` + `components/ui/BalanceDonut.vue` — liste top-20 par balance. Détail affiche donut 2-segments (transferable teal + reserved red, ratios BigInt-safe) + Overview KV + panel ATS registrations (fetch `/accounts/:address/ats?limit=10` + `/ats/count`). Tabs Extrinsics/Transfers/Balance history de la maquette **supprimées** car le backend n'expose pas encore ces queries par compte — remplacées par le panel ATS seul.
5. [x] `pages/ats/index.vue` + `components/tables/AtsTable.vue` + `components/ui/AtsStatsStrip.vue` + `pages/ats/[id].vue` + `components/detail/AtsDetail.vue` — hero copy + stats strip (total/last24h/last7d/unique_owners/protocol) + feed paginé via `?page=` (le backend expose `from_index` absolu → pagination par multiplication). Détail = On-chain record KV + Deposits list + Version history table (réordonnée latest-first).
6. [x] `pages/index.vue` (dashboard) + `components/hero/HeroLiveCard.vue` — hero live card (head block number ticker, finalized head, 3-col stats, ref-time bar, countdown dérivé de `useWallClock` + `block_time_secs`) + `AtsStatsStrip` + deux panels compact (Latest blocks + Latest transfers) avec `row-fade-in` pour flash sur push WS. Les blocs et transferts sortent du store Pinia live (pas de re-fetch).

Utilitaires ajoutés à `web/app/utils/format.ts` :
- `fmtInt(n)` — thousands separators, accepte `number | string | bigint` (BigInt-safe pour u64/u128 venant de l'API)
- `fmtAFT(planck, decimals=12, fix=4)` — planck → AFT via math BigInt (whole/frac split), fallback exponential sous 0.0001
- `fmtUtcTime(ms)` — "YYYY-MM-DD HH:MM:SS UTC" verbatim depuis la maquette

Chaque page respecte le contrat du plan :
- `useFetch` pour seed SSR, keyé sur `activeNetwork + params` via la fonction URL (Nuxt re-fetch automatiquement à leur changement).
- Composant de contenu dans `components/{tables,detail,hero}/` qui prend les données en props — le page file ne fait que routing/fetches/404.
- `<SkeletonRows/>` avec `columns` tracks-CSS pour éviter le shift d'hydratation.
- 404 inline via panneau dédié (pas de redirect). L'erreur `useFetch` → rendu du panel "Not found" + CTA de retour.

Décisions captées au portage :
- **Route naming conflict `ats/[index].vue` vs `ats/index.vue`** : Nuxt génère deux routes de même nom dynamique parce que le segment `index` est littéralement le mot-clé de la route par défaut. Renommé en `ats/[id].vue` avec param `id` — même convention que `extrinsics/[id].vue`, cohérent.
- **Head gelé par réseau** : le `totalPages` des blocs utilise le head fetché au premier rendu du réseau, pas re-fetché à chaque changement de page. Évite que la page `?page=3` disparaisse dès qu'un nouveau bloc arrive (le nombre total monterait, décalant tous les offsets). Switch réseau → nouveau head → pagination réinitialisée.
- **BigInt dans les key functions `useFetch`** : pour construire `from = head - (page - 1) * count` sur des u64, on manipule `BigInt` et on sérialise en string dans l'URL. Number serait faux au-delà de 2^53.
- **ExtrinsicArgs variant narrowing sans `as any`** : le helper `argsVariant(args)` retourne un objet `{ kind, ...payload }` qui permet au template de switcher sur `variant.kind` sans lever de types. Plus lisible que des `v-if="'Transfer' in extrinsic.args"` partout.
- **Compte ATS par compte** : triple fetch en parallèle sur la page account detail (`/account`, `/account/ats`, `/account/ats/count`) plutôt qu'un `Promise.all` imbriqué — Nuxt `useFetch` x3 est une façon propre d'exprimer trois dépendances indépendantes, et chaque appel a son propre `default` pour que l'affichage ne casse pas si l'un échoue.
- **`Pagination` URL-push vs Tabs URL-replace** : la pagination pousse l'historique (back button rewind les pages, attendu) ; les tabs le remplacent (toggler des tabs plusieurs fois ne doit pas empiler d'entrées). Les deux composants existent depuis Phase 6, consommés verbatim ici.
- **Tabs Log/State traces et Extrinsics/Transfers par compte supprimées** : présentes dans la maquette mais fabriquées côté JS (`Math.random()`, slice de données globales). Au v3 on ne rend que ce que le backend expose ; ajouter des tabs vides serait un faux positif visuel.

Validation (tous les routes HTTP 200 avec payload non-trivial) :
| Route | Taille SSR | Contrôles |
|---|---|---|
| `/` | 48 KB | hero-card + stats strip + 2 tables compact (16 `row-fade-in`) |
| `/blocks` | 62 KB | `.page-title` + `.seg` + 25 rows + pagination |
| `/blocks/2216028` | 34 KB | KV 11 rows + Summary + Tabs + liste d'extrinsics dans le bloc |
| `/blocks?page=3` | 65 KB | rangée de numéros 25 plus bas que page 1 |
| `/blocks?filter=finalized` | 60 KB | filtre client exclut l'unfinalized tête |
| `/extrinsics` | 136 KB | 50 rows × 8 cols (Extrinsic ID/Block/Hash/Call/Signer/Fee/Result/Time) |
| `/extrinsics?filter=failed` | 56 KB | uniquement les `pill.fail` |
| `/accounts` | 48 KB | 20 rows rank + donut pas rendu (pas de detail) |
| `/ats` | 79 KB | hero + stats strip (Total/Last24h/Last7d/Unique/Protocol) + table feed |
| `/ats?page=5` | 79 KB | offset 100 → items 100..125 |
| `/ats/443224` | 32 KB | On-chain record KV + Deposits + Version history |

`bun run typecheck` : 0 erreur. Tous les routes testés en `?network=melodie` retournent l'id et les numéros de bloc corrects (ex. `/blocks?network=melodie` → head autour de #682k, sub-title `MELODIE TESTNET`).

Validation visuelle vs maquette reportée en Phase 10 (screenshot diff manuel après Playwright), mais la structure des classes (`.page-title`, `.panel`, `.kv`, `.tabs`, `.seg`, `.pill.*`, `.chip.*`, `.hash`, `.hash-dim`, `.row-fade-in`) est celle de la maquette verbatim.

### Phase 10 — Tests & validation (~1 j)

- [ ] Playwright : adapter `end2end/playwright.config.ts` pour lancer **les deux** runtimes :
  ```ts
  webServer: [
    { command: 'cargo run --features ssr,mock', port: 8088, reuseExistingServer: !CI },
    { command: 'bun --cwd web run dev', port: 3000, reuseExistingServer: !CI },
  ]
  ```
- [ ] Porter les tests existants (`baseline.spec.ts`, `hydration.spec.ts`, `live.spec.ts`) sur les nouveaux sélecteurs Vue.
- [ ] `bun --cwd web run typecheck` : zéro erreur TS.
- [ ] `bun --cwd web run build` : build prod OK.
- [ ] `cargo build --release --features ssr,mock --bin allfeat-explorer` : OK.
- [ ] `cargo clippy --features ssr -- -D warnings` : OK.
- [ ] Screenshots diff manuel vs maquette HTML (page par page).

### Phase 11 — Cleanup & docs (~0,5 j)

- [ ] README.md : section "Development" réécrite (lancer `bun dev` à la racine → cargo + nuxt)
- [ ] `docs/frontend-v2-plan.md` → déplacé en `docs/archive/frontend-v2-plan.md`
- [ ] Ce plan renommé en `docs/frontend-architecture.md` (post-mortem + conventions)
- [ ] Archiver `artifacts/`, `migrations/` déjà OK.
- [ ] Supprimer dépendances dev Leptos si restantes.
- [ ] Commit final + PR.

### Estimation totale

| Phase | Durée |
|---|---|
| 0 — bascule | 0,5 j |
| 1 — strip Leptos backend | 1 j |
| 2 — API REST | 1,5 j |
| 3 — ts-rs | 0,5 j |
| 4 — scaffold Nuxt | 0,5 j |
| 5 — design system SCSS | 1 j |
| 6 — primitives UI | 1,5 j |
| 7 — layout | 1 j |
| 8 — live | 1 j |
| 9 — pages | 3 j |
| 10 — tests | 1 j |
| 11 — cleanup | 0,5 j |
| **Total** | **≈ 13 j homme** |

## Risques & mitigations

| Risque | Mitigation |
|---|---|
| Désync entre types Rust et types TS | ts-rs génère à chaque `cargo test --features ts-bindings` ; ajouter un check CI qui bloque si `bindings/` diverge |
| `u64`/`u128` précision JS | ts-rs export `string` par défaut ; le format côté UI est déjà string-based (planck, block number) |
| WS reconnexion sur HMR dev Nuxt | `useLiveSocket` détecte HMR via `import.meta.hot` et ne reconnecte qu'en cas de socket mort |
| Divergence visuelle vs maquette | Port verbatim des classes CSS ; screenshots diff en Phase 10 |
| Nuxt 4 jeune (bugs éventuels) | Fallback documenté : downgrade vers Nuxt 3 si blocage, aucune API Nuxt 4-only utilisée |
| CORS / proxy cassé en prod | Tester reverse proxy nginx+caddy localement avant deploy ; doc prod dédiée |
| Bun runtime incompat sur lib native (sass) | `sass-embedded` a un binaire Dart, testé avec bun ; fallback `sass` pur-JS si souci |
| Perte de logique subtile (format, identicon, delta timers) | Lecture ligne-à-ligne de `src/format.rs` et `data.js` au portage `utils/` ; tests unitaires Vitest sur les formatters |

## Conventions de maintenance post-rewrite

- Tout nouveau type API doit passer par `src/domain.rs` + `#[derive(TS)]`. Pas de types TS hand-written qui dupliquent des types Rust.
- Un nouveau endpoint REST : handler dans `src/server/api/<module>.rs`, monté dans `src/server/api/mod.rs`, consommé via `useApi()` côté Vue.
- Un nouveau topic live : variant dans `Topic` (`src/live/protocol.rs`), producer côté `live::server`, store Pinia + composable `useLive<Topic>` côté Vue.
- SCSS : pas de CSS-in-JS, pas de `<style>` inline non-scopé. Toute règle globale vit dans `assets/styles/components/`.
- TypeScript strict, zéro `any`. Utiliser les types de `@bindings` partout où on manipule une réponse API.
- Theme : géré exclusivement par `prefers-color-scheme`. Pas de toggle.

## Référence baseline

- Maquette : `~/Downloads/allfeat-chain-explorer/project/`
- Backend pré-rewrite (islands) : commit `9325e25` sur `master`
- Backend pré-îlots (baseline Leptos SSR complet) : commit `64786fd` sur `master`
- `src/format.rs` pré-suppression : `git show 64786fd:src/format.rs` — à consulter lors du portage `web/app/utils/format.ts`
