-- seed_passthrough.sql — DENYLIST PERMANENTE des hubs « passthrough ».
--
-- Rôle : pré-remplir passthrough_node avec les adresses NOTOIREMENT traversantes
-- (programmes système, AMM/DEX, ponts, comptes de tip, hot wallets de CEX...).
-- Ces adresses agrègent des flux de milliers de wallets sans lien réel : si on
-- les laissait passer dans le clustering, elles fusionneraient des deployers
-- indépendants en méga-clusters, OU passeraient sous le seuil auto (degré >= 6
-- en in ET out) sur un petit échantillon et contamineraient les premiers clusters.
--
-- Cycle de vie : ces lignes sont posées avec source='seed'. Elles sont
-- PERMANENTES — reset_recomputable ne supprime QUE source='auto', jamais 'seed'.
-- Le seeding est appliqué par db::init APRÈS le schéma (la table doit exister),
-- et il est IDEMPOTENT grâce à INSERT OR IGNORE (la PK address bloque les
-- doublons) : ré-exécuter init ne crée aucune ligne en double.
--
-- ┌─────────────────────────────────────────────────────────────────────────┐
-- │ NE PAS INVENTER D'ADRESSE. Une adresse base58 erronée est un bug          │
-- │ SILENCIEUX : soit elle exclut à tort des wallets légitimes (faux hub),    │
-- │ soit un vrai hub reste absent et crée un méga-cluster. Chaque adresse     │
-- │ doit être vérifiée à la main avant d'être collée ici.                     │
-- └─────────────────────────────────────────────────────────────────────────┘
--
-- FORMAT IMPÉRATIF (les tests en dépendent) :
--   * UNE ligne INSERT par adresse — pas d'INSERT multi-VALUES.
--   * Toujours `INSERT OR IGNORE INTO passthrough_node`.
--   * Toujours `source='seed'` (3e valeur).
--
-- Exemple de format (COMMENTÉ, donc inactif — modèle à recopier) :
-- INSERT OR IGNORE INTO passthrough_node (address, label, source)
--   VALUES ('<adresse_base58>', '<label lisible>', 'seed');
--
-- Le fichier livré ne contient AUCUNE adresse réelle. Colle tes adresses déjà
-- vérifiées sous la catégorie correspondante, une par ligne, au format ci-dessus.


-- ===== Programmes système =====
-- (System Program, Token Program, Associated Token Account, Memo, etc.)


-- ===== DEX / AMM =====
-- (Raydium, Orca, Meteora, pump.fun AMM, agrégateurs type Jupiter, etc.)


-- ===== Ponts =====
-- (Wormhole, deBridge, Allbridge, etc.)


-- ===== Jito tip accounts =====
-- (comptes de pourboire MEV/Jito)


-- ===== CEX hot wallets (volatiles, à re-vérifier) =====
-- (Binance, Coinbase, OKX, Bybit... ces adresses changent : re-vérifier régulièrement)
