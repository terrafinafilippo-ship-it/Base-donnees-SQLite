//! Test d'intégration de la couche de persistance (Phase 0).
//!
//! Couvre : intégrité, présence des 11 tables, mode WAL, mode STRICT,
//! déduplication via contraintes UNIQUE/PRIMARY KEY, et frontière de DROP de
//! `reset_recomputable`.

use solana_memecoin_db::db;

/// Les 11 tables attendues dans le schéma.
const EXPECTED_TABLES: &[&str] = &[
    "raw_token_launch",
    "raw_wallet_flow",
    "raw_launch_participant",
    "token_outcome",
    "trade_outcome",
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

    // Les 11 tables du schéma existent.
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
fn trade_outcome_survives_reset_recomputable() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // trade_outcome est FACT-LIKE : un trade exécuté est un fait irréversible,
    // reset_recomputable ne doit PAS le toucher.
    conn.execute(
        "INSERT INTO trade_outcome \
         (mint, cluster_id, prediction_id, action, reason, amount_sol, pnl_lamports, bot_slot, ingested_unix_ms) \
         VALUES ('mintT', 7, 42, 'buy', NULL, 1000000000, 250000000, 999, 1700000000000);",
        [],
    )
    .expect("insert trade_outcome");

    db::reset_recomputable(&conn).expect("reset_recomputable");

    let count: i64 = conn
        .query_row("SELECT count(*) FROM trade_outcome;", [], |r| r.get(0))
        .expect("count trade_outcome");
    assert_eq!(count, 1, "la ligne trade_outcome doit subsister (fact-like)");
}

#[test]
fn trade_outcome_strict_mode_rejects_wrong_type() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // bot_slot est INTEGER NOT NULL en table STRICT : un TEXT non numérique doit
    // ÉCHOUER (pas de coercition silencieuse).
    let res = conn.execute(
        "INSERT INTO trade_outcome (mint, action, bot_slot, ingested_unix_ms) \
         VALUES ('mintT', 'buy', 'pas_un_entier', 1700000000000);",
        [],
    );
    assert!(
        res.is_err(),
        "STRICT doit rejeter un TEXT dans la colonne INTEGER bot_slot"
    );
}

#[test]
fn all_tables_are_strict() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // Introspection via PRAGMA table_list : la colonne `strict` vaut 1 pour une
    // table déclarée STRICT. Le test échoue si UNE SEULE des 11 tables perdait
    // ce mode (ou disparaissait).
    for table in EXPECTED_TABLES {
        let strict: i64 = conn
            .query_row(
                "SELECT strict FROM pragma_table_list WHERE name = ?1;",
                [table],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("table_list pour {table} : {e}"));
        assert_eq!(strict, 1, "la table {table} doit être en mode STRICT");
    }
}

/// Compte les lignes INSERT ACTIVES (non commentées) ciblant `passthrough_node`
/// dans `db/seed_passthrough.sql`. Indépendant du contenu réel : marche que le
/// fichier ait 0 ou 46 adresses. Le format imposé (une ligne INSERT par adresse,
/// `INSERT OR IGNORE INTO passthrough_node`) rend ce comptage fiable.
fn count_seed_inserts() -> i64 {
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/db/seed_passthrough.sql"
    ))
    .expect("lecture db/seed_passthrough.sql");
    sql.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("--"))
        .filter(|line| {
            line.contains("INSERT OR IGNORE INTO passthrough_node")
                || line.contains("INSERT INTO passthrough_node")
        })
        .count() as i64
}

#[test]
fn init_seeds_passthrough_denylist() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // Le nombre de lignes 'seed' en base doit refléter EXACTEMENT le nombre
    // d'INSERT actifs du fichier. Si le seeding est débranché, ce test casse.
    let expected = count_seed_inserts();
    let seeded: i64 = conn
        .query_row(
            "SELECT count(*) FROM passthrough_node WHERE source='seed';",
            [],
            |r| r.get(0),
        )
        .expect("count seed");
    assert_eq!(
        seeded, expected,
        "init doit insérer exactement les {expected} adresses 'seed' du fichier"
    );
}

#[test]
fn init_seeding_is_idempotent() {
    let (_dir, path) = temp_db();
    db::init(&path).expect("premier init");
    // Un second init ne doit créer aucun doublon (INSERT OR IGNORE sur la PK).
    let conn = db::init(&path).expect("second init");
    let seeded: i64 = conn
        .query_row(
            "SELECT count(*) FROM passthrough_node WHERE source='seed';",
            [],
            |r| r.get(0),
        )
        .expect("count seed");
    assert_eq!(
        seeded,
        count_seed_inserts(),
        "le ré-init ne doit pas dupliquer les lignes 'seed'"
    );
}

