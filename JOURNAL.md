# JOURNAL DE BORD — la base de données en production

> But de ce fichier : raconter **tout ce qui a été câblé AUTOUR de cette base** (qui
> l'alimente, qui la lit, comment, où, avec quels commits). Le `CLAUDE.md` contient les
> *décisions* (le pourquoi, verrouillé) ; ce journal contient l'*opérationnel* (le quoi/où,
> chronologique). Lis-le pour comprendre comment ta base vit hors de ce dépôt.
>
> ⚠️ Cette base est le **point de rendez-vous** de 3 composants répartis sur 2 autres dépôts.
> Ce dépôt ne contient que la persistance + l'analyseur. L'ingestor et le bot vivent ailleurs
> (voir la carte ci-dessous) mais écrivent/lisent CETTE base.

---

## 1. Carte du système (qui est où)

| Composant | Dépôt | Langage | Rôle vis-à-vis de CETTE base |
|---|---|---|---|
| **Persistance + Analyseur** | `terrafinafilippo-ship-it/Base-donnees-SQLite` (CE dépôt) | Rust | crée le schéma ; l'analyseur ÉCRIT les tables dérivées, LIT le brut |
| **Ingestor réseau** | `matbolze/solana-memecoin-ingestor` (privé) | Rust | ÉCRIT le BRUT (`raw_token_launch`, `raw_launch_participant`) depuis ShredStream |
| **Bot de snipe** | `matbolze/bot-sniping`, branche `feat/trading-terminal` | Python | ÉCRIT `raw_wallet_flow` (backfill RPC) ; LIT `cluster_profile` (cache t=0) |

Tous reliés UNIQUEMENT par le fichier SQLite partagé (WAL). Aucun couplage direct.

---

## 2. Le câblage de la base — qui écrit / lit quoi

Respect strict de la **frontière de propriété** (cf. CLAUDE.md §66) :

| Table | Écrite par | Lue par | Comment |
|---|---|---|---|
| `raw_token_launch` | Ingestor Rust (daemon ShredStream) | Analyseur, backfill, outcome-tracker | `ingest_batch`, append-only idempotent |
| `raw_launch_participant` | Ingestor Rust (co-acheteurs) | Analyseur | idem |
| `raw_wallet_flow` | **Backfill Python** (rôle ingestor) | Analyseur (`collect_members`, `detect_passthrough`) | `INSERT OR IGNORE`, `kind='sol'`, `mint=NULL` |
| `passthrough_node` (seed) | seed appliqué à `init` | Analyseur | 46 hubs (denylist permanente) |
| `cluster`, `cluster_member`, `cluster_profile`, `score_prediction` | **Analyseur Rust** (`run_once`) | **Bot** (cache RAM `cluster_profile`) | dérivé recalculable |
| `token_outcome` | ⚠️ **PERSONNE encore** (voir §6) | Analyseur (`compute_profile.rug_count`) | — |
| `trade_outcome` | (futur : journal du bot) | Analyseur | — |

> Note importante : un 4ᵉ flux (le « côté gagnant », détection des BONS devs par graduation)
> tourne aussi, mais il **n'écrit PAS dans cette base** — voir §5.4 pour pourquoi.

---

## 3. Le trajet de la donnée (vue d'ensemble)

```
 Réseau Jito ─► jito-shredstream-proxy (VPS :9999) ─► entries (push gRPC, ~100-500ms avant le bloc)
                                                          │  (fan-out : 2 abonnés)
         ┌────────────────────────────────────────────────┴───────────────┐
         ▼                                                                  ▼
   BOT (Python, hot path)                                   INGESTOR (Rust daemon)
   décode → décide achat/skip (O(1) RAM)                    décode → écrit le BRUT
         ▲                                                          │ raw_token_launch
         │ lookup t=0                                               │ raw_launch_participant
         │                                                          ▼
   cluster_cache (RAM) ◄──── cluster_profile ◄──── ANALYSEUR (Rust, /5min) ◄──── intel.db
                                                   clusters + risque bayésien      ▲
                                                                                   │ raw_wallet_flow
                                                              BACKFILL (Python, /10min, Helius RPC)
                                                              « qui a financé ce deployer ? »
```

---

### 3bis — Précisions (questions fréquentes)

- **Le bot lit la DB, PAS l'analyseur directement.** Les 3 composants ne se parlent jamais en
  direct : ils communiquent UNIQUEMENT via ce fichier SQLite. L'analyseur ÉCRIT ses résultats
  (`cluster_profile`…) dans la base ; le bot les LIT (chargés en RAM via son cache, lecture
  `PRAGMA query_only=1`). Découplage total → on peut redémarrer/remplacer l'un sans toucher
  l'autre. Le bot consomme donc « l'analyseur » **à travers la base**.

