//! Test d'intégration de la couche de persistance (Phase 0).
//!
//! Couvre : intégrité, présence des 9 tables, mode WAL, mode STRICT,
//! déduplication via contraintes UNIQUE/PRIMARY KEY, et frontière de DROP de
//! `reset_recomputable`.

use solana_memecoin_db::db;

/// Les 9 tables attendues dans le schéma.
const EXPECTED_TABLES: &[&str] = &[
    "raw_token_launch",
    "raw_wallet_flow",
    "raw_launch_participant",
    "token_outcome",
    "cluster",
    "cluster_member",
    "cluster_profile",
    "score_prediction",
    "passthrough_node",
    "analysis_queue",
];

/// Crée un répertoire temporaire et une base dedans, renvoie (dir, chemin).
/// Le `TempDir` doit rester vivant le temps du test pour ne pas être supprimé.
fn temp_db() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("création répertoire temporaire");
    let path = dir.path().join("test.db");
    let path_str = path.to_str().expect("chemin UTF-8").to_string();
    (dir, path_str)
}

#[test]
fn init_creates_valid_schema() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // integrity_check == "ok"
    let integrity: String = conn
        .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
        .expect("integrity_check");
    assert_eq!(integrity, "ok", "integrity_check doit renvoyer ok");

    // journal_mode == "wal"
    let journal: String = conn
        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
        .expect("journal_mode");
    assert_eq!(journal.to_lowercase(), "wal", "journal_mode doit être wal");

    // Les 10 tables existent (9 « groupes » du schéma + analysis_queue).
    for table in EXPECTED_TABLES {
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1;",
                [table],
                |r| r.get(0),
            )
            .expect("requête sqlite_master");
        assert_eq!(count, 1, "table manquante : {table}");
    }
}

#[test]
fn init_is_idempotent() {
    let (_dir, path) = temp_db();
    db::init(&path).expect("premier init");
    // Un second init sur la même base ne doit pas échouer.
    let conn = db::init(&path).expect("second init (idempotence)");
    let integrity: String = conn
        .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
        .expect("integrity_check");
    assert_eq!(integrity, "ok");
}

#[test]
fn strict_mode_rejects_wrong_type() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // observed_slot est INTEGER NOT NULL en table STRICT : insérer un TEXT non
    // numérique doit ÉCHOUER (pas de coercition silencieuse).
    let res = conn.execute(
        "INSERT INTO token_outcome (mint, label, label_class, observed_slot) \
         VALUES ('m1', 'lp_pulled', 'terminal', 'pas_un_entier');",
        [],
    );
    assert!(
        res.is_err(),
        "STRICT doit rejeter un TEXT dans une colonne INTEGER"
    );
}

#[test]
fn unique_constraint_rejects_duplicate() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    let insert = || {
        conn.execute(
            "INSERT INTO token_outcome (mint, label, label_class, observed_slot) \
             VALUES ('mint_dup', 'lp_pulled', 'terminal', 100);",
            [],
        )
    };

    insert().expect("premier insert doit réussir");
    // UNIQUE(mint, label, observed_slot) : le doublon exact doit échouer.
    assert!(insert().is_err(), "le second insert identique doit échouer");
}

#[test]
fn reset_recomputable_respects_drop_frontier() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // Données « jamais droppées » (raw + fact-like).
    conn.execute(
        "INSERT INTO raw_token_launch \
         (mint, deployer, program, slot, seen_unix_ms, launch_sig) \
         VALUES ('mintR', 'dep', 'prog', 1, 1700000000000, 'sig');",
        [],
    )
    .expect("insert raw_token_launch");
    conn.execute(
        "INSERT INTO token_outcome (mint, label, label_class, observed_slot) \
         VALUES ('mintR', 'price_zero', 'terminal', 10);",
        [],
    )
    .expect("insert token_outcome");

    // Donnée recalculable : sera droppée/recréée (donc vidée).
    conn.execute(
        "INSERT INTO cluster (anchor_wallet, method_version, updated_slot) \
         VALUES ('anchor', 1, 5);",
        [],
    )
    .expect("insert cluster");

    // passthrough_node : une 'seed' (permanente) et une 'auto' (purgée).
    conn.execute(
        "INSERT INTO passthrough_node (address, source) VALUES ('seed_addr', 'seed');",
        [],
    )
    .expect("insert passthrough seed");
    conn.execute(
        "INSERT INTO passthrough_node (address, source) VALUES ('auto_addr', 'auto');",
        [],
    )
    .expect("insert passthrough auto");

    db::reset_recomputable(&conn).expect("reset_recomputable");

    let count = |sql: &str| -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    };

    // SUBSISTENT.
    assert_eq!(count("SELECT count(*) FROM raw_token_launch;"), 1, "raw_token_launch doit subsister");
    assert_eq!(count("SELECT count(*) FROM token_outcome;"), 1, "token_outcome doit subsister");
    assert_eq!(
        count("SELECT count(*) FROM passthrough_node WHERE source='seed';"),
        1,
        "la ligne passthrough 'seed' doit subsister"
    );

    // DISPARAISSENT.
    assert_eq!(count("SELECT count(*) FROM cluster;"), 0, "la table cluster doit être vidée");
    assert_eq!(
        count("SELECT count(*) FROM passthrough_node WHERE source='auto';"),
        0,
        "la ligne passthrough 'auto' doit disparaître"
    );
}
