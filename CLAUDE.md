# CLAUDE.md

Guide pour travailler dans ce dépôt. À lire **en entier** avant toute modification.

---

> **CONTEXTE CRITIQUE — lis ceci en premier.**
>
> Ce fichier est destiné à **UN développeur unique** qui travaillera **SEUL** sur ce
> dépôt, avec sa propre instance de Claude Code, sans aucun accès à l'historique ni à
> la mémoire de projet d'où vient le contexte ci-dessous. Ce développeur possède
> **AUSSI le bot de snipe externe** (dépôt séparé) : il est donc la seule personne au
> monde qui détient le contexte du bot. Il prendra **TOUTES** les décisions lui-même,
> à partir de ce que son Claude Code lui proposera en lisant ce fichier.
>
> **Conséquences pour la rédaction :**
> - Il n'existe **PAS** de « propriétaire » tiers à qui remonter quoi que ce soit.
>   Ne crée jamais de décision « en attente de validation externe ». Chaque décision
>   ouverte doit être tranchable **PAR LUI**, et ce fichier doit lui donner de quoi
>   la trancher.
> - Ce fichier ne doit pas seulement **lister** les décisions ouvertes : il doit
>   **enseigner** comment chacune se tranche (défaut recommandé + protocole de test +
>   signal d'échec). Un développeur seul face à une décision sans méthode choisit au
>   hasard ; c'est ce qu'il faut éviter.

---

## Projet

Système d'intelligence pour le sniping de memecoins Solana, écrit en Rust.
Le dépôt est en **Phase 1** : il contient la **couche de persistance SQLite** plus
**deux composants de traitement** — l'**ingestor** (écrit les faits terrain) et
l'**analyseur** (calcule clusters, profils, scores). La logique de trading (le bot
de snipe lui-même) reste hors de ce dépôt ; ici on s'arrête à produire les
prédictions et à enregistrer les `trade_outcome` qu'il remonte.

> Historique : la Phase 0 ne contenait QUE la persistance. La contrainte « aucune
> logique métier ici » a été **levée** pour accueillir ingestor + analyseur. Les
> invariants de **schéma** ci-dessous restent, eux, intégralement verrouillés.

## Structure

```
JOURNAL.md          Journal de bord OPÉRATIONNEL : tout ce qui est câblé autour de la base
                    (ingestor/backfill/analyseur/bot, déploiement 24/7). Le pendant pratique
                    de la §10 (qui, elle, garde les décisions). À lire pour la prod.
db/schema.sql       Source de vérité UNIQUE du schéma (DDL, commentaires inclus).
db/seed_passthrough.sql  Denylist passthrough permanente (source='seed'), appliquée à l'init.
src/lib.rs          Déclare les modules db / ingest / analyze + helper now_unix_ms().
src/db/mod.rs       init() + reset_recomputable(). Lit/applique schema.sql puis seed_passthrough.sql.
src/ingest/mod.rs   Ingestor : écrit BRUT + FACT-LIKE (append-only, idempotent).
src/analyze/mod.rs  Analyseur : clustering, profil bayésien, scoring, passthrough, file.
src/bin/analyze.rs  CLI : exécute un tick d'analyse sur une base et affiche le bilan.
tests/schema.rs     Intégrité, tables, WAL, STRICT, frontière de drop.
tests/ingest.rs     Append-only, idempotence, invariant 'alive', atomicité du batch.
tests/analyze.rs    Clustering, profil, scoring, passthrough, recalculabilité post-reset.
Cargo.toml          rusqlite (feature `bundled`, synchrone — PAS sqlx) + tempfile en dev-dep.
```

## Commandes

```bash
cargo build              # compile (rusqlite bundled, premier build ~15 s : SQLite en C)
cargo test               # lance tous les tests d'intégration (tout doit être vert)
cargo clippy --all-targets   # lint (doit rester sans avertissement)
cargo run --bin analyze [chemin.db]   # un tick d'analyse (défaut : intel.db)
```

## Frontière de propriété des écritures (composants)

