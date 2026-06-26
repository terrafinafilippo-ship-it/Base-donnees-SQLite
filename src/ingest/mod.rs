//! Ingestor — écrit les observations terrain dans la base.
//!
//! Périmètre d'écriture STRICT : uniquement les tables BRUT (`raw_token_launch`,
//! `raw_wallet_flow`, `raw_launch_participant`) et FACT-LIKE (`token_outcome`,
//! `trade_outcome`). L'ingestor ne touche JAMAIS aux tables dérivées
//! recalculables ni à `analysis_queue` (propriété exclusive de l'analyseur).
//!
//! Toutes ces tables sont append-only : aucune mise à jour, aucune suppression.
//! Les insertions sont idempotentes via `INSERT OR IGNORE` sur les contraintes
//! d'unicité du schéma — réinjecter deux fois la même observation est sûr.
//!
//! La source réelle (Geyser/RPC Solana, journal du bot) vit HORS de ce dépôt.
//! Elle construit les structures ci-dessous et appelle ces fonctions ; c'est le
//! point de branchement de l'ingestion.

use rusqlite::{params, Connection};
use std::fmt;

/// Erreur de l'ingestor : soit une erreur SQLite, soit une violation d'un
/// invariant verrouillé détectée avant l'écriture.
#[derive(Debug)]
pub enum IngestError {
    /// Erreur remontée par rusqlite/SQLite.
    Sqlite(rusqlite::Error),
    /// Tentative d'écrire un `token_outcome.label` interdit (`'alive'`).
    /// `'alive'` est une conclusion dérivée à la volée, jamais stockée
    /// (invariant verrouillé du schéma).
    ForbiddenLabel(String),
}

impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IngestError::Sqlite(e) => write!(f, "erreur SQLite : {e}"),
            IngestError::ForbiddenLabel(label) => write!(
                f,
                "label interdit '{label}' : 'alive' est une conclusion dérivée \
                 (aucun label terminal final=1 et âge >= 24h), jamais stockée"
            ),
        }
    }
}

impl std::error::Error for IngestError {}

impl From<rusqlite::Error> for IngestError {
    fn from(e: rusqlite::Error) -> Self {
        IngestError::Sqlite(e)
    }
}

/// Résultat de l'ingestor.
pub type Result<T> = std::result::Result<T, IngestError>;

// ===== Structures d'entrée — une par table écrite. =====

/// Lancement de token observé on-chain. → `raw_token_launch`.
#[derive(Debug, Clone)]
pub struct TokenLaunch {
    pub mint: String,
    pub deployer: String,
    pub program: String,
    pub slot: i64,
    pub seen_unix_ms: i64,
    pub launch_sig: String,
}

/// Mouvement de fonds entre deux wallets. → `raw_wallet_flow`.
#[derive(Debug, Clone)]
pub struct WalletFlow {
    pub sig: String,
    pub slot: i64,
    pub src: String,
    pub dst: String,
    /// `None` pour un transfert de SOL natif (mint absent). Attention : avec
    /// `mint = None`, la contrainte `UNIQUE(sig, src, dst, mint)` ne déduplique
    /// PAS (SQLite traite `NULL != NULL`). Voir [`record_flow`].
    pub mint: Option<String>,
    pub amount: i64,
    /// Nature du flux, ex. `"sol"`, `"token"`, `"fee"`.
    pub kind: String,
}

/// Participant à un launch. → `raw_launch_participant`.
#[derive(Debug, Clone)]
pub struct LaunchParticipant {
    pub mint: String,
    pub wallet: String,
    pub slot: i64,
    pub amount: Option<i64>,
    pub is_signer: bool,
}

/// Fait observé sur le cycle de vie d'un token. → `token_outcome`.
///
/// `label` ne doit JAMAIS valoir `'alive'` (rejeté par [`record_outcome`]).
#[derive(Debug, Clone)]
pub struct TokenOutcome {
    pub mint: String,
    /// Ex. `'lp_pulled'`, `'price_zero'`, `'dev_dumped'`. Jamais `'alive'`.
    pub label: String,
    /// `'terminal'` ou `'event'`.
    pub label_class: String,
    pub observed_slot: i64,
    /// Finalité anti-fork : `true` après 32 slots empilés.
    pub is_final: bool,
}

/// Action exécutée par le bot de snipe, avalée du journal. → `trade_outcome`.
///
/// FACT-LIKE : irréversible et non réobservable. `ingested_unix_ms` est posé
/// automatiquement à l'instant de l'écriture.
#[derive(Debug, Clone)]
pub struct TradeOutcome {
    pub mint: String,
    pub cluster_id: Option<i64>,
    pub prediction_id: Option<i64>,
    /// `'buy'`, `'sell'`, `'skip'`, `'blocked'`.
    pub action: String,
    pub reason: Option<String>,
    pub amount_sol: Option<i64>,
    /// `None` si position ouverte ou action non exécutée.
    pub pnl_lamports: Option<i64>,
    pub bot_slot: i64,
}

