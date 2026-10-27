# Étape 2 : serveur web interne et carte : plan de réalisation et avancement

Référence : [SPECIFICATIONS.md](SPECIFICATIONS.md), §4 (carte et interaction), §5 (vie privée),
« Entrées du CLI » et critères d'acceptation. Étape 1 : [PLAN_ET1.md](PLAN_ET1.md).
Règle : seuls les fichiers de `bitchat-geohash/` sont modifiés ; le dépôt reste autonome
(aucune dépendance de chemin).

**Légende** : ⬜ à faire · 🟨 en cours · ✅ fait · ⏸ en attente d'une décision

## Tableau de bord

| Lot | Contenu | État |
|---|---|---|
| 10 | Mise en place : feature `server`, dépendances, binaire, ressources embarquées | ✅ |
| 11 | `Monitor` : ajout de canaux à chaud (descente), relais géolocalisés, notifications | ✅ |
| 11 bis | Correction de la sélection des relais (une règle par app), report de bitchat-envelope | ✅ |
| 12 | Modèle exposé au navigateur (JSON) et test de vie privée | ✅ |
| 13 | Serveur axum : page, état, flux SSE, commandes, protections | ✅ |
| 13 bis | TLS optionnel, HTTP/1.1 et HTTP/2 | ⬜ |
| 14 | Page carte : cellules, échelle, survol, niveaux | ⬜ |
| 15 | Interaction : descente au clic, réintégration, remontée | ⬜ |
| 16 | Calques relais et mailles (somme des enfants) | ⬜ |
| 17 | CLI `bitchat-geohash` | ⬜ |
| 18 | Validation sur le réseau réel et dans le navigateur | ⬜ |

La numérotation continue celle de l'étape 1. Ordre : 10 → 11 → 11 bis → 12 sont côté librairie ;
13 dépend de 12 ; 13 bis dépend de 13 ; 14 dépend de 13 ; 15 et 16 sont indépendants entre eux ; 17 peut démarrer
dès 13 ; 18 clôt l'étape.

## Architecture visée

```
src/
  monitor.rs     + watch_channels(), descend(), relays(), subscribe() (lot 11)
  store/         + migrations/0002_channel_origin.sql (descentes, D12)
  plan.rs        + ListenPlan::extend() (canaux ajoutés après coup)
  server/        derrière la feature `server`
    mod.rs       serve(monitor, ServerConfig) -> Server { addr(), shutdown() }
    model.rs     types envoyés au navigateur (serde), conversion depuis CellStats
    routes.rs    GET /, /assets/*, /api/state, /api/events (SSE) ; POST /api/descend, /api/reinstate
    guard.rs     contrôle Host / Origin / Content-Type
    tls.rs       boucle d'acceptation tokio-rustls + hyper-util (ALPN h2, http/1.1) (lot 13 bis)
    assets/      index.html, app.js, app.css, leaflet/ (1.9.4, embarqué par include_bytes!)
  main.rs        binaire `bitchat-geohash` (required-features = ["server"])
tests/
  server.rs      HTTP + SSE de bout en bout avec TestRelay
  model.rs       sérialisation, absence de données personnelles
```

Flux : `Monitor` → `subscribe()` (canal `broadcast` : tick, retrait, réintégration, plan
modifié) → tâche de diffusion du serveur → deltas de cellules en SSE vers chaque navigateur.
Commandes du navigateur (`POST`) → `Monitor::descend()` / `reinstate()` → nouveau plan → diff
appliqué aux connexions → notification « plan modifié » → nouvelles cellules affichées.

Le serveur est une **fonction de la librairie** (`server::serve`), pas seulement du binaire :
l'étape 3 (Swift, `WKWebView`) la démarrera dans le processus de l'app.

## Lots

### Lot 10 : mise en place ✅

- [x] Feature `server` (activée par défaut ; `default-features = false` pour la librairie seule) :
      `axum 0.8` (features `tokio`, `http1`, `json`, `query`), `tokio` avec `signal` pour Ctrl-C.
      Pas de `tower-http` ni de `rust-embed` : ressources servies par `include_bytes!`, flux
      SSE construit avec `futures-util` (déjà présent). Pour TLS (lot 13 bis, D17) :
      `tokio-rustls`, `hyper` et `hyper-util` (feature `server-auto`), déjà tirés par axum ou
      proches de `rustls`, et `rustls-pemfile` ou l'API PEM de `rustls-pki-types` pour lire
      les certificats. Plus `tracing-subscriber` (journal du binaire), passé de dev à
      optionnel. Aucun fournisseur crypto autre que ring (ni aws-lc, ni OpenSSL).
