//! Tests d'intégration de l'analyseur : clustering (4 types de liens), profil
//! bayésien, scoring, détection passthrough, pilotage de la file, et
//! recalculabilité après `reset_recomputable`.

use rusqlite::Connection;
use solana_memecoin_db::ingest::{
    self, LaunchParticipant, TokenLaunch, TokenOutcome, WalletFlow,
};
use solana_memecoin_db::{analyze, db};

fn temp_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("analyze.db");
    let conn = db::init(path.to_str().unwrap()).unwrap();
    (dir, conn)
}

fn launch(conn: &Connection, mint: &str, deployer: &str, slot: i64) {
    ingest::record_launch(
        conn,
        &TokenLaunch {
            mint: mint.into(),
            deployer: deployer.into(),
            program: "pump".into(),
            slot,
            seen_unix_ms: 0,
            launch_sig: format!("sig-{mint}"),
        },
    )
    .unwrap();
}

fn flow(conn: &Connection, sig: &str, src: &str, dst: &str, slot: i64) {
    ingest::record_flow(
        conn,
        &WalletFlow {
            sig: sig.into(),
            slot,
            src: src.into(),
            dst: dst.into(),
            mint: None,
            amount: 1,
            kind: "sol".into(),
        },
    )
    .unwrap();
}

fn participant(conn: &Connection, mint: &str, wallet: &str, slot: i64) {
    ingest::record_participant(
        conn,
        &LaunchParticipant {
            mint: mint.into(),
            wallet: wallet.into(),
            slot,
            amount: None,
            is_signer: false,
        },
    )
    .unwrap();
}

