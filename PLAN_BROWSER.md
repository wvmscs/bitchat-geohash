# Version navigateur (carte, console, onglets) : spécification et plan

Démarré le 2026-11-06 sur le GO de l'utilisateur : spécifier, réaliser, tester, qualifier
(relais privé `wss://relay.famillevital.club`, puis réseau réel). **Pas de release : snapshots
seulement.** Ce plan couvre plusieurs dépôts ; il vit ici parce que la carte est la première
étape.

**Légende** : ⬜ à faire · 🟨 en cours · ✅ fait · ⏸ en attente d'une décision

## But

La carte (bitchat-geohash) et la console (bitchat-console) dans le navigateur, sans serveur :
Rust compilé en WebAssembly, état dans la base du navigateur (IndexedDB), plusieurs onglets
(carte, console, autres) sur un même moteur. Sert à analyser le terrain et à simuler, comme
les outils natifs.

## Ce que le navigateur retire (accepté)

| Natif | Navigateur | Conséquence |
|---|---|---|
| Tor (Arti) | impossible (pas de socket TCP) | les relais voient l'IP, sauf proxy local (E4) ou Tor Browser |
| NIP-11, sondes | `fetch` soumis à CORS | verdicts souvent « non sondé » ; complets avec le proxy local |
| DB-IP, BAN (`geocode`) | trop lourd / non prévu | pas de lieu ni d'IP dans les champs, geohash seulement (au début) |
| processus permanent | vit tant qu'un onglet est ouvert | pas de présence ni de réception onglets fermés |
| serveur HTTP, TLS | site statique | rien à installer pour la carte seule |

## Architecture

```text
onglet carte (app.js inchangé)    onglet console      autres onglets
        │ fetch('/api/...'), EventSource('/api/events')
        ▼  shim.js : les intercepte, les passe au moteur par messages
 ┌──────────────── moteur (wasm, worker) ─────────────────┐
 │ Monitor, Listener, Aggregate, Plan (code de la carte)   │
 │ api::handle(route, corps) → JSON ; flux d'événements    │
 │ rt : spawn, sleep, interval, timeout, now               │
 │ ws : WebSocket du navigateur        Store mémoire       │
 └───────────────┬────────────────────────────┬───────────┘
            relais Nostr (wss)          IndexedDB (instantanés)
```

- **Même code, deux cibles.** Le moteur est le code actuel de la carte. Ce qui touche au système
  passe par deux petits modules à double implémentation : `rt` (natif : tokio ; navigateur :
  `wasm-bindgen-futures`, `setTimeout`) et `ws` (natif : `tokio-tungstenite` ; navigateur :
  `web_sys::WebSocket`). Les primitives `tokio::sync` restent (elles compilent en wasm).
- **Mêmes routes.** Les vues JSON (`server::model`) et les routes `/api/*` sont réutilisées :
  le moteur répond aux mêmes requêtes que le serveur axum, sans HTTP. `app.js`, `view.js`,
  `app.css` et Leaflet ne changent pas ; un `shim.js` chargé avant eux remplace `fetch` et
  `EventSource` pour `/api/*` uniquement.
