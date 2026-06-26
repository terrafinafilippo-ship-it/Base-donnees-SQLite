//! Système d'intelligence pour le sniping de memecoins Solana — couche données
//! et traitement.
//!
//! Trois modules :
//! - [`db`] : schéma SQLite. `db/schema.sql` reste la SOURCE DE VÉRITÉ UNIQUE ;
//!   ce module l'applique (init idempotent + reset des tables recalculables).
//! - [`ingest`] : l'ingestor. Écrit les observations terrain dans les tables
//!   BRUT (`raw_*`) et FACT-LIKE (`token_outcome`, `trade_outcome`). Append-only.
//! - [`analyze`] : l'analyseur. Calcule les tables DÉRIVÉ RECALCULABLE
//!   (`cluster`, `cluster_member`, `cluster_profile`, `score_prediction`),
//!   détecte les nœuds `passthrough_node` (`source='auto'`) et pilote la file
//!   `analysis_queue`.
//!
//! Frontière de propriété des écritures :
//! - l'ingestor n'écrit JAMAIS de table dérivée ni `analysis_queue` ;
//! - l'analyseur n'écrit JAMAIS de table BRUT ni FACT-LIKE (il les lit).

pub mod analyze;
pub mod db;
pub mod ingest;

/// Horodatage courant en millisecondes Unix.
///
/// Utilisé par l'ingestor (`trade_outcome.ingested_unix_ms`) et l'analyseur
/// (`analysis_queue.updated_unix_ms`). Les horodatages d'observation terrain
/// (`raw_token_launch.seen_unix_ms`) viennent eux de la source, jamais d'ici.
pub(crate) fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
