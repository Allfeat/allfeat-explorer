# Plan de qualité — Maintenabilité & lisibilité

> **Document de travail vivant.** Aucune phase ne change la sémantique. Chaque
> refactor doit passer `cargo test --features ssr` + `cargo test --features hydrate`
> + `cargo leptos build` sans diff fonctionnel (snapshots Playwright
> identiques).

## Suivi global

| # | Phase                                                       | Priorité | Statut |
|---|-------------------------------------------------------------|----------|--------|
| 1 | Scinder `src/data/rpc/mappers.rs` (1 317 lignes)            | 🔴 haute | ⬜     |
| 2 | Scinder `src/pages/{ats_detail, account_detail, dashboard}` | 🟠 moy.  | ⬜     |
| 3 | Factoriser `live_*_or_empty` + `AtsStrip`                   | 🟠 moy.  | ⬜     |
| 4 | Unifier `net_href` / `use_net_href`                         | 🟡 bas.  | ⬜     |
| 5 | Isoler les helpers web-sys hors de `src/app.rs`             | 🟡 bas.  | ⬜     |
| 6 | Ajouter coverage de tests sur les zones nues                | 🟠 moy.  | ⬜     |
| 7 | Introduire `clippy::pedantic` sélectif + `cargo fmt` en CI  | 🟡 bas.  | ⬜     |

Légende : ⬜ à faire · 🟡 en cours · ✅ terminé · ⚠️ bloqué

---

## Phase 1 — Scinder `mappers.rs`

### Problème
`src/data/rpc/mappers.rs` = 1 317 lignes, 5 domaines différents (blocks,
extrinsics, accounts, transfers, ATS) + helpers communs (hex, hash,
SS58). Les tests en bas du fichier sont encore lisibles mais ajouter ~200
lignes de plus devient inévitable pour les phases 1-3 du plan perf.

### Correction
Nouvelle arborescence :
```
src/data/rpc/mappers/
├── mod.rs           (re-exports + helpers communs ~150 l.)
├── common.rs        (hex_bytes, hash_string, short_label, fmt_*, SS58)
├── blocks.rs        (~200 l.)
├── extrinsics.rs    (~250 l. — devient + léger après phase perf 1)
├── transfers.rs     (~100 l.)
├── accounts.rs      (~100 l.)
└── ats.rs           (~450 l.)
```

`mod.rs` ré-exporte tout pour que `src/data/rpc/provider.rs` n'ait rien à
changer :
```rust
pub use blocks::{map_block, fetch_timestamp, aura_slot, author_from_slot, ...};
pub use extrinsics::{extrinsic_id, parse_extrinsic_id, map_extrinsics, ...};
pub use transfers::map_transfers;
pub use accounts::{parse_ss58, fetch_account, fetch_top_accounts, ...};
pub use ats::{fetch_next_ats_id, build_ats_record, build_ats_list, ...};
pub use common::{hex_bytes, hash_string};
```

Les tests de chaque domaine déménagent dans le fichier correspondant, section
`#[cfg(test)] mod tests`.

### Fichiers touchés
- `src/data/rpc/mappers.rs` → devient `src/data/rpc/mappers/mod.rs`
- Création des 5 sous-fichiers
- Aucune modif aux call-sites (re-exports)

### Tests
- `cargo test --features ssr` doit rester vert (mêmes tests, mêmes noms).
- `cargo bloat --release --crates` avant/après pour vérifier qu'on n'a pas
  inflé la binaire.

### Risque
Très faible. Opération mécanique.

---

## Phase 2 — Scinder les grandes pages

### Problème
Trois fichiers pages dépassent 600 lignes :
- `src/pages/ats_detail.rs` : 755
- `src/pages/account_detail.rs` : 665
- `src/pages/dashboard.rs` : 657

Chaque page contient 5-10 `#[component] fn SousComposant()`. Lisibles
aujourd'hui mais la navigation est lente et les diffs PR gros.

### Correction — template

Pour `account_detail` (exemple type) :
```
src/pages/account_detail/
├── mod.rs              (<80 l., assemblage + imports)
├── balance_card.rs     (~120 l.)
├── overview_card.rs    (~100 l.)
├── ats_card.rs         (~150 l.)
└── tabs/
    ├── mod.rs          (switch)
    ├── extrinsics.rs   (~100 l.)
    ├── transfers.rs    (~80 l.)
    └── history.rs      (~60 l.)
```

Règle de split : un `#[component]` = une fonction visible pour le lecteur.
Laisser les `impl` purs (helpers locaux) en haut du fichier du composant qui
les consomme.

### Priorité intra-phase
1. `ats_detail` (755) — le plus dense
2. `account_detail` (665)
3. `dashboard` (657) — déjà bien structuré par sections, facile

### Fichiers touchés
Voir arborescence. Aucune API publique (routing) ne bouge.

### Tests
- Playwright (end2end) : les snapshots visuels ne doivent pas bouger d'un
  pixel.
