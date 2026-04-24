# Frontend v2 — Leptos islands rewrite plan

Status: planned. Branch: `feat/frontend-islands` (to be created).

The current SSR + full-hydrate frontend exhibits unstable hydration warnings,
flashes between SSR and client-side renders on navigation, FOUC on theme,
and brittle init ordering between clock signals, the live WebSocket and
component effects. Rather than patch in place, we rewrite the frontend
under the **Leptos 0.8 islands model** with strict visual parity.

## Reference documentation (always consult)

The single source of truth for the Leptos islands model is the official
book:

- <https://book.leptos.dev/islands.html>

Cross-check this page whenever the work touches:

- `#[island]` macro usage or design
- `leptos/islands` Cargo feature setup
- `hydrate_islands()` mount entry point
- Props serialization across the SSR ↔ island boundary
- `Suspense` / `Resource` interaction inside vs around islands
- Server function context propagation in island mode
- `leptos_router` integration in islands mode

Each phase below that touches an island will explicitly call this out.

## Decisions captured

| Sujet | Décision |
|---|---|
| Modèle de rendu | **Leptos 0.8 islands natif** (`leptos/islands` + `hydrate_islands()`) |
| Parité visuelle | **Stricte** — DOM produit byte-équivalent, SCSS conservé |
| Theme | **`prefers-color-scheme` CSS pur** — pas de JS, pas de localStorage. Implication : suppression du bouton toggle theme et du state `HeaderVariant`. Seule entorse à la parité visuelle, documentée. |
| `<ConnectionIndicator>` | **Footer**, pastille discrète + label. |
| `<NewItemFade>` | **Accent bleu thème** (`rgba(125, 163, 255, .18)` → 0 sur 2 s, fade-in 300 ms). |
| Buffer live sur switch réseau | **Purge immédiate** des 3 stores (Blocks, Transfers, AtsFeed). |
| Stores live | **Globaux par topic**, posés dans l'island racine, survivent aux navigations. |
| Backend | **Inchangé** — server fns, protocole WS, providers, indexer, domain types restent en l'état. |

## Trois règles structurantes

1. **Rendu par défaut statique.** Tout composant est du HTML mort SSR ; seul un `#[island]` hydrate.
2. **Parité visuelle stricte.** Mêmes classes CSS, même structure DOM, même contenu textuel. SCSS et palette intacts.
3. **Minimum d'islands, maximum de statique.** Chaque island = surface de WASM hydratée + risque de mismatch. Succès mesuré en % de DOM non-hydraté (cible : ≥ 70 %).

## Périmètre du rewrite — fichiers touchés

### Conservés tels quels (zéro ligne touchée)

- `src/domain.rs`, `src/format.rs`, `src/network.rs`
- `src/data/**`, `src/server/**`, `src/indexer/**`
- `src/live/{protocol.rs, server.rs, merge.rs}`
- `src/main.rs`, `src/lib.rs` (patch mineur sur `lib.rs` pour `hydrate_islands()`)
- `style/main.scss`
- `end2end/`, `tests/`

### Wipés et réécrits sous le modèle islands

- `src/app/` (entier)
- `src/pages/` (entier — view markup repris en référence depuis le commit `64786fd`)
- `src/ui/` (entier — mêmes classes CSS, discipline island/statique)
- `src/live/client.rs`, `src/live/mod.rs`

### Ajoutés

- `src/live/store.rs` — `LiveStore<T>` générique
- `src/live/connection.rs` — `<LiveConnection>` mount + `ConnectionState` signal

## Architecture cible

### Inventaire : statique vs island

| Composant | Type | Justification |
|---|---|---|
| Layout `<html>`/`<head>`/`<body>` | Statique | Pas d'interactivité |
| `<Shell>` | Statique | Conteneur + breadcrumb + titres |
| `<Header>` (logo, nav links) | Statique | Liens purs |
| `<NetworkSwitch>` | **Island** | Dropdown + navigation |
| `<Footer>` (liens + container) | Statique | Liens purs |
| `<FooterHeadChip>` | **Island** | Lit head block live |
| `<ConnectionIndicator>` | **Island** | Lit `ConnectionState` |
| `<IndexingBanner>` | **Island** | Poll `get_indexer_status` 5 s |
| `<Breadcrumb>`, titres, filtres labels, `<thead>` | Statique | Présentation pure |
| `<tbody>` listes (Blocks, Extrinsics, Accounts, ATS feed) | **Island** | Live + `<For>` + row transitions |
| `<Pagination>` | **Island** | State + navigation |
| `<FilterSegment>` (All/Finalized/…) | **Island** | State local |
| Tab bar (AccountDetail, AtsDetail) | **Island** | Tab state |
| `<Hero>` layout | Statique | Texte + structure |
| `<HeroCountdown>` | **Island** | Tick wall clock |
| `<HeroLiveCard>` | **Island** | Signal live |
| `<TimeAgo>` | **Island** | Tick wall clock |
| `<CopyButton>` | **Island** | Clipboard |
| `<Identicon>`, `<Hash>`, `<StatusPill>`, `<KV>`, `<Pill>` | Statique | Pur |
| Pages de détail (Block, Extrinsic, Account, Ats) | **Statiques à 95 %** | Données historiques immuables ; seules micro-islands `<TimeAgo>` / tabs / copy |

