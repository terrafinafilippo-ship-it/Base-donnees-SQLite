//! Tests d'intégration de l'ingestor : append-only, idempotence, invariant
//! 'alive', atomicité du batch.

use rusqlite::Connection;
use solana_memecoin_db::db;
use solana_memecoin_db::ingest::{
    self, EventBatch, IngestError, LaunchParticipant, TokenLaunch, TokenOutcome, TradeOutcome,
    WalletFlow,
};

fn temp_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ingest.db");
    let conn = db::init(path.to_str().unwrap()).unwrap();
    (dir, conn)
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn sample_launch() -> TokenLaunch {
    TokenLaunch {
        mint: "MintA".into(),
        deployer: "DeployerA".into(),
        program: "pump".into(),
        slot: 100,
        seen_unix_ms: 1700,
        launch_sig: "sigA".into(),
    }
}

#[test]
fn record_launch_is_append_only_and_idempotent() {
    let (_d, conn) = temp_db();
    let l = sample_launch();

    assert!(ingest::record_launch(&conn, &l).unwrap(), "première insertion");
    assert!(
        !ingest::record_launch(&conn, &l).unwrap(),
        "doublon sur PRIMARY KEY mint : ignoré"
    );
    assert_eq!(count(&conn, "raw_token_launch"), 1);
}

#[test]
fn record_flow_dedupes_token_flows_but_appends_native_sol() {
    let (_d, conn) = temp_db();
    // Flux de TOKEN (mint renseigné) : dédupliqué par UNIQUE(sig,src,dst,mint).
    let tok = WalletFlow {
        sig: "s1".into(),
        slot: 10,
        src: "W1".into(),
        dst: "W2".into(),
        mint: Some("MintA".into()),
        amount: 42,
        kind: "token".into(),
    };
    assert!(ingest::record_flow(&conn, &tok).unwrap());
    assert!(
        !ingest::record_flow(&conn, &tok).unwrap(),
        "doublon token : ignoré"
    );
    // Même sig mais dst différent => nouvelle ligne.
    let mut tok2 = tok.clone();
    tok2.dst = "W3".into();
    assert!(ingest::record_flow(&conn, &tok2).unwrap());

    // Flux SOL natif (mint NULL) : SQLite traite NULL != NULL dans une
    // contrainte UNIQUE, donc deux flux SOL identiques NE sont PAS dédupliqués.
    let sol = WalletFlow {
        sig: "s2".into(),
        slot: 11,
        src: "W1".into(),
        dst: "W2".into(),
        mint: None,
        amount: 1,
        kind: "sol".into(),
    };
    assert!(ingest::record_flow(&conn, &sol).unwrap());
    assert!(
        ingest::record_flow(&conn, &sol).unwrap(),
        "NULL mint : non dédupliqué (sémantique SQLite)"
    );

    assert_eq!(count(&conn, "raw_wallet_flow"), 4);
}

#[test]
fn record_participant_idempotent_on_mint_wallet() {
    let (_d, conn) = temp_db();
    let p = LaunchParticipant {
        mint: "MintA".into(),
        wallet: "W1".into(),
        slot: 100,
        amount: Some(5),
        is_signer: true,
    };
    assert!(ingest::record_participant(&conn, &p).unwrap());
    assert!(!ingest::record_participant(&conn, &p).unwrap());
    assert_eq!(count(&conn, "raw_launch_participant"), 1);
}

#[test]
fn record_outcome_rejects_alive_label() {
    let (_d, conn) = temp_db();
    let alive = TokenOutcome {
        mint: "MintA".into(),
        label: "alive".into(),
        label_class: "terminal".into(),
        observed_slot: 200,
        is_final: true,
    };
    let err = ingest::record_outcome(&conn, &alive).unwrap_err();
    assert!(matches!(err, IngestError::ForbiddenLabel(l) if l == "alive"));
    // Rien écrit : l'invariant est gardé AVANT toute écriture.
    assert_eq!(count(&conn, "token_outcome"), 0);
}

#[test]
fn record_outcome_stores_terminal_label() {
    let (_d, conn) = temp_db();
    let o = TokenOutcome {
        mint: "MintA".into(),
        label: "lp_pulled".into(),
        label_class: "terminal".into(),
        observed_slot: 200,
        is_final: true,
    };
    assert!(ingest::record_outcome(&conn, &o).unwrap());
    assert!(
        !ingest::record_outcome(&conn, &o).unwrap(),
        "doublon UNIQUE(mint,label,observed_slot)"
    );
    let final_val: i64 = conn
        .query_row(
            "SELECT \"final\" FROM token_outcome WHERE mint = 'MintA'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(final_val, 1);
}

#[test]
fn record_trade_appends_and_returns_rowid() {
    let (_d, conn) = temp_db();
    let t = TradeOutcome {
        mint: "MintA".into(),
        cluster_id: Some(7),
        prediction_id: None,
        action: "buy".into(),
        reason: None,
        amount_sol: Some(1_000_000),
        pnl_lamports: None,
        bot_slot: 300,
    };
    let id1 = ingest::record_trade(&conn, &t).unwrap();
    let id2 = ingest::record_trade(&conn, &t).unwrap();
    assert_ne!(id1, id2, "journal append-only : deux lignes distinctes");
    assert_eq!(count(&conn, "trade_outcome"), 2);
    let ingested: i64 = conn
        .query_row(
            "SELECT ingested_unix_ms FROM trade_outcome WHERE id = ?",
            [id1],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ingested > 0, "ingested_unix_ms posé automatiquement");
}

#[test]
fn ingest_batch_counts_new_rows_and_is_idempotent() {
    let (_d, conn) = temp_db();
    let batch = EventBatch {
        launches: vec![sample_launch()],
        flows: vec![WalletFlow {
            sig: "s1".into(),
            slot: 10,
            src: "DeployerA".into(),
            dst: "W1".into(),
            mint: Some("MintA".into()),
            amount: 1,
            kind: "token".into(),
        }],
        participants: vec![LaunchParticipant {
            mint: "MintA".into(),
            wallet: "W1".into(),
            slot: 100,
            amount: None,
            is_signer: false,
        }],
    };

    let c1 = ingest::ingest_batch(&conn, &batch).unwrap();
    assert_eq!(c1.launches, 1);
    assert_eq!(c1.flows, 1);
    assert_eq!(c1.participants, 1);

    // Rejouer le même lot : tout est déjà là, aucun nouveau compté.
    let c2 = ingest::ingest_batch(&conn, &batch).unwrap();
    assert_eq!(c2.launches, 0);
    assert_eq!(c2.flows, 0);
    assert_eq!(c2.participants, 0);

    assert_eq!(count(&conn, "raw_token_launch"), 1);
    assert_eq!(count(&conn, "raw_wallet_flow"), 1);
    assert_eq!(count(&conn, "raw_launch_participant"), 1);
}