- **Ingestor** (`src/ingest`) : écrit UNIQUEMENT BRUT (`raw_*`) et FACT-LIKE
  (`token_outcome`, `trade_outcome`). Jamais de table dérivée, jamais `analysis_queue`.
- **Analyseur** (`src/analyze`) : écrit UNIQUEMENT les tables DÉRIVÉ RECALCULABLE,
  les lignes `passthrough_node source='auto'`, et `analysis_queue`. Il LIT les tables
  BRUT/FACT-LIKE mais ne les modifie jamais.

## API exposée

### `src/db/mod.rs`
- `init(path: &str) -> Result<Connection>` : ouvre/crée la base, applique les PRAGMAs
  (`journal_mode=WAL`, `busy_timeout=5000`, `synchronous=NORMAL`), le schéma, **puis** le
  seed de la denylist passthrough (`db/seed_passthrough.sql`, lignes `source='seed'`).
  **Idempotent** — sûr à rappeler à chaque démarrage (le seed est en `INSERT OR IGNORE`).
- `reset_recomputable(conn: &Connection) -> Result<()>` : DROP/recrée UNIQUEMENT les tables
  recalculables et purge `passthrough_node WHERE source='auto'`. Transactionnel. Ne touche
  jamais les lignes `source='seed'` et ne réapplique pas le seed (inutile : jamais supprimé).

Ni le DDL ni les adresses ne sont **jamais** écrits en dur dans le Rust : `schema.sql` et
`seed_passthrough.sql` sont embarqués via `include_str!` (lecture au build) et restent les
seules sources de vérité.

### `src/ingest/mod.rs`
- Structures d'entrée : `TokenLaunch`, `WalletFlow`, `LaunchParticipant`, `TokenOutcome`,
  `TradeOutcome`, `EventBatch`.
- `record_launch / record_flow / record_participant / record_outcome -> Result<bool>` :
  `INSERT OR IGNORE`, renvoient `true` si la ligne était nouvelle. `record_trade -> Result<i64>`
  (journal append-only). `ingest_batch -> Result<BatchCounts>` (lot atomique, une transaction).
- `record_outcome` **refuse `label == "alive"`** (`IngestError::ForbiddenLabel`) — l'invariant
  est gardé en code, avant écriture.
- Idempotence des flux : garantie pour un flux de token (`mint` renseigné) ; un flux SOL natif
  (`mint = None`) n'est PAS dédupliqué car SQLite traite `NULL != NULL` dans `UNIQUE`.

### `src/analyze/mod.rs`
- `run_once(conn) -> Result<AnalysisReport>` : un tick complet
  (passthrough → enqueue deployers → analyse). Idempotent.
- `detect_passthrough`, `enqueue_new_deployers`, `analyze_deployer` exposés pour usage ciblé.
- `METHOD_VERSION` : version de méthode estampillée sur `cluster` / `score_prediction`.
- Clustering (`METHOD_VERSION = 2`) : 2 types **fondateurs** (`funding` = deployer→wallet,
  `consolidation` = wallet→deployer) tirés uniquement des flux de fonds
  (`raw_wallet_flow`) — seuls ces liens créent l'appartenance à un cluster. La
  co-participation (`raw_launch_participant`) est un **bonus corroboratif** borné
  (`COBEHAVIOR_BONUS_MAX × share`, plafonné à `COBEHAVIOR_STRENGTH_CAP`) qui renforce
  la force d'un lien déjà fondé, jamais le type ni l'appartenance elle-même. Profil :
  modèle Beta-Bernoulli du taux de rug (`risk` = moyenne a posteriori, `confidence`
  croît avec `token_count`). Un rug = un `token_outcome` terminal `final=1`.

## Schéma — 11 tables, classées par cycle de vie

Toutes les tables sont en mode **STRICT**. Le schéma n'a **volontairement aucune clé
étrangère**. Les adresses/wallets/mint sont en **TEXT** (base58), jamais BLOB.

