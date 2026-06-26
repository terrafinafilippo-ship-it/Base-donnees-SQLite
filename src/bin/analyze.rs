//! Exécute UN tick de l'analyseur sur une base existante puis affiche le bilan.
//!
//! Usage : `cargo run --bin analyze [chemin_base]` (défaut : `intel.db`).
//!
//! L'ingestor (alimenté par une source on-chain hors dépôt) remplit les tables
//! BRUT ; ce binaire déclenche le calcul dérivé. À appeler en boucle par un
//! ordonnanceur externe.

use solana_memecoin_db::{analyze, db};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "intel.db".to_string());
    let conn = db::init(&path)?;
    let report = analyze::run_once(&conn)?;
    println!("base       : {path}");
    println!("passthrough: {}", report.passthrough_detected);
    println!("enqueued   : {}", report.deployers_enqueued);
    println!("analyzed   : {}", report.deployers_analyzed);
    println!("clusters   : {}", report.clusters_built);
    println!("members    : {}", report.members_linked);
    println!("predictions: {}", report.predictions_emitted);
    Ok(())
}
