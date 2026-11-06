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
| E1 | Carte seule, lecture : moteur wasm dans un worker, IndexedDB, site statique | 🟨 |
| E2 | Console : envoi, clés, messages privés, groupes, dans le même moteur | ⬜ |
| E3 | Onglets partagés (SharedWorker, sinon onglet meneur par Web Locks), export/import des clés | ⬜ |
| E4 | Proxy local optionnel (Arti, un circuit par cellule, NIP-11 sans CORS) | ⬜ |
| E5 | Site assemblé, snapshot, qualification complète | ⬜ |

### E1 en lots

| Lot | Contenu | État |
|---|---|---|
| 1 | Cible wasm : dépendances par cible, `cargo check --target wasm32-unknown-unknown` | ⬜ |
| 2 | `rt` et `ws` (natif identique à aujourd'hui, tests inchangés) | ⬜ |
| 3 | `Store` mémoire + instantané (tests communs aux deux réalisations) | ⬜ |
| 4 | Routes sans HTTP (`api::handle`) partagées avec axum | ⬜ |
| 5 | Point d'entrée wasm, worker, `shim.js`, page | ⬜ |
| 6 | Tests : node (logique), Chrome sans fenêtre par le protocole DevTools (page) | ⬜ |
| 7 | Qualification : relais privé (événement de test `nak`), puis réseau réel ; snapshot | ⬜ |

## Décisions

| # | Décision | État |
|---|---|---|
| B1 | Branches `*-brow` par dépôt, compilation conditionnelle ; report sur `main` si succès | ✅ utilisateur 2026-11-06 |
| B2 | Proxy local optionnel (Arti), installé par l'utilisateur s'il le veut | ✅ utilisateur 2026-11-06 |
| B3 | Stockage wasm : tables en mémoire pur Rust + instantanés IndexedDB, pas SQLite (SQLite en wasm demande un clang LLVM, absent du Mac) | ⏸ |
| B4 | Nouvelles dépendances, cible wasm seulement : `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, `web-sys`, `getrandom` (feature JS) | ⏸ |
| B5 | Tests de page par Chrome sans fenêtre et le protocole DevTools (node), sans pilote à installer | ⏸ |

## Journal

- 2026-11-06 : outils installés par l'utilisateur (`wasm32-unknown-unknown`, `wasm-bindgen-cli`
  0.2.130). Essai : la bibliothèque de la carte s'arrête d'abord sur `getrandom` (dépendances
  natives à mettre par cible). Plan rédigé.
