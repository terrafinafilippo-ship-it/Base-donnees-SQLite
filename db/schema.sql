PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;

-- ===== BRUT — append-only, seule vérité terrain, jamais droppé. Pas de FK. =====

CREATE TABLE IF NOT EXISTS raw_token_launch (
    mint          TEXT PRIMARY KEY,
    deployer      TEXT NOT NULL,
    program       TEXT NOT NULL,
    slot          INTEGER NOT NULL,
    seen_unix_ms  INTEGER NOT NULL,
    launch_sig    TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS raw_wallet_flow (
    id      INTEGER PRIMARY KEY,
    sig     TEXT NOT NULL,
    slot    INTEGER NOT NULL,
    src     TEXT NOT NULL,
    dst     TEXT NOT NULL,
    mint    TEXT,
    amount  INTEGER NOT NULL,
    kind    TEXT NOT NULL,
    UNIQUE(sig, src, dst, mint)
) STRICT;
-- Aucun index (src)/(dst) en Phase 0. Volontaire.

CREATE TABLE IF NOT EXISTS raw_launch_participant (
    mint       TEXT NOT NULL,
    wallet     TEXT NOT NULL,
    slot       INTEGER NOT NULL,
    amount     INTEGER,
    is_signer  INTEGER NOT NULL,
    PRIMARY KEY (mint, wallet)
) STRICT;

-- ===== FACT-LIKE — côté analyseur mais NON régénérable, jamais droppé. =====

CREATE TABLE IF NOT EXISTS token_outcome (
    id            INTEGER PRIMARY KEY,
    mint          TEXT NOT NULL,
    label         TEXT NOT NULL,        -- 'lp_pulled'|'price_zero'|'dev_dumped' ; jamais 'alive'
    label_class   TEXT NOT NULL,        -- 'terminal' | 'event'
    observed_slot INTEGER NOT NULL,
    final         INTEGER NOT NULL DEFAULT 0,  -- finalité anti-fork : 1 après 32 slots empilés
    UNIQUE(mint, label, observed_slot)
) STRICT;
-- Maturité non stockée : 'alive' dérivé = aucun label terminal final=1 ET âge >= 24h.

-- trade_outcome : actions exécutées par le bot de snipe, ingérées en batch depuis
-- le journal append-only (fichier séparé, HORS de cette base). FACT-LIKE : un trade
-- est un fait irréversible et non réobservable (pas de backfill), du même côté que
-- token_outcome. Jamais droppée/vidée par reset_recomputable, jamais versionnée par
-- method_version. Rapprochée de score_prediction (prédit) pour mesurer l'écart
-- prédit/réalisé.
CREATE TABLE IF NOT EXISTS trade_outcome (
    id               INTEGER PRIMARY KEY,
    mint             TEXT NOT NULL,
    cluster_id       INTEGER,            -- profil jugé au moment de l'action (nullable)
    prediction_id    INTEGER,            -- lien vers score_prediction.id : prédit vs réalisé (nullable)
    action           TEXT NOT NULL,      -- 'buy'|'sell'|'skip'|'blocked'
    reason           TEXT,               -- ex. 'honeypot_check_failed','slippage_exceeded' (nullable)
    amount_sol       INTEGER,            -- lamports engagés (nullable si skip/blocked)
    pnl_lamports     INTEGER,            -- résultat réalisé ; NULL si position ouverte ou non exécuté
    bot_slot         INTEGER NOT NULL,   -- slot de l'action côté bot
    ingested_unix_ms INTEGER NOT NULL    -- quand l'analyseur l'a avalé du journal
) STRICT;

-- ===== DÉRIVÉ RECALCULABLE — droppé/recréé par reset_recomputable. =====

CREATE TABLE IF NOT EXISTS cluster (
    id             INTEGER PRIMARY KEY,
    anchor_wallet  TEXT NOT NULL,
    method_version INTEGER NOT NULL,
    updated_slot   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS cluster_member (
    cluster_id    INTEGER NOT NULL,
    wallet        TEXT NOT NULL,
    link_type     TEXT NOT NULL,        -- 'consolidation'|'funding'|'exclusivity'|'cobehavior'
    link_strength REAL NOT NULL,
    PRIMARY KEY (cluster_id, wallet)
) STRICT;

CREATE TABLE IF NOT EXISTS cluster_profile (
    cluster_id      INTEGER PRIMARY KEY,
    token_count     INTEGER NOT NULL DEFAULT 0,
    rug_count       INTEGER NOT NULL DEFAULT 0,
    beta_alpha      REAL NOT NULL,
    beta_beta       REAL NOT NULL,
    risk            REAL,
    confidence      REAL,
    last_decay_slot INTEGER,
    is_rugger       INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS score_prediction (
    id             INTEGER PRIMARY KEY,
    cluster_id     INTEGER NOT NULL,
    mint           TEXT NOT NULL,
    risk           REAL NOT NULL,
    confidence     REAL NOT NULL,
    method_version INTEGER NOT NULL,
    predicted_slot INTEGER NOT NULL
) STRICT;

-- passthrough_node : mixte. Lignes source='seed' = denylist permanente (jamais droppée) ;
-- lignes source='auto' = détectées, supprimées par reset_recomputable.
CREATE TABLE IF NOT EXISTS passthrough_node (
    address       TEXT PRIMARY KEY,
    label         TEXT,
    source        TEXT NOT NULL,        -- 'seed'|'auto'
    degree        INTEGER,
    heterogeneity REAL,
    decided_slot  INTEGER
) STRICT;

-- ===== FILE D'ANALYSE — propriété exclusive analyseur, état opérationnel, jamais droppée. =====

CREATE TABLE IF NOT EXISTS analysis_queue (
    entity          TEXT NOT NULL,
    entity_kind     TEXT NOT NULL,      -- 'deployer'|'cluster'
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending|analyzing|done|error
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    enqueued_slot   INTEGER NOT NULL,
    updated_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (entity, entity_kind)
) STRICT;

CREATE INDEX IF NOT EXISTS ix_queue_status ON analysis_queue(status);