### Graphe de composants

```
<Shell>                                 // static
  <Header>                              // static
    <Logo/>                             // static
    <Nav/>                              // static
    <NetworkSwitch/>                    // island
  </Header>
  <IndexingBanner/>                     // island
  <AppRoot>                             // ★ island racine — pose tous les contextes
    provide_context(LiveStore<Block>)
    provide_context(LiveStore<Transfer>)
    provide_context(LiveStore<AtsFeedItem>)
    provide_context(ConnectionState)
    provide_context(ChainClock)
    provide_context(WallClock)
    <LiveConnection/>                   // mount-only, sans vue
    <Routes>
      <Route path="/">          <DashboardPage/>
      <Route path="/blocks">    <BlocksPage/>
      …
    </Routes>
  </AppRoot>
  <Footer>                              // static
    <FooterLinks/>
    <FooterHeadChip/>                   // island
    <ConnectionIndicator/>              // island
  </Footer>
</Shell>
```

### Pattern de page systématique

```rust
#[component]                            // STATIQUE
pub fn BlocksPage() -> impl IntoView {
    let initial = Resource::new_blocking(…, latest_blocks(…));
    view! {
        <div class="container">
          <Breadcrumb …/>
          <h1>"Blocks"</h1>
          <div class="panel">
            <table class="table">
              <thead>…</thead>
              <Suspense fallback=|| view!{<SkeletonRows cells=8 rows=25/>}>
                { move || initial.get().map(|data| view! {
                    <BlocksTableBody initial=data/>   // ← island
                }) }
              </Suspense>
            </table>
            <Pagination …/>              // island
          </div>
        </div>
    }
}

#[island]                               // HYDRATÉ
fn BlocksTableBody(initial: Vec<Block>) -> impl IntoView {
    let store = use_context::<LiveStore<Block>>().unwrap();
    Effect::new({
        let initial = initial.clone();
        move |ran: Option<bool>| {
            if !ran.unwrap_or(false) { store.seed(initial.clone()); }
            true
        }
    });
    let rows = Signal::derive(move || store.snapshot());
    view! {
        <tbody>
          <For each=rows key=|b| b.number let:b>
            <BlockRow block=b/>
          </For>
        </tbody>
    }
}
```

Les types passés en props d'island (`Block`, `Transfer`, `Extrinsic`, `Account`,
`AtsRecord`, `AtsFeedItem`) ont déjà `Serialize + Deserialize` dans
`src/domain.rs` — aucun ajout requis pour la sérialisation islands.

### `LiveStore<T>`

```rust
pub struct LiveStore<T: LiveItem + Clone + 'static> {
    items: RwSignal<VecDeque<T>>,
    capacity: usize,
}

impl<T: LiveItem + Clone + 'static> LiveStore<T> {
    pub fn seed(&self, initial: Vec<T>);
    pub fn push_live(&self, item: T);
    pub fn snapshot(&self) -> VecDeque<T>;
    pub fn clear(&self);  // appelé sur switch réseau
}
```

Trois instances posées dans `<AppRoot>` via `provide_context`. Toutes les
islands enfants (table body, hero, footer chip) y accèdent via `use_context`.
Une seule instance par tab, survit aux navigations route → route.

### `LiveClient` + `<LiveConnection>`

- `LiveClient` encapsulé via `SendWrapper<Rc<LiveClient>>` (single-thread wasm,
  satisfait les bounds `Send + Sync` de Leptos context). Assertion debug
  `cfg!(target_arch = "wasm32")` à l'extraction.
- `<LiveConnection>` est un component fantôme (vue vide) à l'intérieur de
  `<AppRoot>` qui exécute **un unique `Effect::new`** au mount : connecte
  le WebSocket, câble les handlers vers les stores, maintient `ConnectionState`.