- `cargo leptos build --release` : même taille de wasm.

### Risque
Moyen — risque de casser un import caché ou un `use super::`.

---

## Phase 3 — Factoriser les duplications

### Duplications identifiées

1. **`live_blocks_or_empty` / `live_ats_or_empty` / `live_transfers_or_empty`**
   Dupliquées entre `src/pages/blocks.rs` et `src/pages/dashboard.rs` :
   ```rust
   #[inline]
   fn live_blocks_or_empty(cap: usize) -> RwSignal<VecDeque<Block>> {
       #[cfg(feature = "hydrate")]
       { crate::live::client::use_live_blocks(cap) }
       #[cfg(not(feature = "hydrate"))]
       { let _ = cap; RwSignal::new(VecDeque::new()) }
   }
   ```
   → Déplacer dans `src/live/mod.rs` (module public) :
   ```rust
   pub fn use_live_blocks_or_empty(cap: usize) -> RwSignal<VecDeque<Block>> { ... }
   pub fn use_live_transfers_or_empty(cap: usize) -> RwSignal<VecDeque<Transfer>> { ... }
   pub fn use_live_ats_feed_or_empty(cap: usize) -> RwSignal<VecDeque<AtsFeedItem>> { ... }
   ```
   Les pages importent et deletent les helpers locaux.

2. **`AtsStrip` / `AtsStatsStrip`** quasi-identique entre `src/pages/dashboard.rs`
   et `src/pages/ats.rs`. Même HTML, même appels, même gestion d'erreur.
   → Créer `src/pages/shared/ats_strip.rs` avec un composant unique :
   ```rust
   #[component]
   pub fn AtsStrip() -> impl IntoView { ... }
   ```
   Les deux pages l'importent. Le refactor peut inclure un `prop` pour
   customiser les libellés si besoin.

3. **Pattern `Resource::new_blocking + match res { Ok(...) | Err(...) }`**
   Répété 20+ fois avec toujours la même shape de fallback (panel dim +
   message). Introduire un helper :
   ```rust
   pub fn render_resource<T, V, F>(
       res: Option<Result<T, ServerFnError>>,
       ok: F,
   ) -> AnyView
   where
       F: FnOnce(T) -> V,
       V: IntoView,
   { ... }
   ```
   Ou mieux : un composant `<ResourceView resource=r view=|t| ... />`.

### Fichiers touchés
- `src/live/mod.rs` : nouveaux helpers
- `src/pages/shared/mod.rs` (nouveau) : composants partagés
- `src/pages/blocks.rs`, `dashboard.rs`, `ats.rs` : suppression des locaux

### Tests
- Playwright inchangé.

### Risque
Faible.

---

## Phase 4 — Unifier `net_href` vs `use_net_href`

### Problème
`src/pages/helpers.rs` expose deux API :
- `pub fn net_href(path: &str) -> String` : re-lookup du spec via
  `use_context` à chaque appel.
- `pub fn use_net_href() -> impl Fn(&str) -> String + Clone + 'static` :
  capture le spec **une fois** en scope de composant.

Les pages utilisent les deux sans logique claire :
- `blocks.rs`, `accounts.rs` utilisent `net_href` direct
- `dashboard.rs` utilise `use_net_href`
- `extrinsics.rs` mélange les deux

### Correction
Garder **uniquement** `use_net_href`. L'autre génère un lookup de contexte
répété — moins efficace et source de "context lost" silencieux si un appel
se fait hors scope reactive.

Migration :
```rust
// Avant:
let href = net_href(&format!("/block/{}", b.number));

// Après (en haut du composant):
let net = use_net_href();
...
let href = net(&format!("/block/{}", b.number));
```

Dans certains closures `<For>` il faudra cloner `net` une fois au niveau
de la boucle.

### Fichiers touchés
- `src/pages/helpers.rs` : supprimer `net_href` (garder `net_a` qui l'utilise
  en interne ou le re-inline)
- ~15 fichiers pages : remplacer les appels

### Tests
- Snapshot Playwright : aucun changement visible.
- `cargo check` : catch les appels non migrés.

### Risque
Faible.

---

## Phase 5 — Isoler les shims web-sys

### Problème
`src/app.rs` mélange :
- La définition de `App` (Leptos, clean)
- Le cycle de vie de `ChainProvider` (Leptos + Effects)
- Des helpers web-sys (`read_local`, `write_local`, `wall_clock_ms`,
  `set_interval`, `clear_interval`) avec `#[cfg(feature = "hydrate")]`

382 lignes avec les imports en double au milieu. La moitié de `app.rs` est
du code plateforme.

### Correction
Nouveau module :
```
src/app/
├── mod.rs           (ex-app.rs, mais sans les web-sys helpers)
├── web_shim.rs      (read_local, write_local, wall_clock_ms, set_interval, clear_interval)
└── clock.rs         (align_to_block, boot_now_ms, real_now_ms)
```

