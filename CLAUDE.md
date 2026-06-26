# CLAUDE.md

Guide pour travailler dans ce dépôt. À lire avant toute modification.

## Projet

Système d'intelligence pour le sniping de memecoins Solana, écrit en Rust.
Le dépôt est en **Phase 1** : il contient la **couche de persistance SQLite** plus
**deux composants de traitement** — l'**ingestor** (écrit les faits terrain) et
l'**analyseur** (calcule clusters, profils, scores). La logique de trading (le bot
de snipe lui-même) reste hors de ce dépôt ; ici on s'arrête à produire les
prédictions et à enregistrer les `trade_outcome` qu'il remonte.

> Historique : la Phase 0 ne contenait QUE la persistance. La contrainte « aucune
> logique métier ici » a été **levée** pour accueillir ingestor + analyseur. Les
> invariants de **schéma** ci-dessous restent, eux, intégralement verrouillés.

## Structure

```
db/schema.sql       Source de vérité UNIQUE du schéma (DDL, commentaires inclus).
src/lib.rs          Déclare les modules db / ingest / analyze + helper now_unix_ms().
src/db/mod.rs       init() + reset_recomputable(). Lit/applique schema.sql.
src/ingest/mod.rs   Ingestor : écrit BRUT + FACT-LIKE (append-only, idempotent).
src/analyze/mod.rs  Analyseur : clustering, profil bayésien, scoring, passthrough, file.
src/bin/analyze.rs  CLI : exécute un tick d'analyse sur une base et affiche le bilan.
tests/schema.rs     Intégrité, tables, WAL, STRICT, frontière de drop.
tests/ingest.rs     Append-only, idempotence, invariant 'alive', atomicité du batch.
tests/analyze.rs    Clustering, profil, scoring, passthrough, recalculabilité post-reset.
Cargo.toml          rusqlite (feature `bundled`, synchrone — PAS sqlx) + tempfile en dev-dep.
```

## Commandes

```bash
cargo build              # compile (rusqlite bundled, premier build ~15 s : SQLite en C)
cargo test               # lance tous les tests d'intégration (tout doit être vert)
cargo clippy --all-targets   # lint (doit rester sans avertissement)
cargo run --bin analyze [chemin.db]   # un tick d'analyse (défaut : intel.db)
```

## Frontière de propriété des écritures (composants)

- **Ingestor** (`src/ingest`) : écrit UNIQUEMENT BRUT (`raw_*`) et FACT-LIKE
  (`token_outcome`, `trade_outcome`). Jamais de table dérivée, jamais `analysis_queue`.
- **Analyseur** (`src/analyze`) : écrit UNIQUEMENT les tables DÉRIVÉ RECALCULABLE,
  les lignes `passthrough_node source='auto'`, et `analysis_queue`. Il LIT les tables
  BRUT/FACT-LIKE mais ne les modifie jamais.

## API exposée

### `src/db/mod.rs`
- `init(path: &str) -> Result<Connection>` : ouvre/crée la base, applique les PRAGMAs
  (`journal_mode=WAL`, `busy_timeout=5000`, `synchronous=NORMAL`) puis le schéma.
  **Idempotent** — sûr à rappeler à chaque démarrage.
- `reset_recomputable(conn: &Connection) -> Result<()>` : DROP/recrée UNIQUEMENT les tables
  recalculables et purge `passthrough_node WHERE source='auto'`. Transactionnel.

Le DDL n'est **jamais** écrit en dur dans le Rust : `schema.sql` est embarqué via
`include_str!` (lecture au build), `schema.sql` reste la seule source de vérité.

### `src/ingest/mod.rs`
- Structures d'entrée : `TokenLaunch`, `WalletFlow`, `LaunchParticipant`, `TokenOutcome`,
  `TradeOutcome`, `EventBatch`.
- `record_launch / record_flow / record_participant / record_outcome -> Result<bool>` :
  `INSERT OR IGNORE`, renvoient `true` si la ligne était nouvelle. `record_trade -> Result<i64>`
  (journal append-only). `ingest_batch -> Result<BatchCounts>` (lot atomique, une transaction).
- `record_outcome` **refuse `label == "alive"`** (`IngestError::ForbiddenLabel`) — l'invariant
  est gardé en code, avant écriture.