fn rug(conn: &Connection, mint: &str, label: &str, slot: i64) {
    ingest::record_outcome(
        conn,
        &TokenOutcome {
            mint: mint.into(),
            label: label.into(),
            label_class: "terminal".into(),
            observed_slot: slot,
            is_final: true,
        },
    )
    .unwrap();
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn cluster_id(conn: &Connection, anchor: &str) -> i64 {
    conn.query_row(
        "SELECT id FROM cluster WHERE anchor_wallet = ? AND method_version = ?",
        rusqlite::params![anchor, analyze::METHOD_VERSION],
        |r| r.get(0),
    )
    .unwrap()
}

/// Construit le scénario : deployer D (rugger en série) + deployer E (propre),
/// wallets liés W1/W2, et leurs outcomes.
fn build_scenario(conn: &Connection) {
    // D lance 3 tokens ; M1 et M2 ruggent, M3 reste "alive" (aucun outcome).
    launch(conn, "M1", "D", 10);
    launch(conn, "M2", "D", 20);
    launch(conn, "M3", "D", 30);
    rug(conn, "M1", "lp_pulled", 15);
    rug(conn, "M2", "dev_dumped", 25);

    // E lance 1 token propre.
    launch(conn, "MX", "E", 40);

    // Flux : D finance W1 et W2 (funding) ; W2 reconsolide vers D (consolidation).
    flow(conn, "f1", "D", "W1", 11);
    flow(conn, "f2", "D", "W2", 12);
    flow(conn, "f3", "W2", "D", 13);

    // Participations : W1 exclusif aux tokens de D ; W2 participe aussi à MX (E).
    participant(conn, "M1", "W1", 10);
    participant(conn, "M2", "W1", 20);
    participant(conn, "M1", "W2", 10);
    participant(conn, "MX", "W2", 40);
}

#[test]
fn run_once_builds_cluster_profile_and_predictions() {
    let (_d, conn) = temp_db();
    build_scenario(&conn);

    let report = analyze::run_once(&conn).unwrap();
    assert_eq!(report.deployers_enqueued, 2, "D et E inscrits");
    assert_eq!(report.deployers_analyzed, 2);

    // Profil de D : 3 tokens, 2 rugs, jugé rugger.
    let cid = cluster_id(&conn, "D");
    let (token_count, rug_count, is_rugger): (i64, i64, i64) = conn
        .query_row(
            "SELECT token_count, rug_count, is_rugger FROM cluster_profile WHERE cluster_id = ?",
            [cid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(token_count, 3);
    assert_eq!(rug_count, 2);
    assert_eq!(is_rugger, 1);

    let risk: f64 = conn
        .query_row(
            "SELECT risk FROM cluster_profile WHERE cluster_id = ?",
            [cid],
            |r| r.get(0),
        )
        .unwrap();
    assert!((risk - 0.6).abs() < 1e-9, "Beta(3,2) => moyenne 0.6, eu {risk}");

    // Une prédiction par token de D.
    let preds: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM score_prediction WHERE cluster_id = ?",
            [cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(preds, 3);

    // E : un seul token, aucun rug => non rugger.
    let eid = cluster_id(&conn, "E");
    let (e_tokens, e_rugger): (i64, i64) = conn
        .query_row(
            "SELECT token_count, is_rugger FROM cluster_profile WHERE cluster_id = ?",
            [eid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(e_tokens, 1);
    assert_eq!(e_rugger, 0);

    // File : D et E marqués done.
    let done: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM analysis_queue WHERE entity_kind='deployer' AND status='done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done, 2);
}

#[test]
fn member_link_types_reflect_strongest_signal() {
    // Lot 1 : seuls les flux fondent l'appartenance, les types émis sont
    // exclusivement 'funding' et 'consolidation'.
    //
    // Avant Lot 1, W1 (co-participant sans flux propre) devenait "exclusivity".
    // Après Lot 1, W1 entre via le flux D→W1 (funding) ; la co-participation
    // lui confère un bonus de force mais ne change pas son type fondateur.
    // W2 reste "consolidation" (prio 2 > prio 1 à force égale, puis boost corroboratif).
    let (_d, conn) = temp_db();
    build_scenario(&conn);
    analyze::run_once(&conn).unwrap();
    let cid = cluster_id(&conn, "D");

    // W1 : flux D→W1 (funding) → type fondateur 'funding'. L'ancien type
    // 'exclusivity' est supprimé (invariant Lot 1).
    let w1_type: String = conn
        .query_row(
            "SELECT link_type FROM cluster_member WHERE cluster_id = ? AND wallet = 'W1'",
            [cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(w1_type, "funding", "W1 entre via flux D→W1 : type fondateur = funding");

    // W2 : flux funding (D→W2) et consolidation (W2→D) à force égale
    // => consolidation gagne (priorité 2 > 1). Type fondateur conservé.
    let w2_type: String = conn
        .query_row(
            "SELECT link_type FROM cluster_member WHERE cluster_id = ? AND wallet = 'W2'",
            [cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(w2_type, "consolidation");
}

#[test]
fn coparticipant_without_flow_is_not_a_member() {
    // Invariant verrouillé Lot 1 : un wallet co-participant à TOUS les mints
    // du deployer mais sans aucun flux de fonds vers/depuis ce deployer NE DOIT
    // PAS apparaître dans cluster_member.
    let (_d, conn) = temp_db();
    launch(&conn, "M1", "D", 10);
    launch(&conn, "M2", "D", 20);
    // W co-participe aux deux tokens de D, aucun flux D↔W.
    participant(&conn, "M1", "W", 10);
    participant(&conn, "M2", "W", 20);

    analyze::run_once(&conn).unwrap();
    let cid = cluster_id(&conn, "D");

    let w_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cluster_member WHERE cluster_id = ? AND wallet = 'W'",
            [cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(w_count, 0, "co-participation seule ne fonde pas l'appartenance");
}

#[test]
fn cobehavior_boosts_existing_member_strength() {
    // Un wallet avec un flux 'funding' ET une co-participation doit avoir une
    // force strictement supérieure à un wallet avec le même flux mais sans
    // co-participation, dans la limite de COBEHAVIOR_STRENGTH_CAP. Le type
    // fondateur 'funding' doit être conservé.
    let (_d, conn) = temp_db();
    launch(&conn, "M1", "D", 10);
    launch(&conn, "M2", "D", 20);

    // W1 : 1 flux funding + co-participation aux 2 mints (share = 1.0).
    flow(&conn, "f1", "D", "W1", 11);
    participant(&conn, "M1", "W1", 10);
    participant(&conn, "M2", "W1", 20);

    // W2 : 1 flux funding identique, aucune co-participation.
    flow(&conn, "f2", "D", "W2", 12);

    analyze::run_once(&conn).unwrap();
    let cid = cluster_id(&conn, "D");

    let (w1_type, w1_str): (String, f64) = conn
        .query_row(
            "SELECT link_type, link_strength FROM cluster_member WHERE cluster_id = ? AND wallet = 'W1'",
            [cid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let w2_str: f64 = conn
        .query_row(
            "SELECT link_strength FROM cluster_member WHERE cluster_id = ? AND wallet = 'W2'",
            [cid],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(w1_type, "funding", "le type fondateur est conservé après boost corroboratif");
    assert!(
        w1_str > w2_str,
        "la co-participation renforce la force : W1={w1_str} doit être > W2={w2_str}"
    );
    assert!(
        w1_str <= analyze::COBEHAVIOR_STRENGTH_CAP,
        "la force est plafonnée à COBEHAVIOR_STRENGTH_CAP : {w1_str}"
    );
}

#[test]
fn no_member_has_exclusivity_or_cobehavior_link_type() {
    // Après un run complet, aucune ligne de cluster_member ne doit porter
    // link_type 'exclusivity' ou 'cobehavior' : ces valeurs ne sont plus
    // produites depuis Lot 1.
    let (_d, conn) = temp_db();
    build_scenario(&conn);
    analyze::run_once(&conn).unwrap();

    let bad: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cluster_member WHERE link_type IN ('exclusivity', 'cobehavior')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad, 0, "aucun membre ne doit porter les types obsolètes");
}

#[test]
fn passthrough_hub_is_detected_and_excluded_from_clusters() {
    let (_d, conn) = temp_db();
    // P lance un token (=> enqueue) et finance le hub H.
    launch(&conn, "MP", "P", 10);
    flow(&conn, "p-h", "P", "H", 11);

    // H est un hub : 6 émetteurs distincts vers H, 6 destinataires distincts.
    for i in 0..6 {
        flow(&conn, &format!("s{i}-h"), &format!("S{i}"), "H", 20 + i);
        flow(&conn, &format!("h-r{i}"), "H", &format!("R{i}"), 30 + i);
    }

    let report = analyze::run_once(&conn).unwrap();
    assert_eq!(report.passthrough_detected, 1, "H repéré comme hub");

    // H présent en passthrough_node source='auto'.
    let src: String = conn
        .query_row(
            "SELECT source FROM passthrough_node WHERE address = 'H'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(src, "auto");

    // Bien que P -> H (funding), H est exclu du cluster de P.
    let cid = cluster_id(&conn, "P");
    let h_members: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cluster_member WHERE cluster_id = ? AND wallet = 'H'",
            [cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(h_members, 0, "hub passthrough jamais membre");
}

#[test]
fn seed_passthrough_is_preserved_across_detection() {
    let (_d, conn) = temp_db();
    // Denylist permanente posée manuellement (rôle d'un seed externe).
    conn.execute(
        "INSERT INTO passthrough_node (address, label, source) VALUES ('SEED', 'cex', 'seed')",
        [],
    )
    .unwrap();
    launch(&conn, "MP", "P", 10);

    analyze::detect_passthrough(&conn).unwrap();
    let seed_still_there: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM passthrough_node WHERE address='SEED' AND source='seed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(seed_still_there, 1, "le seed survit à la détection auto");
}

#[test]
fn run_once_is_idempotent() {
    let (_d, conn) = temp_db();
    build_scenario(&conn);

    analyze::run_once(&conn).unwrap();
    let clusters_after_first = count(&conn, "cluster");

    // Relancé sans nouvelles données : rien à refaire.
    let second = analyze::run_once(&conn).unwrap();
    assert_eq!(second.deployers_analyzed, 0);
    assert_eq!(count(&conn, "cluster"), clusters_after_first);
}

#[test]
fn derived_state_is_recomputable_after_reset() {
    let (_d, conn) = temp_db();
    build_scenario(&conn);
    analyze::run_once(&conn).unwrap();

    let cid = cluster_id(&conn, "D");
    let before: (i64, i64) = conn
        .query_row(
            "SELECT token_count, rug_count FROM cluster_profile WHERE cluster_id = ?",
            [cid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    // reset_recomputable vide le dérivé mais PAS la file ni les faits bruts.
    db::reset_recomputable(&conn).unwrap();
    assert_eq!(count(&conn, "cluster"), 0);
    assert_eq!(count(&conn, "score_prediction"), 0);
    assert_eq!(count(&conn, "raw_token_launch"), 4, "faits bruts intacts");
    assert!(
        count(&conn, "analysis_queue") >= 2,
        "file non droppée par reset"
    );

    // L'analyseur reconstruit à l'identique malgré la file marquée 'done'.
    let report = analyze::run_once(&conn).unwrap();
    assert_eq!(report.deployers_analyzed, 2, "deployers re-analysés");

    let cid2 = cluster_id(&conn, "D");
    let after: (i64, i64) = conn
        .query_row(
            "SELECT token_count, rug_count FROM cluster_profile WHERE cluster_id = ?",
            [cid2],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(before, after, "profil reconstruit identique");
}