- **Transport du flux : Jito→proxy = UDP, proxy→ingestor = gRPC.** Les *shreds* bruts arrivent
  de Jito en paquets **UDP** (multicast). Le `jito-shredstream-proxy` (binaire séparé, sur le
  VPS) les reçoit, reconstruit les `Entry`, et les re-sert aux abonnés locaux (bot + ingestor)
  en **gRPC streaming** (`SubscribeEntries`, HTTP/2, `127.0.0.1:9999`). **L'ingestor reçoit du
  gRPC** ; l'UDP est en amont, dans le proxy. (Inclusion ≠ exécution : ces entries sont
  pré-confirmation, certaines tx échouent au bloc — voir A1.)

## 4. Journal chronologique — 2026-06-27

### 4.1 — Seed passthrough (denylist permanente)
46 adresses hubs vérifiées ajoutées dans `db/seed_passthrough.sql` (`source='seed'`, jamais
droppées). Test `reset_recomputable_respects_drop_frontier` corrigé : il supposait un seed
vide (`== 1`), passé à `count_seed_inserts() + 1`. **Commit `d6d5a43`.**

### 4.2 — Ingestor réseau (dépôt `matbolze/solana-memecoin-ingestor`)
Workspace Rust à 3 crates, `default-members` excluant la coque réseau (pour que `cargo test`
ne compile que le pur) :
- `pump-create-decode` : décodeur PUR du wire Solana (legacy + v0), dépendance unique `bs58`.
  Décode `create` (mint, creator, name/symbol/uri) ET les co-acheteurs (`buy` standard, disc
  `66063d1201daebea`, acheteur = compte[6]). 10 golden tests, **byte-parité prouvée** contre
  le décodeur Python du bot.
- `ingestor` (lib) : `entry::iter_transactions` (framing du blob `Vec<Entry>`),
  `pipeline::process_entries_blob` (blob → `EventBatch`, cœur pur), `sink` (mapping →
  `ingest::TokenLaunch`/`LaunchParticipant`), `gaps::SlotTracker` (trous de slot).
- `ingestor-shredstream` (coque réseau tonic) : client gRPC Jito `SubscribeEntries` →
  `process_entries_blob` → `ingest_batch`. **Durcie** (commit `05b18d7`) : boucle de
  **reconnexion** à backoff (un drop ne tue plus l'ingestion — exigence A1) + **writer
  SQLite découplé** d'un canal borné (l'écriture bloquante ne gèle plus le récepteur async
  sous burst Jito).
- Dépend de CE dépôt : `solana-memecoin-db` (git, rev `d6d5a43`).