- **Stockage.** Même API `Store`, autre réalisation en wasm : tables en mémoire (pur Rust),
  instantané écrit dans IndexedDB (après chaque lot d'écritures et à la fermeture), relu au
  démarrage. Mêmes invariants (aucune donnée personnelle, migrations versionnées).

## Branches

| Dépôt | Branche | Contenu |
|---|---|---|
| bitchat-geohash | `geohash-brow` | `rt`, `ws`, `Store` mémoire, cible wasm, `shim.js`, site de la carte |
| bitchat-console | `console-brow` | protocole séparé de tokio/axum/rusqlite, moteur console en wasm |
| geohash-relays-net | `net-brow` | `ws` et `RelayComm` navigateur partagés, puis le proxy local (E4) |
| bitchat-launcher | `launcher-brow` | site assemblé (carte + console + onglets), snapshot |

En E1, `rt` et `ws` naissent dans bitchat-geohash ; ils passent dans geohash-relays-net en E2,
quand la console en a besoin aussi (une seule implémentation). Report sur `main` et releases
seulement si le projet réussit, sur décision de l'utilisateur.

## Étapes

| Étape | Contenu | État |
|---|---|---|
| E1 | Carte seule, lecture : moteur wasm dans un worker, IndexedDB, site statique | ✅ 3f7c984 |
| E2 | Console : envoi, clés, messages privés, groupes, dans le même moteur | ⬜ |
| E3 | Onglets partagés (SharedWorker, sinon onglet meneur par Web Locks), export/import des clés | ⬜ |
| E4 | Proxy local optionnel (Arti, un circuit par cellule, NIP-11 sans CORS) | ⬜ |
| E5 | Site assemblé, snapshot, qualification complète | ⬜ |

### E1 en lots

| Lot | Contenu | État |
|---|---|---|
| 1 | Cible wasm : dépendances par cible, `cargo check --target wasm32-unknown-unknown` | ✅ 36b0b56 |
| 2 | `rt` et `ws` (natif identique à aujourd'hui, tests inchangés) | ✅ 9509904 |
| 3 | `Store` mémoire + instantané (tests communs aux deux réalisations) | ✅ 809ea70 |
| 4 | Routes sans HTTP (`service::Service`) partagées avec axum | ✅ 50f1165 |
| 5 | Point d'entrée wasm, worker, `shim.js`, page | ✅ 5249ee6 |
| 6 | Tests : Chrome sans fenêtre par le protocole DevTools (page), pilotés depuis Rust | ✅ 5249ee6 |
| 7 | Qualification : relais privé (événement de test `nak`), puis réseau réel | ✅ 3f7c984 |

## Décisions

| # | Décision | État |
|---|---|---|
| B1 | Branches `*-brow` par dépôt, compilation conditionnelle ; report sur `main` si succès | ✅ utilisateur 2026-11-06 |
| B2 | Proxy local optionnel (Arti), installé par l'utilisateur s'il le veut | ✅ utilisateur 2026-11-06 |
| B3 | Stockage wasm : tables en mémoire pur Rust + instantanés IndexedDB, pas SQLite (SQLite en wasm demande un clang LLVM, absent du Mac ; l'utilisateur préfère éviter `brew install llvm`) | ✅ utilisateur 2026-11-07 |
| B4 | Nouvelles dépendances, cible wasm seulement : `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, `web-sys`, `getrandom` (feature JS) | ✅ utilisateur 2026-11-07 |
| B5 | Tests de page par Chrome sans fenêtre et le protocole DevTools (node), sans pilote à installer | ✅ utilisateur 2026-11-07 |

## Journal

- 2026-11-06 : outils installés par l'utilisateur (`wasm32-unknown-unknown`, `wasm-bindgen-cli`
  0.2.130). Essai : la bibliothèque de la carte s'arrête d'abord sur `getrandom` (dépendances
  natives à mettre par cible). Plan rédigé.
- 2026-11-07 : B3, B4, B5 approuvés (pas de `brew install llvm`). Lot 1 : dépendances natives
  dans une table par cible, enregistrements du stockage séparés de SQLite (`store/sqlite.rs`),
  sondes, téléchargement, listener, moniteur et ligne de commande natifs pour l'instant. Seule
  dépendance wasm à ce stade : `getrandom` (feature `js`, tirée par `k256`). Natif inchangé
  (tests et clippy identiques à `main`).
- 2026-11-07 : lot 2, `rt` (tâches, minuteries, horloges, hasard) et `ws` (WebSocket texte) ;
  listener et sondes dessus. En navigateur, les échecs WebSocket sont opaques : tout échec est
  « down » (réessayé), sauf adresse malformée. Lot 3, `store::memory` : mêmes règles que le
  schéma SQLite, garde la dernière liste par plateforme, la dernière sonde par relais, les
  canaux et 7 jours de compteurs ; instantané JSON versionné, sauvé au plus toutes les 5 s et
  à la fermeture ; mêmes tests pour les deux stockages (`tests/store_memory.rs`). Moniteur et
  téléchargement des listes (reqwest, `fetch` du navigateur) sur les deux cibles. Lot 4,
  `src/service` : vues, commandes, changement de zone et diffusion des deltas sortis du
  serveur ; `Service` répond aux requêtes de la page sans HTTP, mêmes codes et corps
  (`tests/service.rs`). Natif inchangé : 193 tests, clippy 0.
- 2026-11-07 : lots 5 à 7. `src/browser.rs` (`Engine` : options de la ligne de commande,
  requêtes, flux, journal dans la console), `web/worker.js` (moteur, instantané et zone dans
  IndexedDB), `web/shim.js` (redirige `fetch('/api/…')` et `EventSource('/api/events')` vers le
  worker ; `app.js` inchangé), `web/build.sh` (site statique dans `target/web`, wasm 2,0 Mo).
  Sauvegarde différée de 5 s en wasm (sinon les dernières écritures d'une rafale attendaient la
  suivante). Tests de page pilotés depuis Rust plutôt que node (relais de test et signatures
  déjà en Rust ; rien de neuf à installer). Qualification : Chrome sans fenêtre ; relais local
  (connexion, cellules dessinées, message compté, descente, retrouvée après rechargement,
  aucune erreur) ; `wss://relay.famillevital.club` en boucle fermée (`nak`, clé jetable) ;
  réseau réel en lecture sur `u09` : listes officielles lues par la page (CORS `*`), 9 relais
  écoutés, 6/9 connexions, 16 cellules avec des présences en 1 min (u09t : 41). Snapshot :
  pas d'outillage pour une branche `*-brow` ; site servi localement à la demande (E5 pour
  l'assemblage).