- Idempotence des flux : garantie pour un flux de token (`mint` renseigné) ; un flux SOL natif
  (`mint = None`) n'est PAS dédupliqué car SQLite traite `NULL != NULL` dans `UNIQUE`.

### `src/analyze/mod.rs`
- `run_once(conn) -> Result<AnalysisReport>` : un tick complet
  (passthrough → enqueue deployers → analyse). Idempotent.
- `detect_passthrough`, `enqueue_new_deployers`, `analyze_deployer` exposés pour usage ciblé.
- `METHOD_VERSION` : version de méthode estampillée sur `cluster` / `score_prediction`.
- Clustering : 4 types de liens (`funding`, `consolidation`, `cobehavior`, `exclusivity`),
  un seul retenu par wallet (le plus fort). Profil : modèle Beta-Bernoulli du taux de rug
  (`risk` = moyenne a posteriori, `confidence` croît avec `token_count`). Un rug = un
  `token_outcome` terminal `final=1`.

## Schéma — 11 tables, classées par cycle de vie

Toutes les tables sont en mode **STRICT**. Le schéma n'a **volontairement aucune clé
étrangère**. Les adresses/wallets/mint sont en **TEXT** (base58), jamais BLOB.

| Catégorie | Tables | Droppée par `reset_recomputable` ? |
|---|---|---|
| **BRUT** (append-only, vérité terrain) | `raw_token_launch`, `raw_wallet_flow`, `raw_launch_participant` | **Jamais** |
| **FACT-LIKE** (faits non réobservables) | `token_outcome`, `trade_outcome` | **Jamais** |
| **FILE D'ANALYSE** (état opérationnel) | `analysis_queue` | **Jamais** |
| **DÉRIVÉ RECALCULABLE** | `cluster`, `cluster_member`, `cluster_profile`, `score_prediction` | **Oui** (drop/recrée) |
| **MIXTE** | `passthrough_node` | Lignes `source='auto'` supprimées ; `source='seed'` conservées |

## Invariants verrouillés — NE PAS enfreindre

Toutes les décisions de schéma sont **déjà prises et verrouillées**. On les implémente,
on ne les rediscute pas. Si une décision paraît discutable : la signaler en commentaire,
ne pas la changer. Si quelque chose est réellement ambigu/contradictoire : s'arrêter et
demander avant de coder.

### Frontière de DROP (critique)
`reset_recomputable` agit **uniquement** sur : `cluster`, `cluster_member`,
`cluster_profile`, `score_prediction`, et `DELETE FROM passthrough_node WHERE source='auto'`.
Ne JAMAIS dropper/vider : `raw_token_launch`, `raw_wallet_flow`, `raw_launch_participant`,
`token_outcome`, `trade_outcome`, `analysis_queue`, ni `passthrough_node` source='seed'.
Ces données sont des faits observés non réobservables (aucun backfill possible) —
les effacer casse la boucle d'apprentissage.

### Interdictions explicites
- **Aucun index** sur `raw_wallet_flow` (ni `src`, ni `dst`). Ajout seulement si mesuré nécessaire.
- **Aucune clé étrangère**.
- **Ne changer aucun type** de colonne (adresses en TEXT, pas BLOB).
- **Ne pas ajouter/renommer/supprimer** de colonne.
- **Jamais `'alive'`** comme valeur de `token_outcome.label` : c'est une conclusion dérivée
  à la volée (aucun label terminal `final=1` ET âge ≥ 24 h), jamais stockée.
- **Ne pas dupliquer le DDL** dans le Rust : le Rust lit/applique `schema.sql`.
- `foreign_keys` laissé par défaut (OFF) : voulu.

## Modifier le schéma

1. Éditer `db/schema.sql` (en `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`,
   mode STRICT). C'est le seul endroit où vit le DDL.
2. Si la nouvelle table est recalculable, l'ajouter à `RECOMPUTABLE_TABLES` dans
   `src/db/mod.rs`. Si elle est BRUT / FACT-LIKE / file d'analyse, **ne pas l'y ajouter**.
3. Étendre `tests/schema.rs` : présence de la table, mode STRICT, et côté de la frontière
   de drop (subsiste vs vidée).
4. Si la table est lue/écrite par un composant, respecter la frontière de propriété
   (ingestor = BRUT/FACT-LIKE ; analyseur = dérivé + `analysis_queue`) et étendre
   `tests/ingest.rs` ou `tests/analyze.rs`.
5. `cargo test` — tout doit rester vert.
