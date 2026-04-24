# API pagination & filters — redesign plan

Status: planned. Branche : à créer (`feat/api-pagination`).

L'API HTTP actuelle expose 18 endpoints sous `/api/v1/networks/{network_id}`
avec trois styles de pagination incompatibles, aucune enveloppe de réponse
et aucun filtre côté serveur. Le frontend paye cette dette : double round-trips
pour estimer des totaux, arithmétique BigInt côté JS pour calculer des
cursors de blocs, filtres 100 % client-side qui cassent la pagination.

Ce document verrouille le design d'une pagination unifiée **cursor-based +
transparent**, d'un framework de filtres côté serveur, et d'un composable
Nuxt unique pour consommer tout ça.

## Diagnostic — ce qu'on casse

| Symptôme | Endroit | Conséquence |
|---|---|---|
| `count` / `limit` / `from` / `from_index` cohabitent | `src/server/api/*.rs` | Le frontend doit mémoriser trois shapes |
| Réponses = `Vec<T>` nu, sans métadonnées | Tous les endpoints de liste | Pas de `total`, pas de `has_more`, pas de `next_cursor` |
| Totaux récupérés via des endpoints séparés | `/blocks/head`, `/ats/stats`, `/accounts/{addr}/ats/count` | Round-trips supplémentaires, races possibles |
| Pagination offset pour ATS | `queries/ats.rs` `OFFSET/LIMIT` | Instable dès qu'une ATS est créée pendant la pagination |
| Filtres client-side | `pages/blocks/index.vue`, `pages/extrinsics/index.vue` | Filtrent la slice chargée, pas le dataset → compteur de pages faux |
| `clamp_count` silencieux | `src/server/api/mod.rs:148` | `count=500` retourne 100 sans erreur |
| Blocks page : fetch `head` puis fetch `blocks?from=head-offset` | `pages/blocks/index.vue:43-83` | Race condition si la chaîne avance entre les deux appels |

## Décisions captées

