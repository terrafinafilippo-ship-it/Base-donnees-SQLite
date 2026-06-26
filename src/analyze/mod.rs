//! Analyseur — calcule les tables DÉRIVÉ RECALCULABLE à partir des faits terrain.
//!
//! Périmètre d'écriture STRICT : uniquement `cluster`, `cluster_member`,
//! `cluster_profile`, `score_prediction` (toutes droppées/recréées par
//! [`crate::db::reset_recomputable`]), les lignes `passthrough_node`
//! `source='auto'`, et la file `analysis_queue`. L'analyseur ne LIT que les
//! tables BRUT / FACT-LIKE — il ne les modifie jamais.
//!
//! Pipeline d'un tick ([`run_once`]) :
//! 1. [`detect_passthrough`] : repère les hubs (mixeurs/CEX) à fort fan-in ET
//!    fan-out et les marque `source='auto'`. Ces nœuds sont exclus du clustering.
//! 2. [`enqueue_new_deployers`] : inscrit dans `analysis_queue` les deployers
//!    vus dans `raw_token_launch` mais pas encore connus.
//! 3. pour chaque deployer à (re)traiter, [`analyze_deployer`] :
//!    construit/reconstruit son cluster, son profil bayésien et ses prédictions.
//!
//! Le profil de risque est un modèle Beta-Bernoulli : `risk` est la moyenne a
//! posteriori du taux de rug du cluster, `confidence` croît avec le nombre de
//! tokens observés. Un rug = un `token_outcome` terminal `final=1`.

use rusqlite::{params, params_from_iter, Connection, Result};
use std::collections::{HashMap, HashSet};

/// Version de la méthode de clustering/scoring. À incrémenter quand l'algorithme
/// change, pour distinguer les `cluster`/`score_prediction` d'époques différentes.
pub const METHOD_VERSION: i64 = 1;

// --- Réglages du modèle (valeurs par défaut Phase 1, à calibrer sur données). ---

/// Prior Beta(α₀, β₀) du taux de rug. (1,1) = uniforme : aucun a priori fort.
const PRIOR_ALPHA: f64 = 1.0;
const PRIOR_BETA: f64 = 1.0;
/// Lissage de la confiance : `confidence = n / (n + K)` où `n = token_count`.
const CONFIDENCE_K: f64 = 5.0;
/// Seuil de `risk` au-dessus duquel un cluster est jugé rugger.
const RISK_THRESHOLD: f64 = 0.5;
/// Nombre minimal de tokens observés pour oser conclure `is_rugger=1`.
const MIN_SAMPLES_FOR_RUGGER: i64 = 3;
/// Fan-in ET fan-out (degrés distincts) à partir desquels une adresse est jugée
/// passthrough. Le double critère sépare un hub (relais) d'un simple wallet de
/// consolidation (fort fan-in, faible fan-out).
const PASSTHROUGH_MIN_DEGREE: i64 = 6;

/// Bilan d'un tick d'analyse.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AnalysisReport {
    pub passthrough_detected: usize,
    pub deployers_enqueued: usize,
    pub deployers_analyzed: usize,
    pub clusters_built: usize,
    pub members_linked: usize,
    pub predictions_emitted: usize,
}

/// Résultat de l'analyse d'un deployer.
#[derive(Debug, Clone)]
pub struct DeployerResult {
    pub cluster_id: i64,
    pub members: usize,
    pub predictions: usize,
}

/// Exécute un tick complet : passthrough → enqueue → analyse des deployers en
/// attente. Idempotent : relancé sans nouvelles données, il ne reconstruit rien.
pub fn run_once(conn: &Connection) -> Result<AnalysisReport> {
    let mut report = AnalysisReport {
        passthrough_detected: detect_passthrough(conn)?,
        deployers_enqueued: enqueue_new_deployers(conn)?,
        ..Default::default()
    };

    for (deployer, _slot) in deployers_needing_analysis(conn)? {
        mark_status(conn, &deployer, "analyzing")?;
        match analyze_deployer(conn, &deployer) {
            Ok(res) => {
                mark_status(conn, &deployer, "done")?;
                report.deployers_analyzed += 1;
                report.clusters_built += 1;
                report.members_linked += res.members;
                report.predictions_emitted += res.predictions;
            }
            Err(e) => {
                mark_error(conn, &deployer, &e.to_string())?;
            }
        }
    }
    Ok(report)
}