- `ConnectionState` = `RwSignal<ConnState>` :
  ```rust
  enum ConnState {
      Connecting,
      Connected,
      Reconnecting { attempt: u32, next_in_ms: u32 },
      Offline,
  }
  ```
- Sur `set_network` : ferme le socket, **purge les 3 stores via `clear()`**,
  ouvre le nouveau socket avec le nouveau réseau.

## Plan phase par phase

### Phase 0 — Préparation (½ j)

- [x] Commit baseline backend sur `master` (commit `64786fd`)
- [x] Persistance de ce plan dans `docs/frontend-v2-plan.md` (commit suivant)
- [ ] Création branche `feat/frontend-islands`
- [ ] Baseline Playwright : screenshots actuels + log des warnings console pour comparaison post-rewrite
- [ ] Wipe ciblé : `src/app/`, `src/pages/`, `src/ui/`, `src/live/{client,mod}.rs`
- [ ] Stub minimal compilant (App vide, pas de routes) + commit "wipe frontend for islands rewrite"

### Phase 1 — Activation islands (½ j)

→ Référence : <https://book.leptos.dev/islands.html> (sections "Setup", "Routing")

- Cargo : ajouter `leptos/islands` aux features `hydrate` et `ssr`
- `src/lib.rs` : `hydrate_islands()` au lieu de `hydrate_body(App)`
- `src/main.rs` : adapter `generate_route_list` + `leptos_routes` au mode islands
- Validation : serveur boot, page minimale rend en mode islands

### Phase 2 — `<Shell>` statique + `<AppRoot>` island (1 j)

→ Référence : <https://book.leptos.dev/islands.html> (sections "Islands", "Sharing Context")

- `src/app/shell.rs` : `<Shell>`, `<Header>` statique, `<Footer>` statique
- `src/app/app_root.rs` : `#[island] fn AppRoot()` qui wrappe `<Routes>` et pose tous les contextes (stores, ConnectionState, clocks)
- `<Suspense>` racine pour `enabled_networks` — fini les `get_untracked` dans Memo
- Pas de `data-theme` JS ; remplacé par CSS `prefers-color-scheme` dans `style/main.scss`

### Phase 3 — Clock propre (½ j)

- `ChainClock` + `WallClock` deviennent `RwSignal<Option<i64>>` : SSR = `None`, hydrate snap puis tick
- Toute lecture dans une island affiche `—` si `None`
- Suppression de `boot_now_ms` et du padding 2 s
- Effets timer câblés dans `<AppRoot>` island, post-mount uniquement

### Phase 4 — `LiveStore` (1 j)

- `src/live/store.rs` : `LiveStore<T>` générique avec `seed`, `push_live`, `snapshot`, `clear`
- 3 instances dans `<AppRoot>` via `provide_context`
- Tests unitaires : seed + push + dedup + cap + équivalence post-navigation simulée

### Phase 5 — `LiveClient` + `<LiveConnection>` (1,5 j)

→ Référence : <https://book.leptos.dev/islands.html> (sections sur Effects côté island)

- Réécriture `src/live/client.rs` sans thread-local — vit dans le contexte via `SendWrapper`
- `src/live/connection.rs` : `<LiveConnection>` mount-only avec unique `Effect::new`
- `ConnectionState` exposé via context, consommé par `<ConnectionIndicator>` island
- Purge stores sur switch réseau

### Phase 6 — Micro-islands header/footer (½ j)

- `#[island] NetworkSwitch` (refacto)
- `#[island] FooterHeadChip`
- `#[island] ConnectionIndicator` (footer, pastille + label)
- `#[island] IndexingBanner` (poll 5 s)

### Phase 7 — Pages (3,5 j)

→ Référence : <https://book.leptos.dev/islands.html> (sections "Server Functions", "Resources in Islands")

Ordre de migration :

1. `BlocksPage` (pilote — pattern reference)
2. `ExtrinsicsPage`
3. `AccountsPage`
4. `AtsPage`
5. `DashboardPage`
6. `BlockDetailPage`
7. `ExtrinsicDetailPage`
8. `AccountDetailPage`
9. `AtsDetailPage`
10. `NotFoundPage`

Pour chaque page :

- Composant statique = shell (titre, breadcrumb, filtres, table outline, pagination outline)
- 1 à 3 islands ciblées (table body live, tab content, copy button)
- `Resource::new_blocking` dans le composant statique, résultat passé en props aux islands (sérialisé via Leptos islands)
- `<Suspense fallback=<SkeletonRows />>` pendant le fetch
- `<NewItemFade>` appliqué sur les rows nouvelles (premier mount DOM)
- Markup repris **verbatim** depuis le commit baseline `64786fd` pour parité visuelle