- [x] `[[bin]] bitchat-geohash`, `required-features = ["server"]`.
- [x] Leaflet 1.9.4 copié dans `src/server/assets/leaflet/` (licence BSD-2, compatible
      GPL-3.0 ; fichier de licence conservé), empreinte SHA-256 notée dans le journal (D10).
- [x] Reprise de `bitchat-envelope/examples/common/map_overlay.html` par copie : styles
      (clair/sombre), fonction `el()` (texte seulement, jamais de HTML venant des relais),
      épingles de relais en anneau (`ringAnchors()` dans `app.js`).
- [x] `AGENTS.md` : nouvelles commandes, mise en page, invariants du serveur (lot 13) ;
      lien vers ce plan.

Fini quand : `cargo build` avec et sans `--no-default-features` passe, le binaire démarre et
sert une page vide.

Tests (`tests/server.rs`, 4 tests) : page et ressources servies avec leur type, toutes
embarquées, aucune ressource externe dans la page ; chemins inconnus → 404 ; Leaflet
identique à la version publiée (SHA-256) ; aucune ressource vide.

### Lot 11 : `Monitor` dynamique ✅

- [x] `ListenPlan::extend(ios, android, channels)` : sélection des relais des nouveaux canaux,
      ajoutés au dernier groupe de chaque relais tant qu'il a de la place, puis à de nouveaux
      groupes ; les souscriptions existantes ne changent pas, sauf celle du groupe complété
      (aucun `CLOSE`). Tout ou rien si un canal est invalide. `len()` / `is_empty()`.
- [x] `Monitor::descend(geohash)` : canaux du niveau de canal suivant dans la cellule (canal ou
      maille de la zone, niveau 3 compris) ; le niveau 7 est atteignable ainsi (demande
      explicite, D1). Plafond `max_channels` appliqué au total (`Error::TooManyChannels` avec
      l'effectif, rien n'est changé) ; hors zone → `Error::OutsideArea` ; sous le niveau 7 →
      `Error::NoFinerLevel`. Canal déjà surveillé laissé tel quel, retiré réintégré (log INFO).
      Rend les canaux ajoutés ou réintégrés ; log INFO `descended into u09tv: 32 channels…`.
- [x] Écoute immédiate des nouveaux canaux ; probe des nouveaux relais seulement, **en tâche
      de fond** : ses verdicts écartent ensuite les relais morts ou AUTH (le listener les
      abandonne de toute façon de lui-même). Pas d'attente du probe avant l'écoute.
- [x] `Monitor::watch_channels(channels)` : même chose pour une liste explicite.
- [x] Canaux ajoutés persistés (D12) : migration `0002_channel_origin.sql` (colonne `origin` :
      `start` ou `descent`), `Write::Descent`, `Write::ForgetDescents`, `Store::descents()`.
      Restaurés au démarrage s'ils sont dans la zone de départ, dans la limite du plafond
      (WARN au-delà) ; ceux qui étaient retirés le restent (D6).
      `MonitorConfig::forget_descents` (`--forget-descents`).
- [x] Listes de relais gardées dans l'état du `Monitor` (aussi avec `Monitor::start`), pour
      calculer les relais des nouveaux canaux.
- [x] `Monitor::relays()` → `Vec<RelayInfo>` (url, lat, lon, iOS / Android, statut, écouté,
      nombre de canaux surveillés, états des connexions), relais écartés compris.
- [x] `Monitor::subscribe()` → `broadcast::Receiver<MonitorEvent>` (256 événements) :
      `Tick { at }`, `Added`, `Retired`, `Reinstated` (listes de canaux), `RelaysChanged`
      (verdict ou état de connexion), `PlanChanged` (rechargement). Un abonné en retard reçoit
      `Lagged` et se resynchronise par un `snapshot()` complet.
- [x] Rechargement 24 h (lot 8) : les canaux de descente sont conservés.
- [x] Exemple `watch` : commande `descend <geohash>` et option `--forget-descents`.

