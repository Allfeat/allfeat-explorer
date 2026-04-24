# Plan d'indexation — Architecture pour le scaling

> **Document de travail vivant.** Décrit la migration de l'explorer depuis un
> backend purement RPC/cache vers une architecture indexée (Postgres) avec
> buffer best-blocks en RAM. Chaque phase se termine avec la feature
> correspondante **entièrement** servie par l'indexeur ; le `RpcProvider` reste
> disponible en fallback (via absence de `DATABASE_URL`) jusqu'au hardening
> final.
>
> **Aucune phase ne ferme sans ses tests.** Chaque phase livre
> systématiquement des tests unitaires (logique pure sur fixtures SCALE) **et**
> des tests d'intégration (contre un node dev `ws://127.0.0.1:9944` + une DB
> Postgres fraîche via Docker). Voir §6.

## Suivi global

| #   | Phase                                                          | Priorité | Statut |
|-----|----------------------------------------------------------------|----------|--------|
| 0   | Infra DB (Postgres + sqlx + migrations + `PgPool`)             | 🔴 haute | ⬜     |
| 0.5 | Scaffolding bandeau `IndexingBanner` + endpoint status         | 🟠 moy.  | ⬜     |
| 1   | Live worker — projection `blocks` uniquement                   | 🔴 haute | ⬜     |
| 2   | Backfill runner — chunks parallèles via subxt historic         | 🔴 haute | ✅     |
| 3   | Projection `extrinsics` + tables + queries migrées             | 🔴 haute | ✅     |
| 4   | Projection `events` + `balance_movements` + latest_transfers   | 🟠 moy.  | ✅     |
| 5   | Table `account_balances` + account_by_address + top_accounts   | 🟠 moy.  | ✅     |
| 6   | Projections ATS (registry, versions, feed) + queries ATS       | 🟠 moy.  | ✅     |
| 7   | Hardening — metrics, `/readyz`, retrait RpcProvider en prod    | 🟡 bas.  | ✅     |

Légende : ⬜ à faire · 🟡 en cours · ✅ terminé · ⚠️ bloqué

---

## Fondations

### Topologie de déploiement

Un seul binaire, trois modes d'exécution via flag :

```
allfeat-explorer --mode={all,indexer,server}
```

| Mode      | Rôle                                           | Subxt finalized | Subxt best | Buffer RAM | HTTP  |
|-----------|------------------------------------------------|-----------------|------------|------------|-------|
| `all`     | Dev + early prod (un seul process)             | oui             | oui        | oui        | oui   |
| `indexer` | Worker dédié, aucun port HTTP ouvert           | oui             | non        | non        | non   |
| `server`  | SSR dédié, lit Postgres pour tout le finalisé  | non             | oui        | oui        | oui   |

**Rationale :**

- Zéro dépendance de messagerie supplémentaire (pas de Redis, pas de
  `LISTEN/NOTIFY`). Postgres est la seule source partagée entre replicas.
- Scale-out naturel : 1 `indexer` + N `server`. Chaque replica `server`
  monte **sa propre** souscription `best_head` via subxt — la souscription
  est bon marché, le buffer <1 Mo, subxt multiplexe la connexion
  interne au client `OnlineClient`.
- L'indexeur **n'indexe que finalized** : pas de rollback DB, pas de
  compteurs réversibles. Le non-finalisé vit uniquement dans le buffer RAM.
- Dev reste trivial (`--mode=all`, un docker-compose avec Postgres).
- Aucun changement de schéma nécessaire pour basculer mono → multi process.

### Règles de partage des responsabilités

Principe : **Postgres = tout ce qui est historique, paginé, filtrable,
agrégé. Buffer RAM = tip non-finalisé. RPC direct = rien en prod.**

| Donnée                               | Source                 | Justification |
|--------------------------------------|------------------------|---------------|
| Blocks, extrinsics, events           | Postgres (finalisé) + buffer (best) | Pagination & filtres massifs, reorg isolée au buffer |
| Transfers (historique + latest)      | Postgres + buffer      | Projection triviale des `Balances.*` events |
| Balance_movements (par compte)       | Postgres               | Historique complet depuis le bloc 0 |
| `account_balances` (free/reserved/frozen/nonce) | Postgres   | Maintenu par diff sur events au fil du live worker |
| Top accounts                         | Postgres (`ORDER BY`)  | Dérivé de `account_balances` — toujours cohérent |
| ATS registry + versions + feed       | Postgres               | Même scan coûteux qu'aujourd'hui → devient SQL trivial |
| ATS stats (compteurs)                | Postgres (incréments)  | Maintenus par l'indexer live |
| Head finalized                       | Watch channel subxt    | Déjà wired dans `RpcClient` |
| Best-head / reorg state              | Buffer RAM             | Volatile par nature |
| Metadata / spec_version              | Cache in-memory + `runtime_versions` | Chargé lazy, réutilisé par spec_version |

**Non-objectifs :**

- Afficher une "pending balance" (finalisé + deltas du buffer) sur les pages
  compte. Décision : **uniquement le finalisé** sur les balances, simplifie
  le buffer et supprime toute ambiguïté UX.
- Persister les best-blocks en DB avec une colonne `is_finalized`. Le
  buffer RAM est la seule vue du non-finalisé.
- Purger les events (retention) au MVP. Rediscuter quand `events` dépasse
  ~50M rows.

---

## 1. Schéma Postgres v1

Fichier unique `migrations/001_initial.sql` généré via `sqlx migrate add`.
Principes :