| Catégorie | Tables | Droppée par `reset_recomputable` ? |
|---|---|---|
| **BRUT** (append-only, vérité terrain) | `raw_token_launch`, `raw_wallet_flow`, `raw_launch_participant` | **Jamais** |
| **FACT-LIKE** (faits non réobservables) | `token_outcome`, `trade_outcome` | **Jamais** |
| **FILE D'ANALYSE** (état opérationnel) | `analysis_queue` | **Jamais** |
| **DÉRIVÉ RECALCULABLE** | `cluster`, `cluster_member`, `cluster_profile`, `score_prediction` | **Oui** (drop/recrée) |
| **MIXTE** | `passthrough_node` | Lignes `source='auto'` supprimées ; `source='seed'` conservées |

## Invariants verrouillés — NE PAS enfreindre

Toutes les décisions de schéma sont **déjà prises et verrouillées**. On les implémente,
on ne les rediscute pas. Si une décision paraît discutable : la signaler en commentaire,
ne pas la changer. Si quelque chose est réellement ambigu/contradictoire : s'arrêter et
demander avant de coder.

### Frontière de DROP (critique)
`reset_recomputable` agit **uniquement** sur : `cluster`, `cluster_member`,
`cluster_profile`, `score_prediction`, et `DELETE FROM passthrough_node WHERE source='auto'`.
Ne JAMAIS dropper/vider : `raw_token_launch`, `raw_wallet_flow`, `raw_launch_participant`,
`token_outcome`, `trade_outcome`, `analysis_queue`, ni `passthrough_node` source='seed'.
Ces données sont des faits observés non réobservables (aucun backfill possible) —
les effacer casse la boucle d'apprentissage.

### Interdictions explicites
- **Aucun index** sur `raw_wallet_flow` (ni `src`, ni `dst`). Ajout seulement si mesuré nécessaire.
- **Aucune clé étrangère**.
- **Ne changer aucun type** de colonne (adresses en TEXT, pas BLOB).
- **Ne pas ajouter/renommer/supprimer** de colonne.
- **Jamais `'alive'`** comme valeur de `token_outcome.label` : c'est une conclusion dérivée
  à la volée (aucun label terminal `final=1` ET âge ≥ 24 h), jamais stockée.
- **Ne pas dupliquer le DDL** dans le Rust : le Rust lit/applique `schema.sql`.
- `foreign_keys` laissé par défaut (OFF) : voulu.

## Modifier le schéma

1. Éditer `db/schema.sql` (en `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`,
   mode STRICT). C'est le seul endroit où vit le DDL.
2. Si la nouvelle table est recalculable, l'ajouter à `RECOMPUTABLE_TABLES` dans
   `src/db/mod.rs`. Si elle est BRUT / FACT-LIKE / file d'analyse, **ne pas l'y ajouter**.
3. Étendre `tests/schema.rs` : présence de la table, mode STRICT, et côté de la frontière
   de drop (subsiste vs vidée).
4. Si la table est lue/écrite par un composant, respecter la frontière de propriété
   (ingestor = BRUT/FACT-LIKE ; analyseur = dérivé + `analysis_queue`) et étendre
   `tests/ingest.rs` ou `tests/analyze.rs`.
5. `cargo test` — tout doit rester vert.

---

## 6 — Décisions à trancher

**Deux natures de décision — lis avant d'agir :**

**(A) ARCHITECTURE / INTERFACE** — se tranche par cohérence avec la topologie figée et,
pour tout ce qui touche le bot, par confrontation avec **le dépôt du bot que TU possèdes**.
Aucune mesure possible a priori ; c'est un choix de conception à faire **TÔT** car il
conditionne le reste.

**(B) CALIBRATION / OPS** — se tranche par la **mesure sur données réelles**. Ne pas figer
avant d'avoir la donnée ; utiliser la valeur par défaut indiquée en attendant.