Tests (`tests/descend.rs`, 8 tests ; `tests/plan.rs` +3 ; `tests/store.rs` +2) : `extend`
garde les souscriptions envoyées et ne ferme rien ; canaux connus ignorés, dernier groupe
complété ; canal invalide → plan inchangé ; descente de `u09tv` → 32 canaux de niveau 6
écoutés, événement compté, `Added` reçu, log INFO ; plafond, hors zone, geohash invalide,
niveau 7 puis rien en dessous ; restauration au redémarrage, pas dans une autre zone,
`forget_descents` ; descentes retirées restées retirées, réintégrées par une nouvelle
descente ; relais mort choisi seulement après une descente → probé, `Dead`, écarté ;
`RelayInfo` ; abonné lent → `Lagged` ; rechargement des listes → descentes conservées ;
migration d'une base v1 avec ses canaux.

### Lot 11 bis : sélection des relais, une règle par app ✅

Report de `bitchat-envelope/Documents/relay-selection-corrections.md` (27/10/2026) : la
sélection appliquait la normalisation d'iOS aux deux listes et ajoutait les ex æquo du
5ᵉ rang. Les CSV figés de `tests/data/` sont ceux du §6 du document (mêmes SHA-256).

- [x] `RelayDirectory::parse(csv, Platform)` : lecture tolérante commune (lignes vides et
      `relay url…` sautées, ligne illisible rejetée et comptée, coordonnées hors bornes
      rejetées : D19) ; iOS : une entrée par hôte normalisé, premières coordonnées gardées ;
      Android : adresse brute (`wss://` ajouté si absent), ni minuscules ni dédoublonnage.
- [x] `ios_conflict()` : premier hôte listé à deux coordonnées ; liste iOS en conflit refusée
      au chargement, repli sur la dernière liste valide en base (D18).
- [x] `ios_selection` (tri `(km, hôte)`, 5 premiers) et `android_selection` (tri **stable**
      sur la distance seule, ordre du CSV, 5 premiers) remplacent `closest` ; plus d'ex æquo.
- [x] `select_both` : union par serveur (`normalize_url`), `Selected.android: Vec<usize>`
      (0, 1 ou 2 rangs) ; propagation à `divergence`, `CellRelay`, `RelayInfo`, `watch`.
- [x] Spec (« égalités comprises ») et PLAN_ET1 (lot 1) corrigés.

Tests (`tests/select.rs`) : fixture et 5 cas de C6 ; cellules `u1`, `gb`, `9q` du §4 ;
322 cellules de niveau 2 où Android n'a que 4 serveurs distincts ; liste iOS en conflit
refusée avec repli, acceptée avec WARN sans liste précédente.

Le « 322 » du document compte les régions à **moins de 5** serveurs distincts (307 à 4,
15 à 3), confirmé par un script Python indépendant de notre code. Un hôte présent à deux
coordonnées dans les deux listes (`relay.earthly.city` sur `9q`) garde celles d'iOS pour
l'affichage, comme dans bitchat-envelope.

### Lot 12 : modèle exposé au navigateur ✅

- [x] `server::model` : `CellView` (geohash, niveau, bbox `[[s,w],[n,e]]`, `watched`,
      messages, présences, participants ou `"low"`, `last_seen`, relais avec rangs iOS/Android
      et statut (Android : liste de rangs), `delivered`, divergence), `MeshView`, `RelayView`
      (connexions : `connecting`, `connected`, `waiting`, `abandoned`), `StatusView`, `Params`
      (fenêtre, délai d'inactivité, *k*, niveaux, départ, profondeur, plafond, URL des tuiles
      et attribution). `CellStats` reste sans `serde` : la conversion est explicite, champ par
      champ (`From<&CellStats>`, etc.).
- [x] Messages SSE (`Push`) : `hello` (état complet à la connexion), `cells` (cellules
      modifiées depuis le dernier envoi), `removed`, `relays` (liste entière si un relais a
      changé), `status` ; champ `type` = nom d'événement SSE ; `seq` croît de 1 par message,
      `hello` porte le dernier envoyé (D11).
- [x] Calcul des deltas : `Published` garde le dernier état envoyé ; `update()` rend les
      messages dans l'ordre `removed`, `cells`, `relays`, `status`, rien si rien n'a changé ;
      une seule instance pour tous les navigateurs (branchée au lot 13).
- [x] Tous les chiffres affichés, niveau 6 compris : seuil *k* désactivé par défaut
      (`k_threshold` = 0) ; s'il est réglé, seuls les participants passent à `"low"` (D20).