`mod.rs` importe via `#[cfg(feature = "hydrate")] use self::web_shim::*;`.

### Fichiers touchés
- `src/app.rs` → `src/app/mod.rs` + deux nouveaux fichiers
- Imports ailleurs : aucun (ré-exports)

### Tests
- `cargo leptos build` passe.
- Pas de changement visible.

### Risque
Faible.

---

## Phase 6 — Couvrir les zones testées à zéro

### Audit

| Module                              | Tests | Couverture  |
|-------------------------------------|-------|-------------|
| `src/data/rpc/cache.rs`             | 3     | ✅ singleflight, propagation erreur, hit-rate |
| `src/data/rpc/client.rs`            | 3     | ✅ connect, invalidate, retry |
| `src/data/rpc/mappers.rs`           | ~20   | ✅ héros : hashes, ATS id, extrinsic id, accounts |
| `src/data/rpc/provider.rs`          | 0     | ❌ |
| `src/live/protocol.rs`              | 3     | ✅ roundtrip |
| `src/live/merge.rs`                 | 0     | ❌ |
| `src/live/server.rs`                | 0     | ❌ |
| `src/live/client.rs`                | 0     | ❌ (wasm-bindgen-test nécessaire) |
| `src/ui/pagination.rs`              | 0     | ❌ (calcul de range) |
| `src/format.rs`                     | 0     | ❌ (fmt_int, fmt_aft, time_ago) |
| `src/network.rs`                    | 0     | ❌ |

### Correction — priorités

**Niveau 1 — pure functions, zero dép externe** :
- `src/format.rs` :
  - `fmt_int(123_456_789) == "123,456,789"`
  - `fmt_aft(1_500_000_000_000, 12, 2) == "1.5"`
  - `time_ago(t - 61_000, t) == "1 min ago"`
  - `fmt_iso_utc(0) == "1970-01-01 00:00:00 UTC"`
- `src/live/merge.rs` :
  - 10 éléments, live vide → identique à `initial[..limit]`
  - Dedup : initial contient `#5`, live contient `#5` → une seule sortie
  - Overlap partiel + truncation
- `src/ui/pagination.rs` :
  - Calcul du range 1-5 autour de la page courante (pure logic)

**Niveau 2 — avec tokio / mock** :
- `src/live/server.rs` : `spawn_forwarder` + `run_receiver` avec des
  `Stream` synthétiques et un `mpsc::channel` pour le sink.
- `src/data/rpc/provider.rs` : injecter un `AllfeatClient` mocké (possible
  avec `mockall` ou en exploitant un trait sur `at_block`).

**Niveau 3 — end-to-end** :
- Playwright `/blocks` → attendre un nouveau block live, vérifier que la
  liste prepend.
- Playwright switch réseau → URL change, `/ws?network=<new>` réouvre.

### Fichiers touchés
- Ajouter `#[cfg(test)] mod tests` dans les modules sans couverture
- `tests/integration/*.rs` pour le provider avec mock

### Risque
Moyen — certains tests d'intégration demanderont de nouveaux dev-deps
(`mockall`, `wasm-bindgen-test`).

---

## Phase 7 — Lint + format CI

### Problème
Pas de CI qui lint / formate. Un contributeur peut merger du code qui casse
`clippy::suspicious` ou du formatage non conforme — le projet n'a pas de
`rustfmt.toml` ni de `.clippy.toml`.

### Correction

1. `rustfmt.toml` minimal :
   ```toml
   edition = "2021"
   max_width = 100
   use_field_init_shorthand = true
   reorder_imports = true
   group_imports = "StdExternalCrate"
   ```

2. `.clippy.toml` :
   ```toml
   msrv = "1.82"
   disallowed-macros = []
   ```

3. `.github/workflows/ci.yml` :
   ```yaml
   name: ci
   on: [push, pull_request]
   jobs:
     check:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
         - uses: dtolnay/rust-toolchain@stable
           with: { components: 'rustfmt,clippy' }
         - uses: Swatinem/rust-cache@v2
         - run: cargo fmt --all --check
         - run: cargo clippy --all-targets --features ssr -- -D warnings
         - run: cargo clippy --all-targets --features hydrate -- -D warnings
         - run: cargo test --features ssr
   ```

### Fichiers touchés
- `rustfmt.toml`, `.clippy.toml`, `.github/workflows/ci.yml` (nouveaux)

### Étape préalable
Faire un `cargo fmt --all` + `cargo clippy --fix` **avant** de merger le
workflow, sinon la première PR est rouge sur tout le tree.

### Risque
Faible. Un seul gros diff de formatting à valider.

---

## Checklist d'exécution

Avant chaque phase :
- [ ] Branche dédiée
- [ ] Vérifier que les snapshots Playwright sont à jour sur master

Après chaque phase :
- [ ] `cargo test --features ssr && cargo test --features hydrate`
- [ ] `cargo leptos build --release`
- [ ] PR avec diff lisible (éviter les mega-PR : 1 phase = 1 PR)