### Phase 8 — Primitives UI (½ j)

- `<SkeletonRows rows=N cells=M />` : statique, dimensions exactes des rows réelles → zéro layout shift
- `<ConnectionIndicator />` : pastille footer, classes `.connection-pill--{connected,reconnecting,offline}`
- `<NewItemFade>` : wrapper sur `<tr>` qui applique `animation: fade-in 300ms ease-out, highlight-fade 2s ease-out` au premier mount via `NodeRef`
- Ajouts SCSS isolés : `.skeleton-shimmer`, `.row-fade-in`, `.connection-pill--*`. Aucune classe existante modifiée.

### Phase 9 — Tests & validation (1 j)

- `cargo test --features ssr,islands --lib` vert
- `pnpm --dir end2end exec playwright test` vert sur baseline + tests neufs (reconnection WS, fade live)
- **Critère qualité** : `chrome devtools` ouvert sur `http://127.0.0.1:3000`, navigation sur toutes les routes, **0 warning d'hydration** dans la console
- Diff bundle WASM avant / après (cible : −30 à −50 %)
- Lighthouse FCP avant / après
- Diff screenshots Playwright : parité visuelle validée (avec exception explicite pour le bouton theme retiré)

### Phase 10 — Cleanup & docs (½ j)

- Suppressions résiduelles si oubliées
- `docs/frontend-v2-plan.md` → `docs/frontend-architecture.md` (post-mortem + conventions de maintenance des islands)
- Section `README.md` "Frontend architecture (islands)"
- Mise à jour `MEMORY.md` côté Claude si nouvelles conventions à pin

## Risques & mitigations

| Risque | Mitigation |
|---|---|
| Props island non-serializable | Audit préalable : tous les types passés sont `Serialize/Deserialize` dans `domain.rs` |
| `SendWrapper<LiveClient>` panique hors wasm | Assertion `cfg!(target_arch = "wasm32")` à l'extraction du context |
| Navigation inter-routes ne déclenche pas re-fetch SSR | `<AppRoot>` island contient `<Routes>` → SPA navigation native, `Resource::new_blocking` refetch sur changement de clé |
| `use_context` dans une island ne trouve pas le contexte d'`AppRoot` | Tous les contextes posés dans l'island racine, vérification au mount via `.expect("AppRoot must provide")` |
| Mismatch d'hydration sur du HTML statique | Par construction impossible : statique n'hydrate pas |
| Server fns appelées depuis des islands vs depuis statique — sémantique différente ? | Identique en 0.8 ; tests unitaires couvrent les deux cas |
| `leptos/islands` jeune, bugs possibles | Phases 1-2 valident tôt sur un écran simple avant d'attaquer toutes les pages |
| Perte de fix subtils du code actuel | Lecture systématique de chaque fichier wipé via `git show 64786fd:<path>` ; report des invariants non-obvious comme commentaires dans le nouveau code |
| Régression visuelle sur Playwright | Screenshots baseline pris en Phase 0, comparés à chaque phase |

## Estimation

| Phases | Durée |
|---|---|
| 0 → 3 (bootstrap + plomberie) | 2,5 j |
| 4 → 5 (stores + client) | 2,5 j |
| 6 (micro-islands header/footer) | 0,5 j |
| 7 (pages) | 3,5 j |
| 8 (primitives) | 0,5 j |
| 9 → 10 (validation + cleanup) | 1,5 j |
| **Total** | **≈ 11 j homme** |

## Conventions de maintenance post-rewrite

À reporter dans `docs/frontend-architecture.md` à la Phase 10 :

- Tout nouveau composant interactif doit être un `#[island]` ; tout composant purement présentationnel reste statique.
- Tout nouvel `#[island]` doit cross-checker <https://book.leptos.dev/islands.html> (props, Suspense, contexte, server fns).
- Aucun signal n'est créé en dehors d'une island ou d'un Effect côté island.
- Les types passés en props entre statique → island doivent être `Serialize + Deserialize`.
- Aucun `web_sys` / `js_sys` / `localStorage` en dehors d'une island.
- Theme : géré exclusivement par CSS `prefers-color-scheme`. Pas de toggle.

## Référence baseline

Le code frontend pré-rewrite est figé sur `master` au commit `64786fd`. Toute
question "comment ça marchait avant ?" se résout par `git show 64786fd:<path>`.