`MeshView` est prêt ; son transport (dans `hello` ou à la demande, selon le niveau choisi
dans la page) est décidé au lot 16.

Tests (`tests/model.rs`, 5 tests) : JSON de référence d'une cellule, d'un relais et du statut ;
avec un seuil *k* réglé, participants d'une cellule et d'une maille de niveau 6 sous *k* →
`"low"`, messages et présences chiffrés ; sans seuil (défaut), une personne seule comptée ; deltas (rien si inchangé, seule
la cellule modifiée, `removed`, `seq`, `hello` après coup) ; **vie privée** de bout en bout
(`TestRelay`, `Monitor`, descente) : 8 événements signés dont le contenu et un tag `n`
portent un marqueur, ni marqueur, ni pubkey, ni id, ni signature, ni `npub`, ni suite de
64 caractères hexadécimaux dans `hello` ni dans les deltas.

### Lot 13 : serveur axum ✅

- [x] `server::serve(monitor, &monitor_config, ServerConfig { listen, allow_remote, tiles,
      attribution, push_every, keep_alive, max_streams, max_body })` → `Server` ;
      `Server::shutdown` ferme les flux, attend les requêtes en cours et rend le `Monitor`
      (toujours actif) ; Ctrl-C dans le binaire.
- [x] Routes : `GET /` (page), `GET /assets/*`, `GET /api/state` (le `hello` complet),
      `GET /api/events` (SSE : `event` = type, `id` = `seq`, `keep-alive` 15 s ; un flux en
      retard est fermé, le navigateur se reconnecte et reçoit un nouveau `hello`),
      `POST /api/descend {geohash}` → `{"added": [...]}`, `POST /api/reinstate {channels | all}`
      → `{"reinstated": [...]}`. Erreurs JSON `{"error": code, "message", ...}` :
      `invalid_geohash` 400, `too_many_channels` 409 (avec `count` et `max`), `outside_area` et
      `no_finer_level` 422, corps illisible `bad_request` 400/422, `too_large` 413,
      `monitor_stopped` 503.
- [x] Diffusion : une tâche suit `subscribe()` et, au plus une fois par `push_every`
      (défaut 1 s) et seulement si quelque chose a bougé, lit l'état, calcule les deltas
      (`Published`) et les publie sur un `broadcast` vers les flux SSE. `hello` et
      l'abonnement sont pris sous le même verrou : aucun delta ne tombe entre les deux.