// ===== Étape 1 : détection des nœuds passthrough. =====

/// Détecte les adresses hub (fort fan-in ET fan-out) et les inscrit dans
/// `passthrough_node` avec `source='auto'`.
///
/// Rafraîchissement : purge d'abord les lignes `source='auto'` (jamais les
/// `source='seed'`, denylist permanente), puis réinsère les hubs détectés.
/// `INSERT OR IGNORE` protège une éventuelle adresse déjà présente en `seed`.
/// Renvoie le nombre de hubs auto-marqués.
pub fn detect_passthrough(conn: &Connection) -> Result<usize> {
    struct Agg {
        inn: HashSet<String>,
        out: HashSet<String>,
        max_slot: i64,
    }
    fn new_agg() -> Agg {
        Agg {
            inn: HashSet::new(),
            out: HashSet::new(),
            max_slot: 0,
        }
    }
    let mut agg: HashMap<String, Agg> = HashMap::new();
    // Scan complet de raw_wallet_flow (aucun index : invariant verrouillé).
    {
        let mut stmt = conn.prepare("SELECT src, dst, slot FROM raw_wallet_flow")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (src, dst, slot) = row?;
            if src == dst {
                continue;
            }
            // Deux emprunts mutables successifs (scopés) : pas de pointeur brut,
            // pas d'aliasing. src/dst clonés pour les clés, originaux réutilisés.
            {
                let e = agg.entry(src.clone()).or_insert_with(new_agg);
                e.out.insert(dst.clone());
                if slot > e.max_slot {
                    e.max_slot = slot;
                }
            }
            {
                let e = agg.entry(dst).or_insert_with(new_agg);
                e.inn.insert(src);
                if slot > e.max_slot {
                    e.max_slot = slot;
                }
            }
        }
    }

    conn.execute("DELETE FROM passthrough_node WHERE source = 'auto'", [])?;

    let mut count = 0usize;
    for (addr, a) in agg {
        let ind = a.inn.len() as i64;
        let outd = a.out.len() as i64;
        if ind >= PASSTHROUGH_MIN_DEGREE && outd >= PASSTHROUGH_MIN_DEGREE {
            let degree = a.inn.union(&a.out).count() as i64;
            // Équilibre in/out ∈ [0,1] : 1 = parfaitement balancé (relais pur).
            let heterogeneity = 2.0 * (ind.min(outd) as f64) / ((ind + outd) as f64);
            count += conn.execute(
                "INSERT OR IGNORE INTO passthrough_node \
                 (address, label, source, degree, heterogeneity, decided_slot) \
                 VALUES (?, ?, 'auto', ?, ?, ?)",
                params![addr, "auto-detected hub", degree, heterogeneity, a.max_slot],
            )?;
        }
    }
    Ok(count)
}

// ===== Étape 2 : alimentation de la file d'analyse. =====

/// Inscrit en `pending` tout deployer présent dans `raw_token_launch` mais pas
/// encore dans `analysis_queue`. `INSERT OR IGNORE` préserve le statut des
/// deployers déjà connus. Renvoie le nombre de nouveaux inscrits.
pub fn enqueue_new_deployers(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO analysis_queue \
         (entity, entity_kind, status, attempts, enqueued_slot, updated_unix_ms) \
         SELECT deployer, 'deployer', 'pending', 0, MAX(slot), ? \
         FROM raw_token_launch GROUP BY deployer",
        params![crate::now_unix_ms()],
    )?;
    Ok(n)
}