| Sujet | Décision |
|---|---|
| Breaking changes sur l'API v1 | **Autorisés** — backend et frontend dans le même repo, pas de versioning `/v2` |
| Style de pagination | **Cursor-based** uniquement, `has_more` via fetch N+1 |
| Cursors | **Transparents** (lisibles à l'œil nu), pas de base64/JSON |
| Noms de paramètres | `count` + `cursor` partout. `limit`, `from`, `from_index` dégagés |
| Enveloppe de réponse | **`Page<T> { items, page_info }`** pour toutes les listes |
| `total` | **Opportuniste** — présent quand cheap, `None` sinon |
| Invalid cursor | **`400 Bad Request`**, pas de silent-fallback |
| Filtres | **Query-params typés par ressource**, pas de DSL générique |
| Frontend | Composable unique `usePaginatedList<T>` centralisant toute la logique |

## Architecture cible

### Enveloppe de réponse

```rust
// src/domain.rs
pub struct Page<T> {
    pub items: Vec<T>,
    pub page_info: PageInfo,
}

pub struct PageInfo {
    pub total: Option<u64>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

pub struct PageRequest {
    pub count: u32,
    pub cursor: Option<String>,
}
```

Tous les types exportés via `ts-rs` (feature `ts-bindings`) pour que le
frontend ait la même forme en TypeScript.

### Conventions de cursor (transparents, newest-first)

| Ressource | Format | Exemple | Condition "next" |
|---|---|---|---|
| `/blocks` | `<block_num>` | `12345` | `num < 12345` |
| `/extrinsics` | `<block>-<idx>` | `12345-3` | `(block, idx) < (12345, 3)` lexico |
| `/transfers` | `<block>-<event_idx>` | `12345-7` | idem |
| `/events` | `<block>-<phase>-<idx>` | `12345-A-3` | `I`=Init, `A`=ApplyExtrinsic, `F`=Final |
| `/ats` | `<id>` | `42` | `id < 42` |
| `/ats/feed` | `<ats_id>-<version>` | `42-3` | `(id, version) < (42, 3)` lexico |
| `/accounts` | — | — | Pas de cursor, top-N par balance |
| `/accounts/{address}/ats` | `<id>` | `42` | `id < 42` |

Chaque ressource a un newtype dans `src/data/cursor.rs` avec impls
`FromStr` + `Display`. Parsing raté → `ApiError::BadRequest`.

### `has_more` et `total`

- **`has_more`** : technique "fetch N+1". Le repo SQL demande `count + 1`
  rows ; si on en reçoit exactement `count + 1` on pop le dernier et on
  set `has_more = true`. Pas de `COUNT(*)` additionnel.
- **`next_cursor`** : calculé depuis le dernier item conservé après le pop.
  Si `has_more == false`, `next_cursor == None`.
- **`total`** : renseigné uniquement quand cheap :

| Ressource | `total` source |
|---|---|
| `/blocks` | `indexed_head + 1` (déjà disponible) |
| `/ats`, `/ats/feed` | Table `ats_stats` existante |
| `/accounts`, `/accounts/{a}/ats` | `SELECT COUNT(*)` (tables petites) |
| `/extrinsics`, `/transfers`, `/events` | `None` — trop coûteux sur 10⁷+ rows |

Les endpoints qui renvoyaient un `total` via un sub-endpoint (`/blocks/head`,
`/ats/stats`, `/accounts/{a}/ats/count`) perdent ce rôle d'endpoint-de-comptage
mais restent pour leurs autres usages (monitoring, `/ats/stats` pour
l'overview). `/accounts/{a}/ats/count` est supprimé.

### Framework de filtres (phase 3)

Une struct `Filters` par ressource, mappée vers des clauses SQL dans
`src/data/indexed/queries/*.rs`. Pas d'abstraction générique `enum Filter` —
trop flexible tue le type-safety.

```rust
pub struct BlockFilters {
    pub finalized: Option<bool>,
    pub min_extrinsics: Option<u32>,
    pub spec_version: Option<u32>,
    pub author: Option<String>, // SS58
}

pub struct ExtrinsicFilters {
    pub signed: Option<bool>,
    pub pallet: Option<String>,
    pub call: Option<String>,
    pub status: Option<CallResult>, // Success | Failed
    pub signer: Option<String>,
}

pub struct TransferFilters {
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_amount: Option<String>, // u128 sérialisé en string
}

pub struct EventFilters {
    pub pallet: Option<String>,
    pub variant: Option<String>,
}
```

Query params en snake_case, alignés 1:1 avec les champs de la struct.

### Composable frontend

```ts
// web/app/composables/usePaginatedList.ts
const {
  items,       // Ref<T[]>
  pageInfo,    // Ref<PageInfo>
  pending,
  error,
  filters,     // Ref<Filters>, réactif
  loadMore,    // () => Promise<void>  (appel de nextCursor)
  refresh,
  reset,
} = usePaginatedList<Block>('/networks/:net/blocks', {
  pageSize: 25,
  filters: { finalized: true },
})
```

La clé de cache intègre le cursor et les filtres. Changer un filtre
reset le cursor à `None`. La composable gère la concaténation pour
les modes "infinite scroll" (ATS feed) ou le remplacement pour les modes
"paginé classique" (blocks, extrinsics) via une option `mode: 'append' | 'replace'`.

## Périmètre — fichiers touchés

### Backend — créés

- `src/data/cursor.rs` — newtypes + `FromStr`/`Display` + tests

### Backend — modifiés

- `src/domain.rs` — ajout `Page<T>`, `PageInfo`, `PageRequest`
- `src/data/provider.rs` — signature des méthodes de liste dans `ChainData`
- `src/data/indexed/provider.rs` + `src/data/mock.rs` — impls alignées
- `src/data/indexed/queries/*.rs` — fetch N+1, support des filtres
- `src/data/rpc/provider.rs` — idem (pour le fallback blocks head)
- `src/server/api/blocks.rs`, `extrinsics.rs`, `accounts.rs`, `ats.rs` — handlers
  retournent `Json<Page<T>>`, parsing des filtres et du cursor
- `src/server/api/mod.rs` — `clamp_count` renvoie `400` au lieu de clamper
  silencieusement quand `count > max`

### Backend — supprimés

- `GET /accounts/{address}/ats/count` — le `total` vient maintenant de `page_info`

### Bindings TypeScript (régénérés)

- `bindings/Page*.ts`, `bindings/PageInfo.ts`, `bindings/PageRequest.ts`
- `bindings/*Filters.ts`

### Frontend — créés

- `web/app/composables/usePaginatedList.ts`

### Frontend — modifiés

- `web/app/composables/useNetworkFetch.ts` — inchangé pour les fetches non-paginés
- `web/app/pages/blocks/index.vue` — bascule sur `usePaginatedList`, retire la double-fetch + l'arithmétique BigInt
- `web/app/pages/extrinsics/index.vue` — idem, filtres deviennent server-side
- `web/app/pages/transfers/index.vue` — idem
- `web/app/pages/events/index.vue` — idem
- `web/app/pages/accounts/index.vue` — pas de cursor (top-N), mais bénéficie de la nouvelle enveloppe
- `web/app/pages/ats/index.vue` — bascule sur cursor au lieu d'`offset`
- `web/app/pages/accounts/[address].vue` — idem pour la liste ATS du compte

## Phases

### Phase 1 — Fondations (pas de changement de comportement)

1. Ajouter `Page<T>`, `PageInfo`, `PageRequest` dans `src/domain.rs`, exportés ts-rs.
2. Créer `src/data/cursor.rs` avec les newtypes `BlockCursor`,
   `ExtrinsicCursor`, `TransferCursor`, `EventCursor`, `AtsCursor`,
   `AtsFeedCursor`, chacun avec `FromStr` + `Display` + tests unitaires.
3. Régénérer les bindings TypeScript.
4. **Aucun endpoint ni trait modifié** — la phase est purement additive. Tests verts.

### Phase 2 — Migration des endpoints (breaking)

Endpoint par endpoint, dans cet ordre (du plus douloureux au plus simple) :

1. `/blocks` — supprime la race condition de la page blocks.
2. `/extrinsics` — débloque les filtres server-side.
3. `/transfers`.
4. `/events`.
5. `/ats` + `/ats/feed` — passe d'offset à cursor.
6. `/accounts` + `/accounts/{addr}/ats` — dernier, simple.

Pour chaque endpoint :
- Signature de la méthode dans `ChainData` prend `PageRequest` + `Filters` (Filters vide pour l'instant).
- Query layer SQL passe en fetch N+1.
- Handler retourne `Json<Page<T>>`.
- Mock (`src/data/mock.rs`) aligné.
- Test d'intégration mis à jour.
- Page frontend correspondante migrée immédiatement (pas de drift).

### Phase 3 — Filtres server-side

Ajout progressif des structs `Filters` et du parsing côté handler.
Retrait des filtres client-side dans les pages correspondantes. Priorité :

1. `BlockFilters` : `finalized`, `min_extrinsics`.
2. `ExtrinsicFilters` : `signed`, `status`, `pallet`, `call`.
3. `TransferFilters` : `from`, `to`.
4. `EventFilters` : `pallet`, `variant`.

### Phase 4 — Composable frontend unique

Extraction de `usePaginatedList` à partir du code déjà dupliqué dans
les 6 pages migrées en phase 2. Remplacement du duplicate par le composable.

## Hors scope — à réévaluer plus tard

- **Totaux pour extrinsics/transfers/events** : nécessiterait une table
  `indexer_counters` maintenue par l'indexer. Attendons qu'un use-case UX
  concret le justifie.
- **Cursors bidirectionnels** (`prev_cursor`) : uniquement si on ajoute un
  bouton "précédent" réel. Pour l'instant les pages historiques sont
  append-only, pas besoin.
- **Pagination keyset profonde sur accounts** : si on passe un jour du
  top-N à un listing complet, il faudra un cursor `(balance, address)`.
- **Rate-limiting / quota par cursor** : pas d'usage public encore.
- **Versioning d'API** : on reste sur `/api/v1/`. Si un client externe
  stable apparaît, on considérera `/v2`.

## Critères de succès

- Tous les endpoints de liste retournent `Page<T>` avec la même enveloppe.
- Aucun appel séparé pour récupérer un total côté frontend.
- Plus aucun filtre client-side sur les pages migrées.
- `web/app/pages/blocks/index.vue` ne contient plus de `BigInt` pour la pagination.
- Les 6 pages de liste utilisent `usePaginatedList` — duplication supprimée.
- Tests d'intégration (`tests/*.rs`) verts, e2e (`end2end/`) verts.
