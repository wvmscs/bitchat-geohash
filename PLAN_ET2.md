# Étape 2 : serveur web interne et carte : plan de réalisation et avancement

Référence : [SPECIFICATIONS.md](SPECIFICATIONS.md), §4 (carte et interaction), §5 (vie privée),
« Entrées du CLI » et critères d'acceptation. Étape 1 : [PLAN_ET1.md](PLAN_ET1.md).
Règle : seuls les fichiers de `bitchat-geohash/` sont modifiés ; le dépôt reste autonome
(aucune dépendance de chemin).

**Légende** : ⬜ à faire · 🟨 en cours · ✅ fait · ⏸ en attente d'une décision

## Tableau de bord

| Lot | Contenu | État |
|---|---|---|
| 10 | Mise en place : feature `server`, dépendances, binaire, ressources embarquées | ⬜ |
| 11 | `Monitor` : ajout de canaux à chaud (descente), relais géolocalisés, notifications | ⬜ |
| 12 | Modèle exposé au navigateur (JSON) et test de vie privée | ⬜ |
| 13 | Serveur axum : page, état, flux SSE, commandes, protections | ⬜ |
| 13 bis | TLS optionnel, HTTP/1.1 et HTTP/2 | ⬜ |
| 14 | Page carte : cellules, échelle, survol, niveaux | ⬜ |
| 15 | Interaction : descente au clic, réintégration, remontée | ⬜ |
| 16 | Calques relais et mailles (somme des enfants) | ⬜ |
| 17 | CLI `bitchat-geohash` | ⬜ |
| 18 | Validation sur le réseau réel et dans le navigateur | ⬜ |

La numérotation continue celle de l'étape 1. Ordre : 10 → 11 → 12 sont côté librairie ;
13 dépend de 12 ; 13 bis dépend de 13 ; 14 dépend de 13 ; 15 et 16 sont indépendants entre eux ; 17 peut démarrer
dès 13 ; 18 clôt l'étape.

## Architecture visée