- [x] **Protections** (D13) :
  - écoute sur `127.0.0.1:8080` par défaut ; une adresse non locale exige `allow_remote`
    (`Error::RemoteListen` sinon) et produit un WARN au démarrage ;
  - `Host` (ou l'autorité de l'URI en HTTP/2) vérifié : adresse d'écoute, plus `localhost`
    sur une adresse de bouclage ; sinon 421 (pas de contrôle si l'adresse est `0.0.0.0`) ;
  - `POST` : `Content-Type: application/json` exigé (sinon 415), `Origin` égal à
    `http://<Host>` (sinon 403) ;
  - en-têtes `Content-Security-Policy` (`default-src 'none'`, scripts, styles et connexions
    `'self'`, images `'self'` et l'hôte des tuiles, `frame-ancestors 'none'`),
    `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` ;
  - corps de requête bornés (64 Kio) ; flux SSE simultanés bornés (64, sinon 503
    `too_many_streams`).
- [x] Logs `tracing` : démarrage (adresse), chaque commande (sans en-têtes), chaque refus
      (méthode, chemin, raison).
- [x] Binaire minimal : `bitchat-geohash <geohash> [--listen] [--allow-remote] [--db]`,
      listes officielles, monitor + serveur, arrêt sur Ctrl-C. Le reste du CLI au lot 17.

Tests (`tests/server.rs`, 8 nouveaux, `TestRelay` + `reqwest`) : `hello` puis delta de la
seule cellule touchée reçu en moins de 2 s après publication d'un événement ; descente par
`POST` → nouvelles cellules dans le flux et dans `/api/state` ; refus expliqués (plafond avec
l'effectif, geohash invalide, hors zone, corps incomplet) ; réintégration d'un canal puis de
tous ; `POST` sans `Origin` ou avec un autre → 403, `text/plain` → 415, mauvais `Host` → 421,
`localhost` accepté, corps trop gros → 413, rien d'exécuté ; en-têtes de sécurité ; 2ᵉ flux au-delà
de la limite → 503, arrêt → flux fermé et monitor rendu ; adresse non locale sans
`allow_remote` refusée. Tests unitaires (`guard.rs`) : noms d'hôte acceptés, origine des tuiles
pour la CSP.

### Lot 13 bis : TLS optionnel ⬜

- [ ] `ServerConfig::tls: Option<TlsFiles { cert, key }>` : certificat et clé PEM fournis par
      l'utilisateur (`--tls-cert`, `--tls-key`), par exemple produits par `mkcert` (D17).
- [ ] `server::tls` : boucle d'acceptation `tokio-rustls` + `hyper-util` (`auto::Builder`),
      ALPN `h2` puis `http/1.1` : HTTP/2 lève la limite de 6 connexions par hôte (flux SSE de
      plusieurs onglets multiplexés). Fournisseur crypto `ring`, déjà installé par
      `install_crypto_provider()`.
- [ ] Sans TLS : HTTP/1.1 simple, comportement par défaut sur `127.0.0.1`.
- [ ] `--allow-remote` exige TLS ; `--insecure-remote` le permet sans, avec un WARN au démarrage.
- [ ] Contrôle `Host` aligné sur les noms du certificat (SAN) quand TLS est actif ; pas de
      `Strict-Transport-Security` (gênant sur `localhost`).
- [ ] Erreurs claires : fichier absent, PEM illisible, clé ne correspondant pas au certificat.
- [ ] Recette `mkcert` documentée (`mkcert -install`, `mkcert localhost 127.0.0.1 ::1`).
- [ ] Génération automatique d'un certificat autosigné (`rcgen`) : reportée à l'étape 3, si
      WKWebView en a besoin (validation par empreinte côté Swift).

Tests (`tests/tls.rs`) : certificat de test figé dans `tests/data/` (autorité de test, jamais
de vraie clé) ; client `reqwest` qui lui fait confiance ; page servie en HTTP/2 (ALPN `h2`)
et en HTTP/1.1 ; flux SSE reçu en TLS ; plusieurs flux SSE sur une seule connexion HTTP/2 ;
`--allow-remote` sans TLS refusé ; clé et certificat discordants → erreur au démarrage.

### Lot 14 : page carte ⬜

- [ ] `index.html`, `app.js`, `app.css` sans étape de build (JavaScript simple, modules ES).
- [ ] Tuiles : URL configurable (`--tiles`, défaut `https://tile.openstreetmap.org/{z}/{x}/{y}.png`),
      attribution toujours affichée (`--tiles-attribution`, défaut OSM).
- [ ] Un rectangle par canal (bbox fournie par le serveur, pas de décodage geohash en JS),
      couleur selon l'échelle choisie : participants ou messages (D16) ; « faible » (sous *k*, si un
      seuil est réglé) hachuré ; canal retiré : contour gris pointillé, sans remplissage ; canal
      sans activité dans la fenêtre : contour seul.
- [ ] **Niveaux superposés** : `u09` profondeur 2 affiche 32 cellules 4c et 1 024 cellules 5c
      au même endroit. Sélecteur de niveau (un niveau visible à la fois, le plus fin par
      défaut) ; nombre de cellules actives indiqué par niveau.
- [ ] Survol : infobulle avec le modèle du §3 (compteurs, `last_seen` relatif, relais iOS et
      Android avec rang et statut, relais ayant livré, divergence). Avertissement fixe :
      présences non comparables entre niveaux (heartbeats aux précisions 2, 4, 5 seulement).
- [ ] Bandeau d'état : connexion SSE (vivante, reconnexion), relais écoutés / écartés,
      connexions ouvertes, canaux surveillés / retirés, rejets.
- [ ] Mise à jour en place (styles modifiés, pas de recréation des couches) ; reconnexion SSE
      automatique (`EventSource`, `Last-Event-ID` = numéro de séquence) avec reprise de l'état
      par `hello`.
- [ ] Flux SSE qui ne s'ouvre pas (limite de 6 connexions par hôte en HTTP/1.1, D9) : message
      « too many tabs open on this server: close one, or enable TLS (HTTP/2) ».
- [ ] Clair et sombre, utilisable à 360 px de large ; textes de la page en anglais, comme
      les logs.

Fini quand : `u09` profondeur 2 s'affiche en moins de 2 s et reste fluide (1 056 rectangles,
une mise à jour par seconde).

### Lot 15 : interaction ⬜

- [ ] Clic sur une cellule surveillée : menu « Descendre » (lot 11) ; refus du serveur
      (plafond) affiché avec l'effectif réel.
- [ ] Clic sur une cellule retirée : « Réintégrer » ; bouton global « Tout réintégrer »
      (confirmation) (D15).
- [ ] Après une descente, la vue passe au niveau des enfants et cadre la cellule ; fil
      d'Ariane des niveaux pour remonter (affichage seulement, la descente n'est pas annulée).