/// Lot d'observations on-chain d'un même créneau, écrit atomiquement.
#[derive(Debug, Default, Clone)]
pub struct EventBatch {
    pub launches: Vec<TokenLaunch>,
    pub flows: Vec<WalletFlow>,
    pub participants: Vec<LaunchParticipant>,
}

/// Nombre de lignes RÉELLEMENT insérées par [`ingest_batch`] (hors doublons
/// ignorés).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BatchCounts {
    pub launches: usize,
    pub flows: usize,
    pub participants: usize,
}

// ===== Insertions unitaires — append-only, idempotentes. =====

/// Enregistre un launch. Renvoie `true` si la ligne était nouvelle, `false` si
/// elle existait déjà (même `mint`).
pub fn record_launch(conn: &Connection, l: &TokenLaunch) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO raw_token_launch \
         (mint, deployer, program, slot, seen_unix_ms, launch_sig) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params![l.mint, l.deployer, l.program, l.slot, l.seen_unix_ms, l.launch_sig],
    )?;
    Ok(n == 1)
}

/// Enregistre un flux de fonds. Renvoie `true` si nouveau, `false` si doublon
/// sur `UNIQUE(sig, src, dst, mint)`.
///
/// Idempotence : garantie pour les flux de token (`mint` renseigné). Pour un
/// flux SOL natif (`mint = None`), SQLite considère `NULL != NULL` dans une
/// contrainte UNIQUE, donc deux flux SOL strictement identiques produisent deux
/// lignes. À la source de fournir des `sig` réellement uniques par transaction.
pub fn record_flow(conn: &Connection, f: &WalletFlow) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO raw_wallet_flow \
         (sig, slot, src, dst, mint, amount, kind) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![f.sig, f.slot, f.src, f.dst, f.mint, f.amount, f.kind],
    )?;
    Ok(n == 1)
}

/// Enregistre un participant. Renvoie `true` si nouveau, `false` si doublon sur
/// `PRIMARY KEY (mint, wallet)`.
pub fn record_participant(conn: &Connection, p: &LaunchParticipant) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO raw_launch_participant \
         (mint, wallet, slot, amount, is_signer) \
         VALUES (?, ?, ?, ?, ?)",
        params![p.mint, p.wallet, p.slot, p.amount, p.is_signer as i64],
    )?;
    Ok(n == 1)
}

/// Enregistre un `token_outcome`. Renvoie `true` si nouveau, `false` si doublon
/// sur `UNIQUE(mint, label, observed_slot)`.
///
/// Rejette `label == "alive"` AVANT toute écriture : c'est un invariant
/// verrouillé du schéma.
pub fn record_outcome(conn: &Connection, o: &TokenOutcome) -> Result<bool> {
    if o.label == "alive" {
        return Err(IngestError::ForbiddenLabel(o.label.clone()));
    }
    let n = conn.execute(
        "INSERT OR IGNORE INTO token_outcome \
         (mint, label, label_class, observed_slot, \"final\") \
         VALUES (?, ?, ?, ?, ?)",
        params![o.mint, o.label, o.label_class, o.observed_slot, o.is_final as i64],
    )?;
    Ok(n == 1)
}

/// Enregistre un `trade_outcome` (journal append-only, toujours inséré).
/// Renvoie le `rowid` de la ligne créée. `ingested_unix_ms` est posé à `now`.
pub fn record_trade(conn: &Connection, t: &TradeOutcome) -> Result<i64> {
    conn.execute(
        "INSERT INTO trade_outcome \
         (mint, cluster_id, prediction_id, action, reason, amount_sol, \
          pnl_lamports, bot_slot, ingested_unix_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            t.mint,
            t.cluster_id,
            t.prediction_id,
            t.action,
            t.reason,
            t.amount_sol,
            t.pnl_lamports,
            t.bot_slot,
            crate::now_unix_ms()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Écrit un lot d'observations on-chain dans une seule transaction.
///
/// Atomique : si une insertion échoue, tout le lot est annulé. Les doublons
/// (déjà présents) sont silencieusement ignorés et NON comptés.
pub fn ingest_batch(conn: &Connection, batch: &EventBatch) -> Result<BatchCounts> {
    let tx = conn.unchecked_transaction()?;
    let mut counts = BatchCounts::default();
    for l in &batch.launches {
        if record_launch(&tx, l)? {
            counts.launches += 1;
        }
    }
    for f in &batch.flows {
        if record_flow(&tx, f)? {
            counts.flows += 1;
        }
    }
    for p in &batch.participants {
        if record_participant(&tx, p)? {
            counts.participants += 1;
        }
    }
    tx.commit()?;
    Ok(counts)
}
