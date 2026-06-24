//! Couche de persistance SQLite — Phase 0.
//!
//! `db/schema.sql` est la SOURCE DE VÉRITÉ UNIQUE du schéma. Le DDL n'est jamais
//! écrit en dur dans ce module : il est lu depuis ce fichier au moment de la
//! compilation via `include_str!`. Le binaire applique donc exactement le
//! contenu de `schema.sql`, et il n'existe pas de double définition à tenir
//! synchronisée.

use rusqlite::{Connection, Result};

/// Contenu de `db/schema.sql`, embarqué à la compilation.
///
/// `include_str!` lit le fichier depuis le disque au build : le DDL reste dans
/// `schema.sql` (single source of truth), aucun chemin n'est résolu à l'exécution.
const SCHEMA_SQL: &str = include_str!("../../db/schema.sql");

/// Tables dérivées RECALCULABLES, droppées puis recréées par
/// [`reset_recomputable`]. L'ordre n'a pas d'importance : aucune clé étrangère
/// dans le schéma (décision verrouillée).
///
/// `passthrough_node` n'apparaît PAS ici : elle est mixte (lignes `seed`
/// permanentes), traitée à part par un `DELETE ... WHERE source='auto'`.
const RECOMPUTABLE_TABLES: &[&str] = &[
    "cluster",
    "cluster_member",
    "cluster_profile",
    "score_prediction",
];

/// Renvoie le DDL de `schema.sql` débarrassé de ses lignes `PRAGMA` de tête.
///
/// Les PRAGMAs sont des réglages de connexion déjà posés par [`init`]. Certains
/// (ex. `synchronous`) ne peuvent PAS être modifiés à l'intérieur d'une
/// transaction : on les retire donc avant de réappliquer le DDL depuis
/// [`reset_recomputable`]. On ne réécrit aucun DDL en dur — on filtre seulement
/// la source de vérité `schema.sql`.
fn schema_ddl_only() -> String {
    SCHEMA_SQL
        .lines()
        .filter(|line| !line.trim_start().to_uppercase().starts_with("PRAGMA"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Applique les PRAGMAs de connexion attendus à chaque ouverture.
///
/// `foreign_keys` est laissé par défaut (OFF) : le schéma n'a volontairement
/// aucune clé étrangère.
fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA busy_timeout = 5000;\n\
         PRAGMA synchronous = NORMAL;",
    )
}

/// Ouvre (ou crée) la base à `path`, applique les PRAGMAs et le schéma de façon
/// idempotente. Sûr à rappeler à chaque démarrage : tout le DDL est en
/// `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`.
pub fn init(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    // `schema.sql` contient lui-même les PRAGMAs en tête ; les réappliquer via
    // execute_batch est inoffensif et idempotent.
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

/// DROP puis recrée UNIQUEMENT les tables recalculables, et purge les lignes
/// `passthrough_node` détectées automatiquement (`source='auto'`).
///
/// FRONTIÈRE DE DROP — ne touche JAMAIS : `raw_token_launch`, `raw_wallet_flow`,
/// `raw_launch_participant`, `token_outcome`, `analysis_queue`, ni les lignes
/// `passthrough_node` où `source='seed'`. Ces données sont des faits observés
/// non réobservables (aucun backfill possible).
pub fn reset_recomputable(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    for table in RECOMPUTABLE_TABLES {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {table};"))?;
    }

    // Recrée les tables droppées en réappliquant le DDL (idempotent : les
    // tables non droppées restent intactes grâce à IF NOT EXISTS). On applique
    // le DDL sans les PRAGMAs : `synchronous` n'est pas modifiable en
    // transaction, et ces réglages sont déjà posés à l'ouverture par `init`.
    tx.execute_batch(&schema_ddl_only())?;

    // passthrough_node : on ne supprime QUE les lignes auto-détectées ; les
    // 'seed' (denylist permanente) subsistent.
    tx.execute("DELETE FROM passthrough_node WHERE source = 'auto';", [])?;

    tx.commit()
}