/// Deployers à (re)traiter : ceux en `pending`, OU ceux dont le cluster a
/// disparu (ex. après [`crate::db::reset_recomputable`]) alors que la file —
/// non droppée — les marque encore `done`. Ce second critère garantit la
/// recalculabilité sans toucher à la frontière de drop ni à la file.
fn deployers_needing_analysis(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT entity, enqueued_slot FROM analysis_queue \
         WHERE entity_kind = 'deployer' \
           AND (status = 'pending' \
                OR NOT EXISTS (SELECT 1 FROM cluster \
                               WHERE anchor_wallet = entity AND method_version = ?))",
    )?;
    let rows = stmt.query_map(params![METHOD_VERSION], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

fn mark_status(conn: &Connection, entity: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE analysis_queue SET status = ?, updated_unix_ms = ? \
         WHERE entity = ? AND entity_kind = 'deployer'",
        params![status, crate::now_unix_ms(), entity],
    )?;
    Ok(())
}

fn mark_error(conn: &Connection, entity: &str, err: &str) -> Result<()> {
    conn.execute(
        "UPDATE analysis_queue \
         SET status = 'error', attempts = attempts + 1, last_error = ?, updated_unix_ms = ? \
         WHERE entity = ? AND entity_kind = 'deployer'",
        params![err, crate::now_unix_ms(), entity],
    )?;
    Ok(())
}

// ===== Étape 3 : analyse d'un deployer. =====

/// (Re)construit cluster + membres + profil + prédictions pour un deployer.
///
/// Reconstruction propre : purge d'abord tout dérivé existant pour cet anchor à
/// la `METHOD_VERSION` courante, puis recrée. Transactionnel.
pub fn analyze_deployer(conn: &Connection, deployer: &str) -> Result<DeployerResult> {
    let passthrough = load_passthrough(conn)?;
    let slot = deployer_max_slot(conn, deployer)?;

    let tx = conn.unchecked_transaction()?;
    purge_anchor(&tx, deployer)?;

    tx.execute(
        "INSERT INTO cluster (anchor_wallet, method_version, updated_slot) VALUES (?, ?, ?)",
        params![deployer, METHOD_VERSION, slot],
    )?;
    let cluster_id = tx.last_insert_rowid();

    let members = collect_members(&tx, deployer, &passthrough)?;
    for (wallet, link_type, strength) in &members {
        tx.execute(
            "INSERT INTO cluster_member (cluster_id, wallet, link_type, link_strength) \
             VALUES (?, ?, ?, ?)",
            params![cluster_id, wallet, link_type, strength],
        )?;
    }

    let profile = compute_profile(&tx, cluster_id, deployer, &members, slot)?;
    let predictions = emit_predictions(&tx, cluster_id, deployer, profile.risk, profile.confidence)?;

    tx.commit()?;
    Ok(DeployerResult {
        cluster_id,
        members: members.len(),
        predictions,
    })
}

/// Charge l'ensemble des adresses passthrough (`seed` + `auto`). Exclues du
/// clustering : ni membres, ni relais de liens.
fn load_passthrough(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT address FROM passthrough_node")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect()
}

fn deployer_max_slot(conn: &Connection, deployer: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(slot), 0) FROM raw_token_launch WHERE deployer = ?",
        params![deployer],
        |r| r.get(0),
    )
}

fn purge_anchor(conn: &Connection, deployer: &str) -> Result<()> {
    let ids: Vec<i64> = {
        let mut stmt =
            conn.prepare("SELECT id FROM cluster WHERE anchor_wallet = ? AND method_version = ?")?;
        let rows = stmt.query_map(params![deployer, METHOD_VERSION], |r| r.get(0))?;
        rows.collect::<Result<_>>()?
    };
    for id in ids {
        conn.execute("DELETE FROM cluster_member WHERE cluster_id = ?", params![id])?;
        conn.execute("DELETE FROM cluster_profile WHERE cluster_id = ?", params![id])?;
        conn.execute("DELETE FROM score_prediction WHERE cluster_id = ?", params![id])?;
        conn.execute("DELETE FROM cluster WHERE id = ?", params![id])?;
    }
    Ok(())
}