```
src/
  monitor.rs     + watch_channels(), descend(), relays(), subscribe() (lot 11)
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

### Lot 10 : mise en place ⬜

- [ ] Feature `server` (activée par défaut ; `default-features = false` pour la librairie seule) :
      `axum 0.8` (features `tokio`, `http1`, `json`, `query`), `tokio` avec `signal` pour Ctrl-C.
      Pas de `tower-http` ni de `rust-embed` : ressources servies par `include_bytes!`, flux
      SSE construit avec `futures-util` (déjà présent). Pour TLS (lot 13 bis, D17) :
      `tokio-rustls`, `hyper` et `hyper-util` (feature `server-auto`), déjà tirés par axum ou
      proches de `rustls`, et `rustls-pemfile` ou l'API PEM de `rustls-pki-types` pour lire
      les certificats. Liste à valider (règle d'AGENTS.md).
- [ ] `[[bin]] bitchat-geohash`, `required-features = ["server"]`.
- [ ] Leaflet 1.9.4 copié dans `src/server/assets/leaflet/` (licence BSD-2, compatible
      GPL-3.0 ; fichier de licence conservé), empreinte SHA-256 notée dans le journal (D10).
- [ ] Reprise de `bitchat-envelope/examples/common/map_overlay.html` par copie : styles
      (clair/sombre), fonction `el()` (texte seulement, jamais de HTML venant des relais),
      épingles de relais en anneau.
- [ ] `AGENTS.md` : nouvelles commandes, mise en page, invariants du serveur (lot 13) ;
      lien vers ce plan.

Fini quand : `cargo build` avec et sans `--no-default-features` passe, le binaire démarre et
sert une page vide.

### Lot 11 : `Monitor` dynamique ⬜

- [ ] `ListenPlan::extend(ios, android, channels)` : sélection des relais des nouveaux canaux,
      ajoutés à des groupes stables nouveaux ou incomplets ; les souscriptions existantes ne
      changent pas, sauf celle du groupe complété.
- [ ] `Monitor::descend(geohash)` : enfants de niveau de canal suivant (`channels::children`
      jusqu'au prochain `Level` observé), plafond `max_channels` appliqué au total
      (`Error::TooManyChannels`), probe des nouveaux relais seulement, diff appliqué. Rend la
      liste des canaux ajoutés. Un canal déjà présent n'est pas dupliqué ; retiré, il est
      réintégré.
- [ ] `Monitor::watch_channels(channels)` : même chose pour une liste explicite.
- [ ] Canaux ajoutés par descente persistés (D12) : migration `0002_channel_origin.sql`
      (colonne `origin` : `start` ou `descent`), relus au démarrage ; option
      `--forget-descents`.
- [ ] `Monitor::relays()` → `Vec<RelayInfo>` (url, lat, lon, plateformes, statut du probe,
      état des connexions, nombre de canaux) : coordonnées lues dans les `RelayDirectory`.
- [ ] `Monitor::subscribe()` → `broadcast::Receiver<MonitorEvent>` : `Tick { at }` à chaque
      tour de boucle, `Retired(Vec<String>)`, `Reinstated(Vec<String>)`, `PlanChanged`,
      `RelayChanged(String)`. Un abonné en retard perd des tick (`Lagged`) et se resynchronise
      par un `snapshot()` complet.
- [ ] Rechargement 24 h (lot 8) : les canaux de descente sont conservés comme les retraits.

Tests (`tests/monitor.rs`, `tests/plan.rs`) : `extend` ne touche pas les souscriptions déjà
envoyées ; descente de `u09tv` → 32 canaux de niveau 6 souscrits sur leurs relais ;
dépassement du plafond → erreur, plan inchangé ; descente persistée puis relue après
redémarrage ; `subscribe` reçoit `Retired` avec le log INFO ; abonné saturé → `Lagged`.

### Lot 12 : modèle exposé au navigateur ⬜

- [ ] `server::model` : `CellView` (geohash, niveau, bbox `[[s,w],[n,e]]`, `watched`,
      messages, présences, participants ou `"low"`, `last_seen`, relais avec rangs iOS/Android
      et statut, `delivered`, divergence), `MeshView`, `RelayView`, `StatusView`, `Hello`
      (paramètres : fenêtre, *k*, niveaux, départ, URL des tuiles et attribution).
      `CellStats` reste sans `serde` : la conversion est explicite, champ par champ.
- [ ] Messages SSE : `hello` (état complet à la connexion), `cells` (cellules modifiées
      depuis le dernier envoi), `removed`, `relays`, `status` ; numéro de séquence pour
      détecter un trou (D11).
- [ ] Calcul des deltas : comparaison avec le dernier snapshot envoyé, côté serveur, une seule
      fois pour tous les navigateurs.

Tests (`tests/model.rs`) : JSON de référence pour une cellule ; cellule de niveau 6 sous *k*
→ `"low"` sans nombre ; **vie privée** : aucune clé ni valeur de 64 caractères hexadécimaux
(pubkey, id, signature) et aucun contenu de message dans `hello` ni dans les deltas, avec des
événements dont le contenu est un marqueur recherché ensuite dans le JSON.

### Lot 13 : serveur axum ⬜

- [ ] `server::serve(monitor, ServerConfig { listen, tiles, attribution, push_every })` ;
      arrêt propre (`Server::shutdown`, Ctrl-C dans le binaire).
- [ ] Routes : `GET /` (page), `GET /assets/*` (types MIME explicites, `Cache-Control`),
      `GET /api/state` (JSON complet, utile aux tests et au débogage), `GET /api/events`
      (SSE, `keep-alive` 15 s), `POST /api/descend {geohash}`, `POST /api/reinstate
      {channels | all}`. Réponses d'erreur JSON avec le code `Error` (ex. `too_many_channels`
      avec l'effectif).
- [ ] Diffusion : une tâche lit `subscribe()`, calcule les deltas au plus une fois par
      `push_every` (défaut 1 s) et les publie sur un `broadcast` vers les flux SSE.
- [ ] **Protections** (D13) :
  - écoute sur `127.0.0.1:8080` par défaut ; une adresse non locale exige `--allow-remote`
    et produit un WARN au démarrage ;
  - en-tête `Host` vérifié contre l'adresse d'écoute (DNS rebinding) ;
  - `POST` acceptés seulement avec `Content-Type: application/json` et un `Origin` égal au
    serveur (une autre page ouverte dans le navigateur ne peut pas commander le moniteur) ;
  - en-têtes `Content-Security-Policy` (scripts et styles `'self'`, images limitées à l'hôte
    des tuiles), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` ;
  - taille des corps de requête bornée ; nombre de flux SSE simultanés borné (64).
- [ ] Logs `tracing` : démarrage (adresse), chaque commande reçue (sans en-têtes), refus.

Tests (`tests/server.rs`, `TestRelay` + `reqwest`) : page et ressources servies ; `hello` puis
delta reçu en moins de 2 s après publication d'un événement ; descente par `POST` → nouvelles
cellules dans le flux ; `POST` sans `Origin` ou avec un autre `Origin` → 403 ; mauvais
`Host` → 421 ; corps trop gros → 413 ; arrêt → flux fermés.

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
      couleur selon l'échelle choisie : participants ou messages (D16) ; « faible » (sous *k*)
      hachuré sans chiffre ; canal retiré : contour gris pointillé, sans remplissage ; canal
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
      distincte de l'activité propre du canal ; *k* appliqué comme pour les cellules.

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
