# CLAUDE.md

Guide pour travailler dans ce dépôt. À lire avant toute modification.

## Projet

Système d'intelligence pour le sniping de memecoins Solana, écrit en Rust.
Ce dépôt est en **Phase 0** : il ne contient QUE la **couche de persistance SQLite**
(schéma + application). Aucune logique métier (ingestion, clustering, scoring, trading)
n'y vit encore, et il ne faut pas en ajouter ici.

## Structure

```
db/schema.sql       Source de vérité UNIQUE du schéma (DDL, commentaires inclus).
src/lib.rs          Déclare `pub mod db`.
src/db/mod.rs       init() + reset_recomputable(). Lit/applique schema.sql.
tests/schema.rs     Test d'intégration (intégrité, tables, WAL, STRICT, frontière de drop).
Cargo.toml          rusqlite (feature `bundled`, synchrone — PAS sqlx) + tempfile en dev-dep.
```

## Commandes

```bash
cargo build      # compile (rusqlite bundled, premier build ~15 s : compile SQLite en C)
cargo test       # lance les tests d'intégration (tout doit être vert)
```

## API exposée (`src/db/mod.rs`)

- `init(path: &str) -> Result<Connection>` : ouvre/crée la base, applique les PRAGMAs
  (`journal_mode=WAL`, `busy_timeout=5000`, `synchronous=NORMAL`) puis le schéma.
  **Idempotent** — sûr à rappeler à chaque démarrage.
- `reset_recomputable(conn: &Connection) -> Result<()>` : DROP/recrée UNIQUEMENT les tables
  recalculables et purge `passthrough_node WHERE source='auto'`. Transactionnel.

Le DDL n'est **jamais** écrit en dur dans le Rust : `schema.sql` est embarqué via
`include_str!` (lecture au build), `schema.sql` reste la seule source de vérité.

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
4. `cargo test` — tout doit rester vert.