/// Force de lien saturante à partir d'un nombre d'occurrences : 1→0.5, 2→0.667,
/// 3→0.75… toujours dans [0,1).
fn saturating_strength(n: i64) -> f64 {
    let n = n.max(0) as f64;
    n / (n + 1.0)
}

/// Construit les membres du cluster ancré sur `deployer`, en croisant quatre
/// signaux. Le PK `(cluster_id, wallet)` n'autorise qu'UN lien par wallet : on
/// retient le plus fort (départage par priorité de type).
///
/// Priorités (départage à force égale) : exclusivity > consolidation > funding >
/// cobehavior.
fn collect_members(
    conn: &Connection,
    deployer: &str,
    passthrough: &HashSet<String>,
) -> Result<Vec<(String, &'static str, f64)>> {
    // wallet -> (priorité, force, type)
    let mut best: HashMap<String, (u8, f64, &'static str)> = HashMap::new();
    let mut consider = |wallet: String, prio: u8, strength: f64, link_type: &'static str| {
        if wallet == deployer || passthrough.contains(&wallet) {
            return;
        }
        match best.get_mut(&wallet) {
            Some(e) if strength > e.1 || (strength == e.1 && prio > e.0) => {
                *e = (prio, strength, link_type);
            }
            Some(_) => {}
            None => {
                best.insert(wallet, (prio, strength, link_type));
            }
        }
    };

    // funding : deployer -> wallet (le deployer a financé le wallet).
    {
        let mut stmt =
            conn.prepare("SELECT dst, COUNT(*) FROM raw_wallet_flow WHERE src = ? GROUP BY dst")?;
        let rows = stmt.query_map(params![deployer], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (wallet, c) = row?;
            consider(wallet, 1, saturating_strength(c), "funding");
        }
    }

    // consolidation : wallet -> deployer (le wallet renvoie vers le deployer).
    {
        let mut stmt =
            conn.prepare("SELECT src, COUNT(*) FROM raw_wallet_flow WHERE dst = ? GROUP BY src")?;
        let rows = stmt.query_map(params![deployer], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (wallet, c) = row?;
            consider(wallet, 2, saturating_strength(c), "consolidation");
        }
    }

    // cobehavior / exclusivity : wallets participant aux tokens du deployer.
    let anchor_mints: Vec<String> = {
        let mut stmt = conn.prepare("SELECT mint FROM raw_token_launch WHERE deployer = ?")?;
        let rows = stmt.query_map(params![deployer], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<_>>()?
    };
    if !anchor_mints.is_empty() {
        let total = anchor_mints.len() as f64;
        let ph = placeholders(anchor_mints.len());
        let sql = format!(
            "SELECT wallet, COUNT(DISTINCT mint) FROM raw_launch_participant \
             WHERE wallet != ? AND mint IN ({ph}) GROUP BY wallet"
        );
        let mut p: Vec<&str> = Vec::with_capacity(1 + anchor_mints.len());
        p.push(deployer);
        p.extend(anchor_mints.iter().map(String::as_str));

        // (wallet, nb_tokens_du_deployer) collecté avant tout autre emprunt de conn.
        let coparticipants: Vec<(String, i64)> = {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(p.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            rows.collect::<Result<_>>()?
        };

        for (wallet, c) in coparticipants {
            if wallet == deployer || passthrough.contains(&wallet) {
                continue;
            }
            let share = c as f64 / total;
            // exclusivity : le wallet ne participe à AUCUN mint hors de l'anchor.
            let outside: i64 = {
                let sql2 = format!(
                    "SELECT COUNT(*) FROM raw_launch_participant \
                     WHERE wallet = ? AND mint NOT IN ({ph})"
                );
                let mut p2: Vec<&str> = Vec::with_capacity(1 + anchor_mints.len());
                p2.push(wallet.as_str());
                p2.extend(anchor_mints.iter().map(String::as_str));
                conn.query_row(&sql2, params_from_iter(p2.iter()), |r| r.get(0))?
            };
            if outside == 0 {
                consider(wallet, 3, share.max(0.5), "exclusivity");
            } else {
                consider(wallet, 0, share, "cobehavior");
            }
        }
    }

    let mut out: Vec<(String, &'static str, f64)> = best
        .into_iter()
        .map(|(wallet, (_, strength, link_type))| (wallet, link_type, strength))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0)); // ordre déterministe
    Ok(out)
}

struct Profile {
    risk: f64,
    confidence: f64,
}

/// Calcule et insère `cluster_profile` (modèle Beta-Bernoulli du taux de rug).
fn compute_profile(
    conn: &Connection,
    cluster_id: i64,
    deployer: &str,
    members: &[(String, &'static str, f64)],
    slot: i64,
) -> Result<Profile> {
    // Wallets du cluster = anchor + membres.
    let mut wallets: Vec<&str> = Vec::with_capacity(members.len() + 1);
    wallets.push(deployer);
    wallets.extend(members.iter().map(|(w, _, _)| w.as_str()));
    let ph = placeholders(wallets.len());

    // Tokens lancés par un wallet du cluster.
    let token_count: i64 = conn.query_row(
        &format!("SELECT COUNT(DISTINCT mint) FROM raw_token_launch WHERE deployer IN ({ph})"),
        params_from_iter(wallets.iter()),
        |r| r.get(0),
    )?;
    // Parmi eux, ceux ayant un label terminal final=1 (= rug). 'alive' n'existe
    // jamais en base (invariant) : aucune ligne ne peut le porter.
    let rug_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT mint) FROM token_outcome \
             WHERE label_class = 'terminal' AND \"final\" = 1 \
               AND mint IN (SELECT mint FROM raw_token_launch WHERE deployer IN ({ph}))"
        ),
        params_from_iter(wallets.iter()),
        |r| r.get(0),
    )?;

    let rug = rug_count.min(token_count);
    let alpha = PRIOR_ALPHA + rug as f64;
    let beta = PRIOR_BETA + (token_count - rug).max(0) as f64;
    let risk = alpha / (alpha + beta);
    let confidence = token_count as f64 / (token_count as f64 + CONFIDENCE_K);
    let is_rugger = (risk >= RISK_THRESHOLD && token_count >= MIN_SAMPLES_FOR_RUGGER) as i64;

    conn.execute(
        "INSERT INTO cluster_profile \
         (cluster_id, token_count, rug_count, beta_alpha, beta_beta, risk, confidence, \
          last_decay_slot, is_rugger) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            cluster_id, token_count, rug, alpha, beta, risk, confidence, slot, is_rugger
        ],
    )?;
    Ok(Profile { risk, confidence })
}

/// Émet une `score_prediction` par token du deployer, au risque/confiance du
/// profil. `predicted_slot` = slot de launch du token.
fn emit_predictions(
    conn: &Connection,
    cluster_id: i64,
    deployer: &str,
    risk: f64,
    confidence: f64,
) -> Result<usize> {
    let mints: Vec<(String, i64)> = {
        let mut stmt = conn.prepare("SELECT mint, slot FROM raw_token_launch WHERE deployer = ?")?;
        let rows = stmt.query_map(params![deployer], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<_>>()?
    };
    for (mint, predicted_slot) in &mints {
        conn.execute(
            "INSERT INTO score_prediction \
             (cluster_id, mint, risk, confidence, method_version, predicted_slot) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![cluster_id, mint, risk, confidence, METHOD_VERSION, predicted_slot],
        )?;
    }
    Ok(mints.len())
}

/// Génère `n` marqueurs `?` séparés par des virgules pour une clause `IN (...)`.
fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(n * 2);
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s
}
