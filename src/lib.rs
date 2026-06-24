//! Phase 0 — couche de persistance SQLite uniquement.
//!
//! Ce crate n'expose AUCUNE logique métier (ingestion, clustering, scoring,
//! trading). Il se limite au schéma (`db/schema.sql`) et à son application via
//! `rusqlite`. Voir `src/db/mod.rs`.

pub mod db;