- [ ] Échelle et niveau choisis gardés dans l'URL (`#level=5&scale=participants`), pour
      partager une vue entre onglets.

Tests : commandes couvertes au lot 13 ; parcours manuel au lot 18 (liste à cocher dans le journal).

### Lot 16 : calques relais et mailles ⬜

- [ ] Calque « relais » (désactivé par défaut) : position de chaque relais écouté ou écarté,
      couleur selon le statut (`ok`, `auth`, `refused`, `down`, `dead`, `unprobed`), forme selon
      la plateforme (les deux apps / une seule), anneau pour les relais aux mêmes coordonnées,
      comme `geohash_map overlay`. Survol d'une cellule → traits vers ses relais.
- [ ] Calque « mailles » : somme des enfants (`MeshStats`) au niveau choisi, niveau 3 compris,
      distincte de l'activité propre du canal ; *k* (s'il est réglé) appliqué comme pour les cellules.

Tests (`tests/model.rs`) : `RelayView` d'un relais aux coordonnées partagées ; `MeshView` sous
*k* → `"low"`.

### Lot 17 : CLI `bitchat-geohash` ⬜

- [ ] Options de la spec : `<geohash> [--depth N] [--levels 4,5,6] [--max-cells N]
      [--window 15m] [--inactivity 15m] [--relays-csv …] [--android-csv … | --no-android]
      [--no-probe] [--db …] [--reinstate-all] [--forget-descents] [--listen 127.0.0.1:8080]
      [--allow-remote] [--insecure-remote] [--tls-cert <pem>] [--tls-key <pem>] [--tiles <url>] [--tiles-attribution <text>] [--open] [--verbose]`.
- [ ] Analyse des arguments partagée avec `examples/watch.rs` (module `cli` interne, sans
      nouvelle dépendance) pour ne pas diverger.
- [x] `--no-drop-dead` de la spec : non retenu (l'invariant 3 l'interdit : un relais mort
      n'est jamais remplacé, et l'écouter quand même ne sert à rien) ; spec corrigée (D14).
- [ ] Au démarrage : plan et verdicts résumés sur stderr, puis l'URL de la carte ; `--open`
      lance le navigateur (`open` / `xdg-open`).
- [ ] Code de sortie non nul et message clair : geohash invalide, plafond, aucune liste,
      port occupé.

Tests : analyse des arguments (unitaires) ; démarrage du binaire sur listes figées et
`TestRelay`, `GET /api/state` répond (`tests/cli.rs`).

### Lot 18 : validation réelle ⬜

- [ ] Boucle fermée sur `wss://relay.famillevital.club` (liste à ce seul relais, `nak`, clé
      jetable) : l'événement apparaît **sur la carte** en moins de 5 s, dans la bonne cellule
      et elle seule (critère de la spec).
- [ ] Message de l'app BitChat de l'utilisateur dans un canal observé, puis descente au clic
      dans ce canal et second message à un niveau plus fin.
- [ ] `u09` profondeur 2 sur les listes officielles, 30 min : fluidité, mémoire du serveur et
      du navigateur, débit SSE (octets par minute), retraits visibles sur la carte.
- [ ] Contrôle des messages envoyés au navigateur (capture de `/api/events`) : aucune pubkey,
      aucun nickname, aucun contenu.
- [ ] Safari et un navigateur Chromium ; largeur téléphone.
- [ ] TLS avec un certificat `mkcert` : HTTP/2 négocié (onglet réseau), 8 onglets ouverts
      sans blocage ; sans TLS, message « too many tabs » au 7ᵉ onglet.
- [ ] Mesures et captures dans le journal ; capture d'écran dans le README ou la PR.