| # | Décision | Options | Défaut si tu ne sais pas | Comment trancher (critère concret) | Signal que tu t'es trompé |
|---|---|---|---|---|---|
| A1 | **Source ingestor** ⚠️ LA PLUS IRRÉVERSIBLE — pas de backfill, ce qui n'est pas capturé est perdu définitivement. À trancher AVANT d'écrire une ligne de code réseau. | Jito ShredStream gRPC `SubscribeEntries` (décision figée de la topologie) vs Geyser (suggéré par un commentaire trompeur du code) | **ShredStream** | Vérifier que la topologie du bot appelle ShredStream. Si le bot reçoit déjà des entries pré-confirmation via Jito, utiliser la même source garantit la cohérence temporelle. | Gaps de séquence massifs dans `raw_wallet_flow.slot` ; ou intentions pré-confirmation comptées comme transactions settlées (fausse finalité). |
| A2 | **Jonction avec le bot** ⚠️ À FAIRE EN PREMIER — sans cette vérification tout le reste peut être inutile. | Champs de `score_prediction` + `cluster_profile` tels qu'ils sont vs adapter le schéma/l'algo pour matcher ce que le bot lit | **Aucun défaut** — tu DOIS ouvrir le dépôt du bot et comparer | Lister les champs que le bot consomme depuis sa lecture en cache RAM. Les confronter colonne par colonne aux tables produites. Documenter le mapping dans la section 10. | Le bot ne trouve pas un champ (KeyError / champ NULL), ou il interprète `risk` dans une échelle différente (ex. [0,100] vs [0,1]). |
| A3 | **Asymétrie de coût rugger** : faux alive (perte de capital) est bien plus coûteux qu'un faux rug (manque à gagner). Le seuil symétrique `RISK_THRESHOLD=0.5` ne le capture pas. | Seuil asymétrique plus élevé (ex. 0.7) ; exiger un `label terminal final=1` avant tout achat ; s'appuyer sur le blanchiment lent (Lot 2) | **Attendre Lot 2 + ne jamais traiter un cluster sans historique suffisant comme sûr** (`MIN_SAMPLES_FOR_RUGGER` est là pour ça) | Ouvrir le dépôt du bot : comment classe-t-il un cluster sans historique ? Si le bot passe outre `is_rugger=0` sans vérifier `token_count`, c'est une faille. Corriger côté bot ou ajouter un champ `maturity_flag`. | Le bot achète des tokens d'un deployer dont le cluster a `token_count < MIN_SAMPLES` et qui rugge peu après. |
| B1 | **`MATURITY_MS` (défaut 24 h)** : fenêtre en dessous de laquelle un token sans label terminal est comptabilisé comme « alive » à la volée. | 24 h (valeur actuelle) vs recaler sur le « genou » de la distribution temps-jusqu'au-rug | **24 h** jusqu'à avoir la donnée | Accumuler des `token_outcome` réels, tracer l'histogramme du délai `launch_slot → observed_slot` du premier label terminal. Prendre le percentile 90. | Trop court → des rugs en cours comptés comme alive, profil de risque sous-estimé. Trop long → blanchiment jamais atteint, tous les clusters restent suspects indéfiniment. |
| B2 | **Seuils et priors** (`RISK_THRESHOLD`, `MIN_SAMPLES_FOR_RUGGER`, `PASSTHROUGH_MIN_DEGREE`, `CONFIDENCE_K`, `COBEHAVIOR_BONUS_MAX`, `PRIOR_ALPHA/BETA`) | Valeurs actuelles du code vs valeurs calibrées sur données réelles | **Valeurs actuelles** — ne pas les changer dans le vide | Constituer un eval set de 20-30 deployers étiquetés à la main (rugger / pas rugger, source : données historiques on-chain). Faire varier chaque paramètre, mesurer précision + rappel. Enregistrer la courbe précision/rappel dans la section 10. | Trop de faux positifs (clusters sains bloqués, le bot ne snipe rien) ou trop de faux négatifs (ruggers non détectés, pertes répétées). |
| B3 | **`N_final = 32 slots`** (finalité anti-fork dans `token_outcome.is_final`) | 32 slots (valeur actuelle) vs valeur empirique | **32** | Sur un échantillon de données réelles, vérifier qu'aucun label `final=1` posé à slot S ne s'inverse après S+32. Si un reorg est observé, augmenter. | Un label terminal `final=1` est contredit par un bloc postérieur (reorg réel observé). |
| B4 | **Filtre ingestor** : quels flux écrire dans `raw_wallet_flow` | Statique (program IDs fixes : pump.fun, PumpSwap, DEX connus) vs dynamique (HashSet de wallets surveillés, rafraîchi depuis l'analyseur) | **Statique d'abord** pour valider le pipeline, dynamique ensuite | Mesurer le taux de liens de consolidation manqués (wallet non connu au moment de la tx, son financement est dans le passé). Si le taux est élevé, activer le lookup RPC rétroactif côté analyseur (Lot 4), pas le filtre. | Clusters fragmentés alors que les wallets sont visiblement liés on-chain ; liens de consolidation absents de `raw_wallet_flow` pour des transactions publiques. |
| B5 | **Granularité du décodage dans l'ingestor** avant écriture `raw_*` (latence hot-path vs volume) | Décoder le minimum (mint, src, dst, slot, amount) vs décoder davantage de champs dès l'entrée | **Décoder le minimum** — on peut toujours enrichir plus tard, on ne peut pas défaire une latence produite | Mesurer le temps de traitement par entry sous charge réelle (Jito produit ~50k entries/s en pointe). Si le hot-path est sous 100 µs/entry, le décodage est neutre. | Temps de traitement par entry > 200 µs sous charge, retard de la file ShredStream, slots manqués. |
| B6 | **Modèle Claude pour le jugement qualitatif de l'analyseur** (Lot futur) | claude-sonnet-4-6 via Batch API + prompt caching vs autre modèle | **claude-sonnet-4-6** | Construire l'eval set (voir B2). Comparer les verdicts du modèle aux étiquettes manuelles. Figer chaque verdict avec la version du modèle (champ `method_version` sur `score_prediction`) pour reproductibilité de la calibration. | Verdicts incohérents entre deux runs sans changement de données ; ou précision < 70 % sur l'eval set. |
| B7 | **Infra : Hetzner Frankfurt vs Falkenstein** (cible AX42 bare metal) | Frankfurt (latence Jito légèrement meilleure selon la géographie du PoP) vs Falkenstein | **AX42 en provisoire**, à valider avant prod | Lancer deux instances en parallèle 24 h, comparer : `nstat -az | grep RcvbufErrors` (overflows UDP), `mpstat -P ALL 5` (CPU par core), taux de croissance de la DB. Choisir celle avec le moins d'overflow. | RcvbufErrors > 0 en continu → l'instance perd des entries. Croissance DB anormalement faible → des flux sont silencieusement ignorés. |
| B8 | **Index `(src,dst)` sur `raw_wallet_flow`** (interdit par invariant, levable si mesuré) | Pas d'index (défaut verrouillé) vs index ajouté après mesure | **Pas d'index** — ne pas ajouter avant mesure | UNIQUEMENT si `detect_passthrough` ou `collect_members` deviennent un goulot mesuré (ex. > 500 ms par tick sur une DB de production). Mesurer avec `EXPLAIN QUERY PLAN` + `sqlite3_profile`. Documenter la mesure en section 10 avant d'ajouter l'index. | `detect_passthrough` ou `collect_members` dépasse 500 ms sur la DB de production (> 1M lignes `raw_wallet_flow`). |

---

## 6bis — Comment trancher quand tu es seul

Méthode générale, à appliquer à toute décision de ce fichier ou que Claude Code te soumet :

**1. Regarde la nature de la décision.**
- **(A) Architecture/interface** → tranche par cohérence avec la topologie (section 2 et le bot) et, si ça touche le bot, en **ouvrant le dépôt du bot**. Fais-le **tôt**.
- **(B) Calibration/ops** → ne tranche **pas** dans le vide. Pose le défaut recommandé, continue, et reviens trancher quand tu as la donnée réelle.

**2. Avant de figer une décision (A) irréversible** (au premier chef : la source de l'ingestor), écris en une phrase ce que tu perdrais si tu te trompais. Si la réponse est « des données impossibles à récupérer », traite-la comme **bloquante** et prends le temps de vérifier.

**3. Quand tu tranches, trace-le.**
Date + choix + raison, dans la section 10 (trace de décision). Si le choix engage le schéma → reporte-le dans CLAUDE.md. S'il engage l'algo → commente-le dans le code avec `// DÉCISION :`. Une décision non tracée sera re-débattue dans un mois.

**4. Ordre conseillé pour démarrer :**
- **(a) Jonction bot (A2) en premier** — gratuit, et peut invalider des semaines de travail.
- **(b) Lot 2 ensuite** — pur, sans réseau, implementable immédiatement.
- **(c) Puis trancher A1 (source ingestor)** et attaquer le Lot 3 (ingestor réseau).

---

## 10 — Trace de décision

> Date + choix + raison. Une décision non tracée sera re-débattue dans un mois.

### 2026-06-27 — A1 : source ingestor = **Jito ShredStream** (FIGÉE)
Cohérent avec la topologie et avec le bot (qui consomme déjà des entries
pré-confirmation via Jito). Le Lot 3 (ingestor réseau) est implémenté dans un dépôt
séparé `matbolze/solana-memecoin-ingestor` (workspace Rust : décodeur pur
`pump-create-decode` + binaire `ingestor` + coque gRPC `ingestor-shredstream`). Il
dépend de cette lib (`solana_memecoin_db::ingest`, rev `d6d5a43`) et écrit le BRUT.
État : décodage `create` + co-acheteurs (`buy` standard, acheteur=compte[6]) prouvés
par golden tests ; mapping → `ingest_batch` testé contre une vraie base ; coque tonic
compile, **à valider au 1er run VPS**. ⚠️ L'ingestor n'écrit PAS encore
`raw_wallet_flow` (Scope A) → le clustering n'a pas de liens fondateurs tant que ce
n'est pas réglé (cf. B4 / Lot 4 — décision à trancher ensuite).

### 2026-06-27 — A2 : jonction analyseur → bot (RÉSOLUE)
Confrontation des champs produits (`cluster_profile`) vs consommés par le gate du bot
(`entry_gate` → `rugger_graph.Verdict` / `dev_analyzer`). Jonction implémentée et
testée **côté bot** (`src/analysis/cluster_cache.py` + `src/trading/entry_gate.py`,
13 tests verts). 3 pièges tranchés :

- **Échelle** : `risk` est en **[0,1]** (moyenne Beta) ; le gate pense en **0-100**.
  → conversion `round(risk*100)` AU point de jonction. Aucune valeur [0,1] ne fuit
  dans le gate. Seuils alignés (0.5 ↔ 50).
- **Seuil = politique du BOT** : le bot lit `risk`/`confidence`/`token_count` bruts et
  applique SON seuil (coût asymétrique faux-alive ≫ faux-rug, cf. A3). `is_rugger`
  pré-calculé n'est qu'un indice, jamais une vérité aveugle.
- **Garde A3 (token_count)** : un cluster `risk≥seuil` mais `token_count<MIN_SAMPLES`
  n'est **ni rugger ni « sûr »** → reste « inconnu » côté gate (entrée conditionnée au
  momentum). `is_cluster_rugger` impose `token_count≥MIN_SAMPLES` ET `risk≥seuil`.
- **Sens / périmètre** : l'analyseur n'alimente QUE le côté rugger ; jamais de
  « trusted ». La whitelist `good_devs` du bot reste curée et séparée.

**Mapping du cache RAM** (lookup t=0, clé = wallet du créateur) :
`cluster_member ⋈ cluster_profile` **∪ `cluster.anchor_wallet ⋈ cluster_profile`**
(les deployers ne sont PAS dans `cluster_member`). **PAS `score_prediction`** (par-mint,
calculé après analyse → un mint neuf n'y est pas au t=0). Wallet dans plusieurs
clusters → on garde le plus risqué (prudence). Lecture **seule** (`PRAGMA
query_only=1`), rafraîchie toutes les `CACHE_REFRESH_SECONDS`, swap atomique ; base
analyseur absente → fallback propre. Câblé au démarrage du bot
(`main.py : cluster_cache.start_background()`).

### 2026-06-27 — Lot 3 : ingestor réseau ShredStream (VALIDÉ EN LIVE)
Coque gRPC tonic du dépôt `matbolze/solana-memecoin-ingestor` durcie (reconnexion à
backoff + writer SQLite découplé d'un canal borné = backpressure propre, exigences A1)
puis **validée sur le VPS** contre le proxy Jito local (`127.0.0.1:9999`) : connexion +
abonnement + écriture OK, **21 launches réels** écrits dans `raw_token_launch` (+
co-acheteurs), `[gaps] vus=3000 manquants=0` (séquence de slots parfaite → le risque A1
d'incohérence temporelle est écarté). `raw_wallet_flow` reste à 0 (Scope A) → traité par
B4 ci-dessous.

### 2026-06-27 — B4 : flux fondateurs `raw_wallet_flow` par backfill RPC (RÉSOLUE — gap fermé)
`collect_members` ne fonde l'appartenance que sur `funding` (src=deployer) et
`consolidation` (dst=deployer) tirés de `raw_wallet_flow`. L'ingestor live (Scope A)
n'écrit que launches+co-acheteurs → ces flux manquaient → clusters creux. **Tranché :
backfill RPC rétroactif** (option « dynamique » de B4), car les arêtes de financement
sont ANTÉRIEURES à la vue live du deployer (jamais captables en live).

Implémenté **côté bot, en Python** (pas en Rust) pour deux contraintes dures : budget
Helius fini (réutilise le client RPC déjà discipliné en crédits du bot, comme
`analyze_dev`) et VPS 2 cœurs latence-critique (pas de pile TLS Rust à recompiler sur la
prod). Tient le **rôle ingestor** (écrit UNIQUEMENT `raw_wallet_flow`, `INSERT OR IGNORE`,
`kind='sol'`, `mint=NULL`) → frontière de propriété respectée ; l'analyseur Rust lit
ensuite ces flux. Bornage (crédits) : 1 `getSignaturesForAddress` + ≤`BACKFILL_TX_MAX`
(déf. 6) `getTransaction` par deployer, ≤`BACKFILL_MAX_DEPLOYERS`/run, sleep entre appels.
Idempotence au niveau deployer via base d'état **séparée** (`backfill_state.db`, hors
schéma partagé) — indispensable car un flux SOL natif (`mint=NULL`) n'est PAS dédupliqué
par `UNIQUE(sig,src,dst,mint)` (NULL != NULL côté SQLite).

**Validé LIVE** : 18 deployers réels (de l'intel.db rempli par Lot 3) → **308 flux
fondateurs écrits** (~126 appels RPC), arêtes structurellement correctes (incidentes au
deployer, funding priorité-1).

LIMITES connues (calibration, NON bloquant) : (1) on prend les `TX_MAX` signatures les
**plus anciennes** = financement d'origine pour un wallet frais (cas rugger typique) ; un
wallet réutilisé à long historique peut avoir son financement initial au-delà de
`limit=1000` → on capte alors ses funding-out récents (liens priorité-1 valides, pas du
bruit). (2) Les micro-transferts (fees pump / tips Jito ~0.0003 SOL) sont écrits tels
quels : leur exclusion est le rôle du **denylist passthrough** (seed + détection auto
fan-in/out), PAS de l'ingestor (qui écrit des faits) → à surveiller en B2.

Fichiers bot : `src/analysis/flow_backfill.py` (cœur pur testé + worker),
`scripts/flow_backfill_worker.py` (CLI `--loop`), `tests/test_flow_backfill.py` (6 tests).
**Reste** : déployer l'analyseur Rust sur le VPS (maillon flux→cluster→profil→cache bot,
aujourd'hui le seul chaînon non encore exécuté en prod) et planifier le worker
(cron/systemd, état persistant hors conteneur éphémère).