**Validé EN LIVE sur le VPS** contre le proxy local `127.0.0.1:9999` : connexion +
abonnement + écriture OK, 21 launches réels écrits, **`[gaps] vus=3000 manquants=0`**
(séquence de slots parfaite → le risque A1 d'incohérence temporelle est écarté).

### 4.3 — A2 : jonction analyseur → bot (le bot LIT `cluster_profile`)
Côté bot (`src/analysis/cluster_cache.py`) : cache RAM lecture-seule (`PRAGMA query_only=1`)
de `cluster_member ⋈ cluster_profile` ∪ `cluster.anchor_wallet ⋈ cluster_profile`, lookup
O(1) au t=0, refresh périodique, swap atomique. 3 pièges tranchés : **échelle**
(`risk`[0,1] → score 0-100 converti côté bot), **sens** (alimente seulement le côté rugger),
**garde A3** (cluster `token_count < MIN_SAMPLES` = ni rugger ni sûr). 13 tests verts.
Décision tracée CLAUDE.md §10. **Commit bot `c659ebf`.**

### 4.4 — Lot 4 / B4 : `raw_wallet_flow` par backfill RPC (gap fermé)
`collect_members` ne fonde l'appartenance que sur `funding`/`consolidation` tirés de
`raw_wallet_flow`. L'ingestor live ne les voit pas (antérieurs au launch). → **backfill RPC
rétroactif** (option « dynamique » de B4). Implémenté côté bot Python (réutilise le client
RPC discipliné en crédits du bot + évite une pile TLS Rust sur le petit VPS), **rôle
ingestor** : écrit UNIQUEMENT `raw_wallet_flow`. Borné, idempotent au niveau deployer (état
SÉPARÉ `backfill_state.db`, car `mint=NULL` n'est pas dédupliqué par `UNIQUE`). **Validé
live : 18 deployers → 308 flux.** **Commit bot `3d429fe`.** Trace CLAUDE.md §10.

### 4.5 — Analyseur déployé sur le VPS (le maillon flux→cluster→profil)
Binaire `analyze` (CE dépôt) buildé sur le VPS. Premier run sur la base alimentée :
**18 clusters, 218 membres, 18 profils, 21 prédictions**, 46 passthrough seed appliqués.
Puis vérifié que le **cache du bot lit bien ces profils** : `cluster_cache.refresh()` → 221
wallets chargés, `get(ancre)` → profil réel, `is_cluster_rugger` correct (garde A3). 

➡️ **Boucle complète prouvée de bout en bout** : ShredStream → ingestor → backfill →
analyseur → cache bot.

### 4.6 — Trace des décisions
A1 (source = ShredStream, figée), A2 (résolue), Lot 3 (ingestor validé), B4 (backfill)
écrites dans `CLAUDE.md` §10. **Commit `88b1f49`.**

---

## 5. Déploiement 24/7 (le câblage opérationnel sur le VPS)

VPS Hetzner « shredstream », 2 cœurs. Bot latence-critique → tout le pipeline tourne en
**basse priorité** (`Nice`) pour ne jamais voler de CPU au hot path.

### 5.1 — Base partagée unique
`/root/astra-terminal/data/intel.db` — sur le **bind-mount** du conteneur `astra-terminal`
(= `/app/data/intel.db` à l'intérieur). Ainsi les binaires **host** (ingestor, analyzer) ET
les workers **conteneur** (backfill, outcomes, cache bot) lisent/écrivent le **même fichier**
(même inode, verrous SQLite WAL OK).

### 5.2 — 4 jobs systemd
| Unité | Type | Fréquence | Commande |
|---|---|---|---|
| `memecoin-ingestor.service` | daemon | continu (Restart=always, `Nice=10`) | `…/ingestor-shredstream` (host), `DB_PATH=…/intel.db`, endpoint `127.0.0.1:9999` |
| `memecoin-backfill.timer` | oneshot | 10 min | `docker exec astra-terminal python /app/scripts/flow_backfill_worker.py` |
| `memecoin-analyzer.timer` | oneshot | 5 min | `…/memecoin-analyzer/target/debug/analyze …/intel.db` (host) |
| `memecoin-outcomes.timer` | oneshot | 10 min | `docker exec astra-terminal python /app/scripts/outcome_worker.py` |

Binaires : ingestor `release` (`/root/solana-memecoin-ingestor/target/release/`), analyzer
`debug` (`/root/memecoin-analyzer/target/debug/`). Toolchain : rustup installé sur le host,
builds en `nice -n 19 -j1`.

### 5.3 — Persistance des fichiers Python côté conteneur
`cluster_cache.py`, `flow_backfill.py`, `outcome_tracker.py` + workers injectés dans
`astra-terminal` puis **bakés** : `docker commit` → image `astra-terminal:latest` (sauvegarde
de l'ancienne = `astra-terminal:pre-lot4` pour rollback). Le conteneur de trading **n'a pas
été redémarré** (choix prudence : c'est le conteneur live).

### 5.4 — Côté GAGNANT (hors de cette base — à savoir)
En parallèle du modèle rugger, un tracker repère les **MEILLEURS devs** et les **bons ruggers**
pour les sniper au bloc 0 (commits bot `dd4a3ad`, `6d1d820`, `3d23abd`, `86bdd6d`). Il lit
l'état de courbe via `getAccountInfo` (parse `complete`, `real_sol_reserves` → progression, et
le **market cap** = `vsol×supply/vtoken`), high-water mark par mint dans un store SÉPARÉ
(`outcomes.db`). Trois catégories, lookup RAM O(1) au t=0 :
- **ÉLITE** : a gradué (≥85 SOL) → snipe, peut tenir.
- **BON RUGGER** : atteint un mcap tradeable (~8-10K) de façon répétée même s'il rug → snipe
  pour un flip rapide (TP, ne pas tenir).
- garde d'historique (≥3 launches) symétrique de A3.

Résultats réels (24/7 sur ~2 jours) : **18k+ mints suivis, ~105 graduations, 16 devs élites
détectés** (ex. un dev à 48 launches / 6 graduations). **Bornage volume** : on priorise les
deployers récurrents (les seuls candidats), pas le firehose.

⚠️ **Pourquoi le côté gagnant N'ÉCRIT (presque) PAS dans cette base** : ton analyseur compte
TOUT `token_outcome` terminal comme un RUG ; écrire « graduated » ici flaggerait chaque bon
dev en rugger. Le côté gagnant vit donc côté bot. SEULE exception : voir §6 (le tracker écrit
les RUGS dans `token_outcome`).

---

## 6. État actuel & ce qui reste (côté CETTE base)

**Ce qui marche** : le brut est alimenté en continu (ingestor + backfill), l'analyseur tourne
toutes les 5 min et produit clusters/profils, le bot les consomme. Le pipeline est autonome.

**✅ LE gap est FERMÉ — `token_outcome` est désormais alimenté en temps réel.**

Historique : un 1er producteur basé sur l'échantillonnage de courbe (`price_zero` quand une
courbe pompe puis s'effondre) s'est avéré à **très faible recall** — sur pump.fun la courbe
**gèle** à l'abandon (le SOL ne draine pas) et un dump est trop rapide pour un sondage 10 min.
→ l'échantillonnage est le mauvais outil pour les rugs (gardé en best-effort, ~0).

**Le BON producteur `dev_dumped`, FAIT et VALIDÉ** : l'ingestor décode dans le flux ShredStream
la **vente standard pump.fun** (`detect_pumpfun_sells`, disc `sell`=sha256("global:sell"),
mint=compte[2], vendeur=compte[6], miroir du buy). Dans le writer (hors hot path), il compare
le vendeur au **créateur connu du mint** (lookup indexé `raw_token_launch.mint`=PK) ; si égal
→ `record_outcome` **dev_dumped** (terminal, `is_final=false` car pré-confirmation), puis pose
la **finalité anti-fork B3** (`UPDATE final=1 WHERE observed_slot <= slot-32`, throttlé 1/256
batches). Rôle FACT-LIKE → frontière respectée. Repo `matbolze/solana-memecoin-ingestor`.

**Mesuré en live** : ~311 ventes standard / 90 s (le `sell` standard est fréquent),
**~20 `dev_dumped` écrits / 90 s**, finalité OK (19/21 final=1), et l'analyseur compte enfin
les rugs (`rug_count>0`, un cluster `risk>0.5`). `is_rugger` reste 0 pour les dumpeurs à
`token_count<3` (garde A3 correcte) → bascule à 1 dès qu'un rugger RÉCURRENT (≥3 tokens) dumpe.

Conséquence : **le modèle de risque n'est plus inerte**. Lot 2 (blanchiment lent) et la
calibration B2/B6 deviennent possibles (il y a enfin du signal rug à décayer / calibrer).
Limite connue : les ventes derrière une ALT (v0) ou en variante `sell_v2` ne sont pas
décodées → recall partiel (suffisant : le signal est abondant).

**Mineurs** : raffiner `gaps` via le leader schedule ; micro-transferts fees/tips (à exclure
côté passthrough, B2) ; extraction acheteur `buy_v2` si besoin.

---

## 7. Pointeurs commits

| Dépôt | Commit | Quoi |
|---|---|---|
| CE dépôt | `d6d5a43` | seed passthrough (46) + fix test frontière |
| CE dépôt | `88b1f49` | CLAUDE.md §10 : traces A1/A2/Lot3/B4 |
| ingestor | `05b18d7` | coque ShredStream durcie (reconnexion + writer découplé) |
| bot | `c659ebf` | A2 — cache RAM des profils (lit `cluster_profile`) |
| bot | `3d429fe` | Lot 4 — backfill `raw_wallet_flow` |
| bot | `dd4a3ad` | côté gagnant (graduation) — hors de cette base |
| bot | `6d1d820` | auto-whitelist bons devs → snipe bloc 0 |
| bot | `3d23abd` | catégorie « bon rugger » (mcap tradeable répété) |
| bot | `86bdd6d` | bornage volume + détecteur rug best-effort (écrit `token_outcome`) |

État pipeline 24/7 (vérifié 2026-06-29, ~2 j de run) : ingestor `active` (9.1M slots vus,
465 manquants = 99.995 %), `raw_token_launch`≈58k, `raw_wallet_flow`≈127k, `cluster`≈14.5k,
`token_outcome`=0 (cf. §6). Côté gagnant : 18k mints suivis, ~105 graduations, 16 devs élites.

**dev_dumped (côté rug, ingestor)** : décodeur `detect_pumpfun_sells` + câblage coque
(lookup créateur + record_outcome + finalité B3), commits ingestor `cac7256` (câblage) +
diag. Validé live : modèle rugger rallumé (`rug_count>0`, `risk>0.5`).

_Dernière mise à jour : 2026-06-29._