Fini quand : tous les critères d'acceptation de la spec sont vérifiés, navigateur compris.

## Décisions

| # | Question | Proposition | État |
|---|---|---|---|
| D9 | Poussée en direct : WebSocket ou SSE ? | SSE : flux à sens unique, reconnexion native du navigateur, simple dans axum, lisible avec `curl` ; les commandes passent par `POST`. Réserve : limite de 6 connexions par hôte en HTTP/1.1, d'où un message dans la page (lot 14) et HTTP/2 via TLS (D17). WebSocket (`/api/ws`) à revoir si la page doit un jour s'abonner à la zone visible | ✅ |
| D10 | Leaflet depuis un CDN ou embarqué ? | Embarqué dans le binaire : marche hors ligne, version figée, aucune requête tierce hormis les tuiles | ✅ |
| D11 | État complet à chaque tick ou deltas ? | État complet à la connexion (`hello`), puis deltas des cellules modifiées au plus une fois par seconde | ✅ |
| D12 | Les canaux ajoutés par descente survivent-ils à un redémarrage ? | Oui, comme les retraits (D6), avec `--forget-descents` ; migration `0002` | ✅ |
| D13 | Exposition réseau | `127.0.0.1` par défaut ; autre adresse avec `--allow-remote` seulement ; contrôle `Host`/`Origin` ; pas d'authentification à l'étape 2 | ✅ |
| D14 | `--no-drop-dead` de la spec | Abandonné (contraire à l'invariant 3) ; spec corrigée | ✅ |
| D15 | Clic sur une cellule | Menu : « Descendre » pour une cellule surveillée, « Réintégrer » pour une cellule retirée | ✅ |
| D16 | Échelle de couleurs | Logarithmique, palette lisible par les daltoniens (type viridis), « faible » hachuré ; choix participants / messages dans la page | ✅ |
| D17 | TLS ? | Optionnel : certificat et clé fournis (`--tls-cert`, `--tls-key`, ex. `mkcert`), HTTP/1.1 et HTTP/2 par ALPN (`tokio-rustls` + `hyper-util`) ; obligatoire avec `--allow-remote` sauf `--insecure-remote` (WARN) ; certificat autosigné généré (`rcgen`) reporté à l'étape 3 | ✅ |
| D18 | Liste iOS où un hôte figure à deux coordonnées (l'app la refuse) | Refusée comme l'app : WARN et repli sur la dernière liste valide en base ; acceptée avec WARN s'il n'y en a aucune | ✅ |
| D19 | Coordonnées hors bornes (Android les accepte) | Ligne rejetée et comptée, comme avant | ✅ |
| D20 | Cellule (ou maille) de niveau 6 sous *k* : que montrer ? | Tous les chiffres, participants compris : seuil *k* désactivé par défaut (`k_threshold` = 0), gardé en option (il ne masque alors que les participants) ; spec §5 et invariant 1 corrigés | ✅ |

## Risques

- **Superposition des niveaux** : les canaux 4c et 5c couvrent la même zone ; sans sélecteur
  de niveau la carte est illisible (traité au lot 14).
- **Charge du navigateur** : 1 056 rectangles suffisent pour `u09` profondeur 2 ; au-delà
  (descentes répétées, jusqu'à 2 048 canaux) le rendu SVG de Leaflet peut ralentir.
  Repli : rendu `L.canvas()`.
- **Politique d'usage des tuiles OSM** : usage léger et interactif seulement ; pour un usage
  soutenu, `--tiles` vers un autre fournisseur ou un serveur local.
- **Commandes depuis une autre page** (CSRF, DNS rebinding) sur un serveur local : traitées
  par D13 et testées au lot 13.
- **Certificats** : un certificat autosigné est refusé par Safari et WKWebView tant qu'on ne
  lui a pas fait confiance ; d'où `mkcert` à l'étape 2. Ne jamais committer une vraie clé
  privée (seule l'autorité de test de `tests/data/` l'est, marquée comme telle).
- **Descente vers des relais non encore probés** : quelques secondes avant l'écoute ; la
  cellule l'indique (« starting »).
- **Croissance de `cell_bucket`** : une ligne par canal actif et par minute, sans purge à
  l'étape 1. Faible au lot 9 (127 Ko en 16 min) ; une rétention (ex. 30 jours) pourra être
  ajoutée par une migration si les mesures du lot 18 le justifient.

## Journal

| Date | Lot | Avancement |
|---|---|---|
| 27/10/2026 | — | Plan initial de l'étape 2. Décisions D9 à D16 proposées. |
| 27/10/2026 | — | D9 (SSE) validée avec réserve sur la limite de 6 connexions ; D17 (TLS optionnel, HTTP/2) ajoutée et validée ; nouveau lot 13 bis. |
| 27/10/2026 | — | D10 à D16 validées (Leaflet embarqué, deltas, descentes persistées, exposition locale par défaut, `--no-drop-dead` abandonné, menu au clic, échelle logarithmique). Spec corrigée (entrées du CLI). |
| 27/10/2026 | 10 | Feature `server` (par défaut), binaire `bitchat-geohash` (page seule, `--listen`, arrêt sur Ctrl-C), `server::router()` et ressources embarquées (`include_bytes!`). Leaflet 1.9.4 depuis le registre npm, intégrité `sha512-nxS1ynzJ…FA74PA==` conforme ; SHA-256 `leaflet.js` `db49d009…f4e5641a`, `leaflet.css` `a7837102…4d147d04c6`. 96 tests (92 sans la feature) ; clippy propre dans les deux configurations. |
| 27/10/2026 | 10 | `Cargo.lock` propre au dépôt (227 paquets), tiré du verrou du workspace pour garder les mêmes versions ; copie hors workspace : `cargo test --locked` 96 tests, 92 sans la feature, clippy et build release OK. |
| 27/10/2026 | 11 | `ListenPlan::extend`, `Monitor::descend` / `watch_channels` / `relays` / `subscribe`, migration `0002_channel_origin`. Probe des nouveaux relais en tâche de fond plutôt qu'avant l'écoute (pas d'attente au clic). 108 tests (104 sans la feature), stables sur 3 lancements. **Boucle fermée** sur `wss://relay.famillevital.club` : `watch u09tv --depth 0`, `descend u09tv` (32 canaux ajoutés sur la connexion ouverte, sans reconnexion), kind 20000 publié par `nak` avec une clé jetable dans `u09tvw` → compté dans `u09tvw` seul. |
| 27/10/2026 | — | Lot 11 commité (`730dd49`). Nouveau lot 11 bis : report des corrections de sélection de bitchat-envelope. D18 (liste iOS en conflit refusée, repli) et D19 (rejet des coordonnées hors bornes) validées. |
| 27/10/2026 | 11 bis | Sélection des relais corrigée (une règle par app, plus d'ex æquo, `Selected.android: Vec<usize>`, union par serveur) ; D18 (liste iOS en conflit refusée, repli sur la dernière valide, au rechargement la liste courante est gardée) ; D19. Résultats identiques à `geohash_map` corrigé sur `u09tv`, `u0`, `9q8yy`, `gcpvj`, `u1`, `gb`, `9q` ; `9q8yy` passe de 10 à 8 serveurs. 117 tests (113 sans la feature), stables sur 3 lancements ; clippy propre dans les deux configurations. Listes officielles du jour : iOS identique à `tests/data/` (sans conflit) ; `watch u09tv` réel : 8 serveurs, un relais mis de côté (`dead`). |
| 27/10/2026 | — | Lot 11 bis commité (`98a72e8`). |
| 27/10/2026 | 12 | `server::model` : vues, `Push` (SSE), `Published` (deltas). D20 validée : tous les chiffres affichés, seuil *k* désactivé par défaut (`k_threshold` 3 → 0, gardé en option ; seuls les participants passent à `"low"` s'il est réglé) ; spec §5 et invariant 1 corrigés. 123 tests (114 sans la feature), stables sur 3 lancements ; clippy propre dans les deux configurations. |
| 27/10/2026 | — | Lot 12 commité (`2bc7165`). |
| 27/10/2026 | 13 | `server::serve`, `Server`, routes `/api/*`, SSE, diffusion des deltas, protections D13, binaire minimal (geohash, `--listen`, `--allow-remote`, `--db`). 132 tests (114 sans la feature), stables sur 3 lancements ; clippy propre dans les deux configurations. Essai réel du binaire sur `u09tv` (listes officielles) : 33 cellules, 8 relais dont 1 écarté (certificat), `hello` puis deltas dans le flux, `Host` étranger → 421, Ctrl-C → arrêt propre. |