#[test]
fn reset_recomputable_respects_drop_frontier() {
    let (_dir, path) = temp_db();
    let conn = db::init(&path).expect("init");

    // ===== Côté « jamais droppé » : on remplit TOUTES ces tables. =====
    conn.execute(
        "INSERT INTO raw_token_launch \
         (mint, deployer, program, slot, seen_unix_ms, launch_sig) \
         VALUES ('mintR', 'dep', 'prog', 1, 1700000000000, 'sig');",
        [],
    )
    .expect("insert raw_token_launch");
    conn.execute(
        "INSERT INTO raw_wallet_flow (sig, slot, src, dst, mint, amount, kind) \
         VALUES ('sigF', 2, 'srcW', 'dstW', 'mintR', 1000, 'transfer');",
        [],
    )
    .expect("insert raw_wallet_flow");
    conn.execute(
        "INSERT INTO raw_launch_participant (mint, wallet, slot, amount, is_signer) \
         VALUES ('mintR', 'walletP', 3, 500, 1);",
        [],
    )
    .expect("insert raw_launch_participant");
    conn.execute(
        "INSERT INTO token_outcome (mint, label, label_class, observed_slot) \
         VALUES ('mintR', 'price_zero', 'terminal', 10);",
        [],
    )
    .expect("insert token_outcome");
    conn.execute(
        "INSERT INTO trade_outcome (mint, action, bot_slot, ingested_unix_ms) \
         VALUES ('mintR', 'buy', 11, 1700000000001);",
        [],
    )
    .expect("insert trade_outcome");
    conn.execute(
        "INSERT INTO analysis_queue (entity, entity_kind, enqueued_slot, updated_unix_ms) \
         VALUES ('dep', 'deployer', 4, 1700000000002);",
        [],
    )
    .expect("insert analysis_queue");
    // passthrough_node 'seed' = denylist permanente, ne doit pas être purgée.
    conn.execute(
        "INSERT INTO passthrough_node (address, source) VALUES ('seed_addr', 'seed');",
        [],
    )
    .expect("insert passthrough seed");

    // ===== Côté « recalculable » : on remplit TOUTES ces tables. =====
    conn.execute(
        "INSERT INTO cluster (anchor_wallet, method_version, updated_slot) \
         VALUES ('anchor', 1, 5);",
        [],
    )
    .expect("insert cluster");
    conn.execute(
        "INSERT INTO cluster_member (cluster_id, wallet, link_type, link_strength) \
         VALUES (1, 'walletM', 'funding', 0.9);",
        [],
    )
    .expect("insert cluster_member");
    conn.execute(
        "INSERT INTO cluster_profile (cluster_id, beta_alpha, beta_beta) \
         VALUES (1, 1.0, 1.0);",
        [],
    )
    .expect("insert cluster_profile");
    conn.execute(
        "INSERT INTO score_prediction \
         (cluster_id, mint, risk, confidence, method_version, predicted_slot) \
         VALUES (1, 'mintR', 0.5, 0.5, 1, 6);",
        [],
    )
    .expect("insert score_prediction");
    // passthrough_node 'auto' = détectée, doit être purgée.
    conn.execute(
        "INSERT INTO passthrough_node (address, source) VALUES ('auto_addr', 'auto');",
        [],
    )
    .expect("insert passthrough auto");

    db::reset_recomputable(&conn).expect("reset_recomputable");

    let count = |sql: &str| -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    };

    // ===== DOIVENT SUBSISTER (comptes exacts). =====
    assert_eq!(count("SELECT count(*) FROM raw_token_launch;"), 1, "raw_token_launch doit subsister");
    assert_eq!(count("SELECT count(*) FROM raw_wallet_flow;"), 1, "raw_wallet_flow doit subsister");
    assert_eq!(count("SELECT count(*) FROM raw_launch_participant;"), 1, "raw_launch_participant doit subsister");
    assert_eq!(count("SELECT count(*) FROM token_outcome;"), 1, "token_outcome doit subsister");
    assert_eq!(count("SELECT count(*) FROM trade_outcome;"), 1, "trade_outcome doit subsister");
    assert_eq!(count("SELECT count(*) FROM analysis_queue;"), 1, "analysis_queue ne doit JAMAIS être droppée");
    assert_eq!(
        count("SELECT count(*) FROM passthrough_node WHERE source='seed';"),
        1,
        "la ligne passthrough 'seed' doit subsister"
    );

    // ===== DOIVENT ÊTRE VIDÉES (comptes exacts). =====
    assert_eq!(count("SELECT count(*) FROM cluster;"), 0, "cluster doit être vidée");
    assert_eq!(count("SELECT count(*) FROM cluster_member;"), 0, "cluster_member doit être vidée");
    assert_eq!(count("SELECT count(*) FROM cluster_profile;"), 0, "cluster_profile doit être vidée");
    assert_eq!(count("SELECT count(*) FROM score_prediction;"), 0, "score_prediction doit être vidée");
    assert_eq!(
        count("SELECT count(*) FROM passthrough_node WHERE source='auto';"),
        0,
        "la ligne passthrough 'auto' doit disparaître"
    );
}