- `BYTEA` partout pour hashes et `AccountId32` — pas de hex string en
  colonne (gain d'espace + index plus compact).
- `NUMERIC(39, 0)` pour les balances (`u128` max = 39 chiffres). Permet
  `+`/`-`/`ORDER BY` natifs sans conversion.
- PK composites sur les tables "par block" — pas de `BIGSERIAL` déguisé.
- Index agressifs sur les colonnes de pagination et de search.

```sql
-- ── Chain state ───────────────────────────────────────────────────────────

CREATE TABLE blocks (
  num             BIGINT PRIMARY KEY,
  hash            BYTEA  NOT NULL UNIQUE,
  parent_hash     BYTEA  NOT NULL,
  state_root      BYTEA  NOT NULL,
  extrinsics_root BYTEA  NOT NULL,
  author          BYTEA,
  timestamp_ms    BIGINT NOT NULL,
  spec_version    INT    NOT NULL,
  extrinsic_count INT    NOT NULL,
  event_count     INT    NOT NULL,
  indexed_at      TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX blocks_author_idx    ON blocks (author, num DESC) WHERE author IS NOT NULL;
CREATE INDEX blocks_timestamp_idx ON blocks (timestamp_ms DESC);

CREATE TABLE extrinsics (
  block_num    BIGINT       NOT NULL REFERENCES blocks(num) ON DELETE CASCADE,
  idx          INT          NOT NULL,
  hash         BYTEA        NOT NULL,
  pallet       TEXT         NOT NULL,
  call         TEXT         NOT NULL,
  signer       BYTEA,
  tip          NUMERIC(39),
  fee          NUMERIC(39)  NOT NULL,
  nonce        BIGINT,
  success      BOOLEAN      NOT NULL,
  error_module TEXT,
  error_name   TEXT,
  args_scale   BYTEA        NOT NULL,
  PRIMARY KEY (block_num, idx)
);
CREATE INDEX extrinsics_signer_idx      ON extrinsics (signer, block_num DESC, idx) WHERE signer IS NOT NULL;
CREATE INDEX extrinsics_hash_idx        ON extrinsics (hash);
CREATE INDEX extrinsics_pallet_call_idx ON extrinsics (pallet, call, block_num DESC);

CREATE TABLE events (
  block_num   BIGINT   NOT NULL REFERENCES blocks(num) ON DELETE CASCADE,
  idx         INT      NOT NULL,
  phase_kind  SMALLINT NOT NULL,        -- 0=ApplyExtrinsic, 1=Finalization, 2=Initialization
  phase_idx   INT,                       -- extrinsic idx si phase_kind=0, NULL sinon
  pallet      TEXT     NOT NULL,
  variant     TEXT     NOT NULL,
  data_scale  BYTEA    NOT NULL,
  PRIMARY KEY (block_num, idx)
);
CREATE INDEX events_pallet_variant_idx ON events (pallet, variant, block_num DESC);
CREATE INDEX events_phase_idx          ON events (block_num, phase_kind, phase_idx);

-- ── Balances ──────────────────────────────────────────────────────────────

CREATE TABLE balance_movements (
  block_num    BIGINT       NOT NULL REFERENCES blocks(num) ON DELETE CASCADE,
  event_idx    INT          NOT NULL,
  account      BYTEA        NOT NULL,
  kind         SMALLINT     NOT NULL,   -- enum MovementKind
  delta        NUMERIC(39)  NOT NULL,   -- signé via NUMERIC (supporte négatif)
  counterparty BYTEA,                    -- pour Transfer (l'autre compte)
  PRIMARY KEY (block_num, event_idx, account)
);
CREATE INDEX balance_movements_account_idx ON balance_movements (account, block_num DESC, event_idx);

-- MovementKind (SMALLINT enum — définition centralisée côté Rust) :
--   0  Transfer (p2p — deux rows, un par compte, signes opposés)
--   1  Deposit
--   2  Withdraw
--   3  Fee                 (TransactionPayment.TransactionFeePaid)
--   4  Slash
--   5  Reserve
--   6  Unreserve
--   7  ReserveRepatriated
--   8  Burn
--   9  Minted
--  10  Frozen
--  11  Thawed
--  12  Locked
--  13  Unlocked

CREATE TABLE account_balances (
  account             BYTEA        PRIMARY KEY,
  free                NUMERIC(39)  NOT NULL DEFAULT 0,
  reserved            NUMERIC(39)  NOT NULL DEFAULT 0,
  frozen              NUMERIC(39)  NOT NULL DEFAULT 0,
  nonce               BIGINT       NOT NULL DEFAULT 0,
  first_seen_block    BIGINT       NOT NULL,
  last_activity_block BIGINT       NOT NULL
);
CREATE INDEX account_balances_total_idx ON account_balances ((free + reserved) DESC);

-- ── ATS (Allfeat-specific) ────────────────────────────────────────────────

CREATE TABLE ats_registry (
  id              BIGINT PRIMARY KEY,
  owner           BYTEA  NOT NULL,
  created_block   BIGINT NOT NULL,
  created_ext_idx INT    NOT NULL,
  version_count   INT    NOT NULL DEFAULT 0
);
CREATE INDEX ats_registry_owner_idx ON ats_registry (owner, id DESC);

CREATE TABLE ats_versions (
  ats_id       BIGINT NOT NULL REFERENCES ats_registry(id) ON DELETE CASCADE,
  version      INT    NOT NULL,
  block_num    BIGINT NOT NULL,
  ext_idx      INT    NOT NULL,
  metadata_cid TEXT,
  PRIMARY KEY (ats_id, version)
);
CREATE INDEX ats_versions_block_idx ON ats_versions (block_num DESC, ats_id);

-- ── Runtime metadata ledger ───────────────────────────────────────────────

CREATE TABLE runtime_versions (
  spec_version     INT    PRIMARY KEY,
  first_seen_block BIGINT NOT NULL,
  metadata_blob    BYTEA  NOT NULL
);

-- ── Indexer state ─────────────────────────────────────────────────────────

CREATE TABLE indexer_cursor (
  stream       TEXT        PRIMARY KEY,   -- 'live' | 'backfill' | …
  last_indexed BIGINT      NOT NULL,
  updated_at   TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE backfill_chunks (
  id          SERIAL      PRIMARY KEY,
  from_block  BIGINT      NOT NULL,
  to_block    BIGINT      NOT NULL,
  status      TEXT        NOT NULL,       -- pending | running | done | failed
  lease_until TIMESTAMPTZ,                -- pour reprise après crash worker
  last_error  TEXT,
  updated_at  TIMESTAMPTZ DEFAULT now(),
  UNIQUE (from_block, to_block)
);
CREATE INDEX backfill_chunks_status_idx ON backfill_chunks (status, lease_until);
```

### Partitioning

Pas à J1. Volumétrie Melodie :
- 3M blocks × 1 row `blocks` = 3M rows → OK.
- 3M blocks × ~10 events = 30M rows `events` → Postgres gère nativement.
- `balance_movements` à peu près pareil (2-5 mouvements moyens par block).

Plan de sortie (quand la DB dépasse ~10M blocks indexés ou ~100M events) :
`PARTITION BY RANGE (block_num)` avec des buckets de 500 000, migration via
`pg_partman` ou script maison.

---

## 2. Architecture de l'indexeur

Nouveau module `src/indexer/` :

```
src/indexer/
├── mod.rs            # entry point, orchestration des workers
├── live.rs           # consommateur du stream finalized
├── backfill.rs       # range workers parallèles
├── buffer.rs         # PendingBuffer (best-blocks en RAM)
├── sink.rs           # INSERT transactions — unique endroit qui touche Postgres
├── projections/
│   ├── mod.rs
│   ├── blocks.rs     # map_block → row
│   ├── extrinsics.rs
│   ├── events.rs
│   ├── balances.rs   # projection des Balances.* events → balance_movements + delta
│   └── ats.rs
└── metadata.rs       # cache runtime_versions, lookup par spec_version
```

### Live worker (`live.rs`)

Responsabilités :

1. Souscrit au stream finalized via `client.blocks().subscribe_finalized()`.
2. Pour chaque block reçu : ouvre une `sqlx::Transaction`, appelle toutes
   les projections en séquence, insère les rows, met à jour
   `account_balances` via UPSERT + delta, bump `indexer_cursor.live`,
   `COMMIT`.
3. **Un seul transaction par block.** Si le process meurt mid-block, la
   transaction rollback naturellement. Pas de demi-block en DB.
4. **Idempotent** via `ON CONFLICT (block_num, idx) DO NOTHING` sur chaque
   insert. Rejouer un block déjà indexé est un no-op.
5. Publie au buffer RAM un event `Finalized{block_num}` pour que le buffer
   drop les entries correspondantes.

Pseudo-code :

```rust
async fn index_finalized_block(
    at: &OnlineClientAtBlock<SubstrateConfig>,
    pool: &PgPool,
    meta: &MetadataCache,
    block_num: u64,
) -> DataResult<()> {
    let mut tx = pool.begin().await?;

    let block_row      = projections::blocks::map(at, block_num).await?;
    let timestamp_ms   = block_row.timestamp_ms;
    let extrinsic_rows = projections::extrinsics::map(at, timestamp_ms).await?;
    let event_rows     = projections::events::map(at).await?;
    let balance_rows   = projections::balances::project(&event_rows, &extrinsic_rows)?;
    let ats_rows       = projections::ats::project(&event_rows, &extrinsic_rows)?;

    sink::insert_block(&mut tx, &block_row).await?;
    sink::insert_extrinsics(&mut tx, &extrinsic_rows).await?;
    sink::insert_events(&mut tx, &event_rows).await?;
    sink::insert_balance_movements(&mut tx, &balance_rows).await?;
    sink::apply_balance_deltas(&mut tx, &balance_rows, block_num).await?;
    sink::apply_ats(&mut tx, &ats_rows).await?;
    sink::bump_cursor(&mut tx, "live", block_num).await?;

    tx.commit().await?;
    Ok(())
}
```

### Backfill workers (`backfill.rs`)

1. **Au boot**, calcule les gaps entre `genesis = 0` et
   `finalized_head_rx.borrow()` :
   ```sql
   WITH series AS (SELECT generate_series(0, $1) AS num)
   INSERT INTO backfill_chunks (from_block, to_block, status)
   SELECT min(gap), max(gap), 'pending'
   FROM (
     SELECT num AS gap,
            num - ROW_NUMBER() OVER (ORDER BY num) AS grp
     FROM (SELECT num FROM series EXCEPT SELECT num FROM blocks) g
   ) bucketed
   GROUP BY grp
   HAVING max(gap) - min(gap) < 1000;  -- découpage max 1k blocks par chunk
   ```
   Simplification : à froid (DB vide), une seule query insère `⌈N/1000⌉`
   chunks contigus.

2. **Workers** : N tâches tokio (configurable, défaut 4). Chaque worker
   boucle :
   ```
   claim un chunk via UPDATE ... WHERE status='pending' OR
       (status='running' AND lease_until < now()) RETURNING
   pour block_num in from..to:
       fetch block at hash via subxt .blocks().at(...)
       même pipeline que live_worker
   UPDATE status='done'
   ```

3. **Métadonnées par spec_version** : `projections/*` reçoivent le
   `OnlineClientAtBlock` qui porte déjà le metadata correct (subxt le
   fetch automatiquement quand on pointe un block passé). Le cache
   `MetadataCache` (voir §2.4) mémoïse `spec_version → Metadata` pour
   éviter de re-télécharger à chaque block.

4. **Throughput attendu** : 4 workers × ~100ms/block sur archive node →
   ~40 blocks/s. Melodie (3M) ≈ 20h. Allfeat mainnet (400k) ≈ 2h40.
   Acceptable en background pendant que le live worker tourne.

5. **Reprise** : un worker qui meurt laisse son chunk en `status='running'`
   avec `lease_until = now() + 5min`. Un autre worker le reprend après
   expiration du lease.

### Cache metadata (`metadata.rs`)

```rust
pub struct MetadataCache {
    by_spec: DashMap<u32, Arc<Metadata>>,
    pool: PgPool,
}

impl MetadataCache {
    pub async fn get_or_fetch(
        &self,
        at: &OnlineClientAtBlock<SubstrateConfig>,
    ) -> DataResult<Arc<Metadata>> {
        let spec = at.runtime_version().spec_version;
        if let Some(m) = self.by_spec.get(&spec) { return Ok(m.clone()); }

        // Pas en RAM : essayer la DB d'abord.
        if let Some(blob) = load_from_db(&self.pool, spec).await? {
            let meta = Arc::new(Metadata::decode(&mut &blob[..])?);
            self.by_spec.insert(spec, meta.clone());
            return Ok(meta);
        }

        // Première rencontre de ce spec_version : fetch via subxt, store.
        let meta = at.metadata(); // déjà chargé par subxt pour ce block
        let blob = meta.encode();
        store_in_db(&self.pool, spec, at.block_number()?, &blob).await?;
        let arc = Arc::new((*meta).clone());
        self.by_spec.insert(spec, arc.clone());
        Ok(arc)
    }
}
```

Les projections consultent ce cache quand elles ont besoin d'info statique
sur les types (rare : la plupart des décodages passent par `at` directement,
qui porte son propre metadata).

---

## 3. Best-block buffer (`buffer.rs`)

### Structure

```rust
pub struct PendingBuffer {
    finalized_head: u64,
    blocks: VecDeque<IndexedBlock>,   // ascendant par num, cap 64
    by_hash: HashMap<[u8; 32], usize>,
    by_xt_hash: HashMap<[u8; 32], (usize, u32)>,
    events_tx: broadcast::Sender<BufferEvent>,
}

pub enum BufferEvent {
    BlockAppended(IndexedBlock),
    Finalized { block_num: u64 },
    Reorged   { dropped_hashes: Vec<[u8; 32]> },
}
```

### Contenu par entrée

```rust
pub struct IndexedBlock {
    pub header:      BlockRow,
    pub extrinsics:  Vec<ExtrinsicRow>,
    pub events:      Vec<EventRow>,
    pub transfers:   Vec<Transfer>,   // projection des Balances.Transfer
    // PAS de balance_movements — les balances sont finalisé-only.
}
```

### API publique

```rust
impl PendingBuffer {
    pub fn append_best(&mut self, block: IndexedBlock);
    pub fn advance_finalized(&mut self, up_to: u64);
    pub fn best_head(&self) -> Option<u64>;
    pub fn iter_above(&self, num: u64) -> impl Iterator<Item = &IndexedBlock>;
    pub fn lookup_block(&self, hash: &[u8; 32]) -> Option<&IndexedBlock>;
    pub fn lookup_extrinsic(&self, hash: &[u8; 32]) -> Option<(&IndexedBlock, u32)>;
    pub fn subscribe(&self) -> broadcast::Receiver<BufferEvent>;
}
```

### Reorg policy (pragmatique)

Sur chaque best tick :

1. `parent == buffer.tail().hash` → append simple.
2. Sinon → **re-fetch** `[finalized+1 … new.number]` en entier via subxt,
   replace le buffer complet, émettre `Reorged{dropped}` avec les hashes
   disparus.

Justification : 10 blocks × projection en mémoire ≈ <500ms, négligeable
face à la fréquence des reorgs (<1/heure en pratique). Si profiling montre
un coût sensible, passer à un walk-back incrémental dans un second temps.

### Source des best-blocks

Souscription subxt `client.blocks().subscribe_best()` spawned depuis
`RpcClient::ensure_watcher_running` (nouvelle tâche soeur du watcher
finalized existant). Les deux watchers partagent le même slot `inner`
(`Arc<RwLock<Option<AllfeatClient>>>`) et la même politique de reconnect.

### Fan-out vers le live WS

Le serveur WS actuel (`src/live/server.rs`) remplace sa source (actuellement
`RpcClient::subscribe_blocks`) par `PendingBuffer::subscribe()`. Le merge
logic (`src/live/merge.rs`) gagne deux cas :

- `BufferEvent::BlockAppended(b)` avec `b.is_finalized = false` →
  envoie au client avec badge "pending".
- `BufferEvent::Finalized{block_num}` → envoie un petit event "finalize"
  que le client utilise pour flipper le badge de la ligne correspondante.
- `BufferEvent::Reorged{dropped_hashes}` → envoie un event "remove" pour
  que le client retire les lignes.

Taille wire : un event `Finalized` fait ~16 octets. Un `BlockAppended`
reste sous 10 Ko (extrinsics + events décodés). Aucun changement d'ordre
de grandeur par rapport au protocole actuel.

---

## 4. Query layer — `IndexedProvider`

Nouveau module `src/data/indexed/` :

```
src/data/indexed/
├── mod.rs
├── provider.rs      # impl ChainData
└── queries/         # SQL préparés par domaine
    ├── blocks.rs
    ├── extrinsics.rs
    ├── transfers.rs
    ├── accounts.rs
    └── ats.rs
```

### Structure

```rust
pub struct IndexedProvider {
    pool: PgPool,
    buffer: Option<Arc<RwLock<PendingBuffer>>>,   // None en mode `indexer`
    finalized_head_rx: watch::Receiver<Option<u64>>,
    // Optionnel : fallback RPC pour les méthodes non encore migrées
    rpc_fallback: Option<Arc<RpcProvider>>,
}
```

### Routage par méthode

| Méthode                  | Source principale               | Buffer ? | Notes |
|--------------------------|---------------------------------|----------|-------|
| `head_block`             | `finalized_head_rx`             | non      | L'UI affiche le tip finalisé ; `best_head` exposé séparément pour le WS |
| `block_by_number`        | DB si `num ≤ finalized`, buffer sinon | oui | Lookup direct |
| `latest_blocks(count, from)` | Union `buffer.iter_above() ∪ DB` | oui | Items avec `is_finalized: bool` |
| `extrinsics_in_block`    | DB ou buffer selon `num`        | oui      | |
| `latest_extrinsics`      | Union buffer + DB               | oui      | `is_finalized` flag par item |
| `extrinsic_by_id`        | Buffer first (hash lookup) → DB | oui      | Split parse id `"block-idx"` vs hash |
| `latest_transfers`       | Union buffer + DB               | oui      | Buffer projette les `Balances.Transfer` events |
| `account_by_address`     | DB (`account_balances`)         | **non**  | Balance = finalisé only |
| `top_accounts`           | DB (`ORDER BY (free+reserved)`) | **non**  | |
| `ats_stats`              | DB (counters)                   | **non**  | |
| `ats_by_index`           | DB                              | **non**  | |
| `ats_list`               | DB                              | **non**  | |
| `ats_version_feed`       | DB                              | **non**  | |
| `account_ats`            | DB                              | **non**  | |
| `account_ats_count`      | DB                              | **non**  | |
| `subscribe_blocks`       | Buffer broadcast                | oui      | |
| `subscribe_transfers`    | Buffer broadcast                | oui      | |
| `subscribe_ats_feed`     | DB → poll (voir Phase 6) ou buffer | —      | À trancher Phase 6 |

### Pas de `HybridProvider` séparé

Le routage vit **dans** `IndexedProvider`. Pas de trait supplémentaire
exposée aux pages. Pendant la migration, `IndexedProvider` peut porter
un `rpc_fallback: Option<Arc<RpcProvider>>` pour les méthodes pas encore
migrées — retiré à la phase 7.

---

## 5. Frontend — bandeau `IndexingBanner`

### Endpoint d'état

Nouveau server function dans `src/server/fns/health.rs` :

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct IndexerStatus {
    pub state: IndexerState,
    pub finalized_head:   Option<u64>,
    pub indexer_head:     Option<u64>,
    pub live_lag_blocks:  Option<u64>,
    pub backfill_done:    u64,
    pub backfill_total:   u64,
    pub backfill_pct:     f32,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum IndexerState {
    Healthy,       // lag ≤ 2 blocks ET backfill_pct = 100
    CatchingUp,    // lag > 2 blocks
    Backfilling,   // backfill_pct < 100
    Offline,       // cursor stale > 60s OU DB unreachable
}

#[server]
pub async fn get_indexer_status() -> Result<IndexerStatus, ServerFnError> { ... }
```

Implémentation :
- `indexer_head` = `SELECT last_indexed FROM indexer_cursor WHERE stream='live'`
- `finalized_head` = `finalized_head_rx.borrow()`
- `backfill_done` = `SELECT COUNT(*) FROM blocks`
- `backfill_total` = `finalized_head + 1`
- Cache côté serveur (`Arc<watch::Receiver<IndexerStatus>>`) rafraîchi
  toutes les 2s par une tâche dédiée → `get_indexer_status()` est un
  `borrow()` trivial.

### Composant Leptos

Nouveau fichier `src/ui/banner.rs`, monté dans `src/app/mod.rs` juste
au-dessus de `<Header>` :

```rust
#[component]
pub fn IndexingBanner() -> impl IntoView {
    let status = create_local_resource(
        || (),
        |_| async { get_indexer_status().await.ok() },
    );
    // refresh toutes les 5s côté client
    // ...
}
```

### Copy & états visuels

| État          | Couleur | Copy |
|---------------|---------|------|
| `Healthy`     | —       | (bandeau caché, pas de shift de layout) |
| `Backfilling` | jaune   | "Historical data is still being indexed ({pct:.1}% complete, ~{eta} remaining). Search results, account history and pagination into older blocks may be partial. Live data is unaffected." |
| `CatchingUp`  | orange  | "Indexing is {lag} blocks behind the chain tip. Recent blocks may appear with a short delay." |
| `Offline`     | rouge   | "The indexer is currently offline. Data shown below may be out of date." |

**ETA backfill** : extrapolation linéaire sur les 60 dernières secondes de
progression (`Δ(backfill_done) / 60s`). Précision acceptable pour un
ordre de grandeur.

**Style** : position sticky sous le header, ~32px, non-dismissible (se
masque seul quand `state = Healthy`).

### Phasing du bandeau

- **Phase 0.5** : scaffold composant + endpoint stub (toujours `Healthy`).
  Valide le rendu et le layout shift avant d'avoir l'indexeur.
- **Phase 1** : endpoint réel câblé → `Healthy` / `CatchingUp` fonctionnels
  dès que le live worker tourne.
- **Phase 2** : état `Backfilling` actif dès que le backfill démarre.
  C'est la principale UX pendant les ~20h d'indexation initiale.
- **Phase 7** : état `Offline` (détection cursor stale), hardening ETA,
  revue UX/copy.

---

## 6. Stratégie de tests

> **Règle :** chaque phase livre **à la fois** des tests unitaires (logique
> pure, aucune I/O) **et** des tests d'intégration (node + Postgres réels).
> La CI exécute les deux à chaque PR. Aucune phase ne ferme sans ses tests.

### Infrastructure

**Node de dev** — toujours accessible à `ws://127.0.0.1:9944` sur la machine
de dev et dans CI. Les tests d'intégration subxt pointent par défaut vers
cette URL ; surcharge via `ALLFEAT_TEST_NODE_URL`. On valide contre un
**vrai runtime Allfeat** — les projections sont sensibles aux types
pallet-spécifiques, un mock masquerait les dérives.

**Postgres de test** — toujours une DB **fraîche** par run, via Docker. Deux
modes :

- **Local dev** : `docker-compose up -d postgres-test` (service dédié,
  port distinct de la DB applicative, `tmpfs` pour data, zéro persistance).
- **CI** : service Postgres 16 déclaré dans le job, `tmpfs` pour la data
  dir, killed en fin de job.

Chaque test d'intégration isole son état :

1. Ouvre une connexion admin au Postgres de test.
2. `CREATE DATABASE test_<pid>_<rand>` — isolation par test.
3. Run toutes les migrations (`sqlx::migrate!()`).
4. Exécute le scénario.
5. `DROP DATABASE` en teardown (via `impl Drop for TestDb`).

Helpers partagés dans `tests/common/mod.rs` :

```rust
pub async fn fresh_db() -> TestDb;                              // DB jetable + pool
pub async fn dev_node_client() -> AllfeatClient;                // ws://127.0.0.1:9944
pub async fn wait_for_block(pool: &PgPool, num: u64, timeout: Duration);
pub async fn wait_for_cursor(pool: &PgPool, stream: &str, at_least: u64, timeout: Duration);
```

### Tests unitaires

**Portée** : projections, buffer, merge logic, routage `IndexedProvider`,
parsing helpers.

**Règles** :

- **Zéro I/O réseau, zéro DB.** Les inputs sont des fixtures SCALE
  extraites une fois depuis le node dev et stockées dans
  `tests/fixtures/` (format `.scale` binaire + manifest JSON :
  block number, spec_version, description humaine).
- Tests en `#[test]` dans des `mod tests` inline (même fichier que le
  code).
- Full suite <10 s. Tout test unitaire qui dépasse 100 ms est suspect.

### Tests d'intégration

**Portée** : chemin complet node → indexer → DB → server fn → (UI si
applicable).

**Règles** :

- DB **fraîche** par test, **aucun** partage d'état.
- Connexion au node dev `ws://127.0.0.1:9944` (surcharge via env var).
- Une fonction `#[tokio::test]` = un scénario, fichier top-level
  `tests/<domaine>.rs`.
- Timeouts explicites, jamais >30 s ; attendre un block finalisé budget
  15 s max.

### Stratégie anti-flakes

- Zéro `sleep()` dur : `wait_for_block` / `wait_for_cursor` avec polling
  court + timeout explicite.
- Chaque test auto-contenu (DB fraîche, producer subxt propre).
- Fixtures committées, jamais re-générées en CI.
- Échec sporadique interdit : pas de retry ; marquer `#[ignore]` avec
  ticket ouvert obligatoire.

### Pipeline CI

Deux jobs en parallèle :

- `test-unit` : `cargo test --lib --features ssr` — aucun service
  externe, budget <2 min.
- `test-integration` : services `postgres` (port 54329) +
  `allfeat-dev-node` (container démarré au début du job, healthcheck sur
  `ws://127.0.0.1:9944`), `cargo test --test '*' --features ssr` —
  budget 15 min.

`cargo sqlx prepare --check` vérifie que `.sqlx/` est à jour.

---

## 7. Phasage incrémental

Chaque phase se termine avec la feature **entièrement** servie par
`IndexedProvider` **et** ses tests unitaires + intégration verts. Le
fallback RPC reste activable par flag jusqu'à la phase 7.

### Phase 0 — Infra DB

**Livrables :**
- `Cargo.toml` : ajouter `sqlx` (features `runtime-tokio`, `postgres`,
  `macros`, `migrate`, `chrono`), `sqlx-cli` en dev-dep.
- `migrations/001_initial.sql` (schéma complet ci-dessus).
- `.sqlx/` committé pour CI offline (`cargo sqlx prepare --workspace`).
- `src/server/config.rs` : lecture `DATABASE_URL`.
- `src/server/state.rs` : `PgPool` dans `ServerState`.
- `docker-compose.yml` (racine) : service Postgres 16 + volume nommé.
- CI : job `sqlx migrate run` contre Postgres service.

**Tests :**
- Unitaires : `#[test]` sanity qui instancie `sqlx::migrate!()` pour
  garantir que le macro compile (aucun code métier à tester à ce stade).
- Intégration :
  - `tests/migrations.rs::runs_clean_on_fresh_db` : applique toutes les
    migrations sur `fresh_db()`, assert présence des 11 tables et des
    index critiques (`blocks_hash_idx`, `extrinsics_signer_idx`,
    `extrinsics_hash_idx`, `account_balances_total_idx`).
  - `tests/migrations.rs::idempotent_rerun` : relance `migrate!()` sur
    la même DB sans erreur ni diff.

**Critère d'achèvement :**
- `cargo sqlx migrate run` passe sur une DB fresh.
- `cargo test --features ssr` reste vert (aucun code métier ne consomme
  encore `PgPool`).

### Phase 0.5 — Bandeau scaffold

**Livrables :**
- `src/ui/banner.rs` + style SCSS.
- `src/server/fns/health.rs::get_indexer_status` renvoyant `Healthy`
  constant.
- Montage dans `src/app/mod.rs`.

**Tests :**
- Unitaires : `IndexingBanner` rendu pour chaque `IndexerState` produit
  le HTML attendu (SSR via `leptos::ssr::render_to_string`), assertions
  sur le texte affiché et la classe CSS d'état.
- Intégration : `tests/banner.rs::endpoint_returns_healthy_stub` hit
  `get_indexer_status()`, assert `state = Healthy` et payload conforme
  au contrat.

**Critère :** visuel validé sur toutes les pages, pas de layout shift.

### Phase 1 — Live worker (blocks)

**Livrables :**
- `src/indexer/` squelette complet (modules vides pour les autres
  projections).
- `src/indexer/live.rs` consomme finalized stream, n'indexe que la table
  `blocks`.
- `src/indexer/mod.rs` : `spawn(mode)` décidant quoi démarrer selon
  `--mode`.
- `IndexedProvider` partiel : `head_block`, `block_by_number`,
  `latest_blocks` servent depuis DB.
- Endpoint `get_indexer_status` câble `finalized_head_rx` + cursor.

**Tests :**
- Unitaires : `projections::blocks::map` sur 3 fixtures (block avec
  author résolu via Aura slot, block sans author, deux `spec_version`
  distincts pour couvrir les variations de header).
- Intégration :
  - `tests/indexer_live.rs::indexes_new_finalized_block` : spawn le live
    worker sur `fresh_db()` + `dev_node_client()`,
    `wait_for_block(pool, head+1, 15s)`, assert la row `blocks.num =
    head+1` avec hash non nul.
  - `tests/indexer_live.rs::resumes_from_cursor_after_restart` :
    stop/start du worker sans reset DB, assert zéro conflit PK grâce
    au `ON CONFLICT DO NOTHING`.
  - `tests/banner.rs::reflects_live_lag` : injecte un cursor
    artificiellement en retard, `state = CatchingUp` avec
    `live_lag_blocks > 2`.

**Critère :** démarrer l'explorer avec `--mode=all`, attendre 1 block,
vérifier que la page `/blocks` affiche l'entrée issue de la DB. Lag
affiché dans le bandeau.

### Phase 2 — Backfill runner

**Livrables :**
- `src/indexer/backfill.rs` + table `backfill_chunks` déjà en Phase 0.
- Config : `INDEXER_BACKFILL_CONCURRENCY=4` (env var).
- Bandeau passe à `Backfilling` avec `%` réel.

**Tests :**
- Unitaires : `backfill::split_gaps_into_chunks` sur ranges synthétiques
  (full gap, partial gap, no gap, chunk size exactement à la boundary de
  1000).
- Intégration :
  - `tests/backfill.rs::indexes_short_range` : backfill explicite sur
    `[head-100, head]`, `COUNT(*) = 101`, tous les hashes uniques,
    aucun gap dans `blocks.num`.
  - `tests/backfill.rs::parallel_workers_no_duplicate` : 4 workers
    concurrents sur la même DB fraîche, zéro conflit PK, nombre total
    de rows attendu.
  - `tests/backfill.rs::resumes_after_worker_crash` : drop le handle
    d'un worker mid-chunk, simuler l'expiration du lease, vérifier
    qu'un autre worker claim le chunk et le complète.
  - `tests/banner.rs::shows_backfilling_state` : pendant un backfill
    actif, `state = Backfilling` et `backfill_pct` monotonement
    croissant sur 3 samples espacés.

**Critère :** backfill complet de Melodie testnet atteint en
<48h avec 4 workers. `SELECT COUNT(*) FROM blocks = finalized_head + 1`.

### Phase 3 — Extrinsics

**Livrables :**
- `src/indexer/projections/extrinsics.rs`.
- `IndexedProvider::extrinsics_in_block`, `latest_extrinsics`,
  `extrinsic_by_id` servent depuis DB.
- Buffer minimal : extrinsics par block indexés en RAM pour le lookup
  tip.

**Tests :**
- Unitaires :
  - `projections::extrinsics::map` sur fixtures couvrant : signed,
    unsigned, inherent (`timestamp.set`), failed avec
    `error_module`/`error_name`, tip non nul, `fee` non nul.
  - Parse/format `extrinsic_id` (`"block-idx"` ↔ `(u64, u32)`) + rejet
    strict des formats invalides (empty, trailing chars, non-numeric).
- Intégration :
  - `tests/extrinsics.rs::lookup_by_hash_from_db` : backfill un range
    connu, lookup par hash (bytes) retourne la row correcte.
  - `tests/extrinsics.rs::page_extrinsic_detail_smoke` : request SSR
    `/extrinsic/<id>`, assert HTML contient signer, pallet, call, fee
    et statut (success/failed).
  - `tests/extrinsics.rs::buffer_lookup_tip` : injecte un best block
    avec un extrinsic dans le buffer, `extrinsic_by_id` resolve depuis
    le buffer avec flag `is_finalized = false`.

**Critère :** search par hash d'extrinsic resolve depuis DB. Page
`/extrinsics/<id>` opérationnelle.

### Phase 4 — Events + transfers

**Livrables :**
- `src/indexer/projections/events.rs`, `balances.rs`.
- Tables `events`, `balance_movements` remplies.
- `IndexedProvider::latest_transfers`, `subscribe_transfers` depuis
  buffer + DB.

**Tests :**
- Unitaires :
  - `projections::events::map` sur fixtures couvrant les 3 phases
    (ApplyExtrinsic, Finalization, Initialization).
  - `projections::balances::project` : une fixture par `MovementKind`
    (Transfer émet 2 rows symétriques avec deltas opposés,
    Deposit/Withdraw/Fee/Slash/Reserve/Unreserve/ReserveRepatriated/
    Burn/Minted/Frozen/Thawed/Locked/Unlocked : 1 row chacun, signe
    correct, `counterparty` renseigné uniquement sur Transfer).
- Intégration :
  - `tests/transfers.rs::history_for_known_account` : backfill un range
    contenant un transfer ciblé (block number + compte connus du
    testnet), `latest_transfers` pour ce compte retourne la row
    attendue.
  - `tests/transfers.rs::live_stream_emits_on_new_block` : subscribe au
    canal buffer, émet un transfer via extrinsic sur le node dev,
    reçoit l'event dans les 15s.

**Critère :** historique de transfers pour compte X correct (comparer
avec un scan RPC direct sur 100 comptes témoins).

### Phase 5 — Balances & accounts

**Livrables :**
- UPSERT `account_balances` dans `live.rs` (et replay pendant backfill).
- `IndexedProvider::account_by_address`, `top_accounts` depuis DB.

**Tests :**
- Unitaires : UPSERT delta sur fixtures synthétiques — start balance
  X, apply séquence A/B/C de kinds, assert final balance. Inclut les
  cas Reserved puis Unreserve (round-trip neutre), Slashed (decrease
  stricte), Frozen/Thawed (ne bouge pas `free`).
- Intégration :
  - `tests/accounts.rs::balance_matches_system_account` : pour 20
    comptes observés via RPC `System::Account` au finalized head,
    compare avec `account_by_address` — égalité stricte sur
    free/reserved/frozen/nonce.
  - `tests/accounts.rs::top_accounts_ordered` : `top_accounts(N)`
    renvoie des rows ordonnées par `(free+reserved) DESC`, cohérent
    avec un scan RPC.
  - `tests/accounts.rs::nonce_advances_on_signed_extrinsic` : envoi
    d'un extrinsic signé depuis un compte test, nonce DB = nonce RPC
    après finalisation.

**Critère :** page compte affiche free/reserved/frozen/nonce cohérents
avec `System::Account` au head finalisé (spot-check sur 50 comptes).
Top accounts ordonné correctement.

### Phase 6 — ATS

**Livrables :**
- `src/indexer/projections/ats.rs`.
- Tables `ats_registry`, `ats_versions` remplies ; compteurs dans
  `ats_registry.version_count` maintenus par UPSERT.
- Toutes les méthodes `ats_*` servies depuis DB.

**Tests :**
- Unitaires : `projections::ats::project` sur fixtures d'events
  `Ats.*` (création d'un ATS, nouvelle version, transfert d'ownership
  si applicable au pallet).
- Intégration :
  - `tests/ats.rs::registry_matches_rpc_scan` : pour chaque ATS
    rencontré via scan RPC (ancien provider, utilisé comme oracle),
    `ats_by_index` retourne la même entrée (owner, created_block,
    version_count).
  - `tests/ats.rs::version_feed_pagination` : pagination ascendante +
    descendante renvoient des pages disjointes, concaténation ==
    feed complet (pas de duplicate, pas de trou).
  - `tests/ats.rs::stats_counters_consistent` : `ats_stats` agrège
    correctement `ats_registry.version_count` après N events — pas de
    drift entre counter incrémental et COUNT(*).

**Critère :** pages ATS identiques en rendu vs `RpcProvider` (snapshot
Playwright stable).

### Phase 7 — Hardening

**Livrables :**
- Métriques Prom (voir §8).
- `/readyz` retourne 503 tant que lag > 30s.
- État `Offline` du bandeau détecté via cursor stale > 60s.
- `--mode=server` en prod refuse de démarrer si `DATABASE_URL` absent.
- `RpcProvider` n'est plus jamais instancié en mode prod (uniquement
  dev / `--mode=all` sans DB pour onboarding rapide).
- Runbook ops (backup Postgres, reset backfill, re-indexer un range).

**Tests :**
- Unitaires : détection `Offline` à partir d'un `indexer_cursor.updated_at`
  stale (fixture avec timestamp > 60s) ; parsing `--mode` (valeurs
  valides + rejet des invalides).
- Intégration :
  - `tests/readyz.rs::503_when_lag_exceeds_threshold` : injecte un lag
    artificiel >30s via cursor, assert HTTP 503 sur `/readyz`.
  - `tests/readyz.rs::200_when_healthy` : live worker à jour, assert
    HTTP 200.
  - `tests/ops.rs::server_mode_refuses_without_db` : lance le binaire
    en `--mode=server` sans `DATABASE_URL`, assert exit code non-zéro
    et message d'erreur explicite sur stderr.
  - `tests/metrics.rs::exports_prometheus_text` : hit `/metrics`,
    assert présence des counters/gauges clés
    (`indexer_blocks_indexed_total`, `indexer_lag_seconds`,
    `buffer_size`, `indexer_reorg_total`).

**Critère :** déploiement multi-replica validé en staging, monitoring
branché, doc ops relue.

---

## 8. Observability

### Tracing

- Span par block indexé : `block_num, spec_version, xt_count,
  event_count, duration_ms, stream={live,backfill}`.
- Span par chunk backfill : `from, to, workers, duration_ms,
  throughput_blocks_per_s`.

### Métriques Prometheus

Exposées sur `/metrics` (nouveau endpoint dans `src/server/health.rs`) :

| Nom                                    | Type    | Labels              |
|----------------------------------------|---------|---------------------|
| `indexer_blocks_indexed_total`         | counter | `stream`            |
| `indexer_lag_seconds`                  | gauge   | —                   |
| `indexer_lag_blocks`                   | gauge   | —                   |
| `indexer_backfill_remaining_blocks`    | gauge   | —                   |
| `indexer_backfill_throughput_bps`      | gauge   | —                   |
| `indexer_reorg_total`                  | counter | —                   |
| `indexer_projection_duration_seconds`  | histo   | `projection`        |
| `buffer_size`                          | gauge   | —                   |
| `buffer_best_head`                     | gauge   | —                   |
| `buffer_finalized_head`                | gauge   | —                   |
| `db_query_duration_seconds`            | histo   | `query`             |

### Healthz / readyz

- `/healthz` : 200 si le process vit (toujours).
- `/readyz` :
  - 503 si `PgPool::acquire()` échoue.
  - 503 si `indexer_lag_seconds > 30` (configurable).
  - Opt-in : 503 tant que `backfill_remaining > 0` — sinon le boot
    initial bloque readyz plusieurs heures ; par défaut, off.

---

## 9. Décisions différées

1. **Retention / volumétrie** — aucune politique au MVP. À reprendre quand
   `events` dépasse ~50M rows (estimation : >4M blocks sur Melodie).
   Plan de sortie : partitioning par range `block_num` via `pg_partman`.

2. **`subscribe_ats_feed` via buffer** — pour l'instant le buffer ne
   projette pas les events ATS. À trancher en Phase 6 : soit on étend le
   buffer (cohérent avec blocks/transfers), soit on sert via poll DB
   (simpler, acceptable pour un feed bas-fréquence).

3. **Multi-network indexing** — ~~une seule DB par network, ou une DB
   multi-tenant avec `network_id` en colonne ?~~ **Tranché : DB
   multi-tenant.** Toutes les tables indexées portent une colonne
   `network_id TEXT NOT NULL` en tête de leur PK / UNIQUE, un seul
   binaire spawn un set de workers (live + backfill) par network
   activé au boot (entrée de `src/network.rs::NETWORKS` dont
   `RPC_ENDPOINT_<ID>` est défini ; les autres sont complètement
   désactivées, pas indexées et masquées dans l'UI). `IndexedProvider`
   route par `ctx.spec.id` : network indexé → DB, autre → RPC fallback.
   Un seul `DATABASE_URL` pour toute la flotte.

4. **Balance snapshots reconciliation** — projection event-driven est
   complète mais peut dériver sur un bug. Plan B : snapshot périodique de
   `System::Account` (toutes les 10k blocks) et diff contre la DB en
   background, alerte sur divergence. Outil de monitoring, pas de
   correction auto. Pas nécessaire au MVP.

---

## 10. Glossaire rapide

| Terme             | Définition |
|-------------------|------------|
| Live worker       | Tâche tokio qui consomme le stream finalized subxt et écrit en DB, block par block |
| Backfill worker   | Tâche tokio qui claim un chunk `backfill_chunks` et l'indexe via subxt historic |
| Best-block buffer | Ring buffer en RAM couvrant `[finalized+1 … best]`, vit dans le process `server`/`all` |
| Finalized-only    | L'indexeur **n'écrit** en DB que des blocks finalisés. Pas de rollback, pas de colonne `is_finalized` en DB |
| Projection        | Fonction `(Block, Events, Extrinsics) → Vec<Row>` qui ne touche pas la DB — `sink.rs` fait les INSERT |
| Cursor            | Ligne `indexer_cursor` qui tracke le dernier block indexé par stream (`live` | `backfill`) |
| Reorg             | Nouveau best-head dont le parent ne correspond pas au tail du buffer → re-fetch `[finalized+1, new]` en entier |

---

## 11. Références

- Schéma courant `ChainData` : `src/data/provider.rs`
- Client RPC actuel : `src/data/rpc/client.rs` (watch supervisor, broadcast
  channels, policy de reconnect — à réutiliser pour la souscription best)
- Mappers existants : `src/data/rpc/mappers/` (deviennent `projections/`
  après migration — même logique, sink différent)
- Live WebSocket : `src/live/` (serveur, merge logic, protocole)
- Subxt examples : [paritytech/subxt `subxt/examples/`][subxt-examples]
  — notamment `historic_blocks.rs` et `blocks.rs` (finalized vs best
  subscription)

[subxt-examples]: https://github.com/paritytech/subxt/tree/master/subxt/examples
