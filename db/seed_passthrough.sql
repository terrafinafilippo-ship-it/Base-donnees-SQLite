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
--   * ZÉRO doublon d'adresse (le test compare nb. de lignes INSERT au nb. de
--     lignes réellement insérées ; un doublon est ignoré et casse l'égalité).
--
-- Contenu : 46 adresses vérifiées (4 programmes système, 13 DEX/AMM/routeurs,
-- 4 comptes de frais pump.fun/PumpSwap, 8 tips Jito, 6 ponts, 11 hot wallets CEX).


-- ===== Programmes système (4) =====
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA', 'SPL Token Program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb', 'SPL Token-2022 Program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL', 'Associated Token Account (ATA) Program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('So11111111111111111111111111111111111111112', 'Wrapped SOL (wSOL) mint', 'seed');

-- ===== DEX / AMM / routeurs (13) =====
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4', 'Jupiter Aggregator v6 program (primary)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN', 'Jupiter v6 secondary deployment', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8', 'Raydium AMM v4 program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK', 'Raydium CLMM program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('routeUGWgWzqBWFcrCfv8tritsqukccJPu3q5GPP3xS', 'Raydium Routing program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc', 'Orca Whirlpools program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo', 'Meteora DLMM program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB', 'Meteora Dynamic AMM Pools program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY', 'Phoenix DEX program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('2wT8Yq49kHgDzXuPxZSaeLaH1qbmGXtEyPy64bL7aD3c', 'Lifinity Swap v2 program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('EewxydAPCCVuNEyrVN68PuSYdQ7wKn27V9Gjeoi8dy3S', 'Lifinity Swap v1 program', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P', 'pump.fun program (bonding curve)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA', 'PumpSwap AMM program (pump.fun migration venue)', 'seed');

-- ===== Comptes de frais pump.fun / PumpSwap (4) =====
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV', 'pump.fun / PumpSwap protocol fee recipient', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM', 'pump.fun fee account', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf', 'pump.fun Global config PDA', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('94qWNrtmfn42h3ZjUZwWvK1MEo9uVmmrBPd2hpNjYDjb', 'PumpSwap protocol fee recipient token account', 'seed');

-- ===== Jito tip accounts (8) =====
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49', 'Jito tip account 1', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY', 'Jito tip account 2', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe', 'Jito tip account 3', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh', 'Jito tip account 4', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5', 'Jito tip account 5', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt', 'Jito tip account 6', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL', 'Jito tip account 7', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT', 'Jito tip account 8', 'seed');

-- ===== Ponts cross-chain (6) =====
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth', 'Wormhole Core Bridge (Solana)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('wormDTUJ6AWPNvk59vGQbDvGJmqbDTdgWgAqcLBCgUb', 'Wormhole Token Bridge / Portal (Solana)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('src5qyZHqTqecJV4aY6Cb6zDZLMDzrDKKezs22MPHr4', 'deBridge DLN Source program (Solana)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('dst5MGcFPoBeREFAA5E3tU5ij8m5uVYwkzkSAbsLbNo', 'deBridge DLN Destination program (Solana)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('DEbrdGj3HsRsAzx6uH4MKyREKxVAfBydijLUF3ygsFfh', 'deBridge messaging gate (Solana)', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('BrdgN2RPzEMWF96ZbnnJaUtQDQx7VRXYaHHbYCBvceWB', 'Allbridge Core Bridge (Solana)', 'seed');

-- ===== CEX hot wallets (volatiles, à re-vérifier périodiquement) (11) =====
-- ATTENTION : ces adresses changent (rotation des hot wallets). Posées en 'seed'
-- (permanentes) elles peuvent devenir périmées : re-vérifier régulièrement.
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM', 'Binance Hot Wallet 1', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('5tzFkiKscXHK5ZXCGbXZxdw7gTjjD1mBwuoFbhUvuAi9', 'Binance 2', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('53unSgGWqEWANcPYRF35B2Bgf8BkszUtcccKiXwGGLyr', 'Binance.US Hot Wallet', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('GJRs4FwHtemZ5ZE9x3FNvJ8TMwitKTh21yxdRPqn7npE', 'Coinbase Hot Wallet 2', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('2AQdpHJ2JpcEgPiATUXjQxA8QmafFegfQwSLWSprPicm', 'Coinbase Commerce', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('H8sMJSCQxfKiFTCfDR3DUMLPwcRbM61LGFJ8N4dK3WjS', 'Kraken Hot Wallet', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('is6MTRHEgyFLNTfYcuV4QBWLjrZBfmhVNYR6ccgr8KV', 'OKX Hot Wallet', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('AC5RDfQFmDS1deWZos921JfqscXdByf8BKHs5ACWjtW2', 'Bybit Hot Wallet', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('ddKxKM2Xns3amrHu2qvYYegv7kEa7guho41fDWysffy', 'KuCoin Hot Wallet 2', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('AobVSwdW9BbpMdJvTqeCN4hPAmh4rHm7vwLnQ5ATSyrS', 'Crypto.com Hot Wallet 2', 'seed');
INSERT OR IGNORE INTO passthrough_node (address, label, source) VALUES ('u6PJ8DtQuPFnfmwHbGFULQ4u4EgjDiyYKjVEsynXq2w', 'Gate.io', 'seed');
