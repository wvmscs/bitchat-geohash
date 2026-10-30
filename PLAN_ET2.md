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
| 11 bis | Correction de la sélection des relais (une règle par app), report du prototype | ✅ |
| 12 | Modèle exposé au navigateur (JSON) et test de vie privée | ✅ |
| 13 | Serveur axum : page, état, flux SSE, commandes, protections | ✅ |
| 13 bis | TLS optionnel, HTTP/1.1 et HTTP/2 | ✅ |
| 14 | Page carte : cellules, échelle, survol, niveaux | ✅ |
| 15 | Interaction : descente au clic, réintégration, remontée | ✅ |
| 16 | Calques relais et mailles (somme des enfants) | ✅ |
| 17 | CLI `bitchat-geohash` | ✅ |
| 18 | Validation sur le réseau réel et dans le navigateur | ✅ |
| 19 | Changement de la zone surveillée en direct (pour bitchat-console), version 0.2.0 | ✅ |
| 20 | Changement de zone plus rapide (listes et verdicts réutilisés, sondage en arrière-plan), version 0.2.1 | ✅ |
| 21 | Annuaire et sélection pris dans `geohash-relays` (DRY), code redondant supprimé | ✅ |
| 22 | Derrière un proxy TLS (Cloudflare Tunnel, NGINX, Caddy) : contrôle par requête et sonde au démarrage | ✅ |
| 23 | Zone donnée par un lieu ou une IP (`geohash-lookup`, feature `geocode`), nom approximatif de la zone | ✅ |
| 24 | Version (tag ou commit) et nom de la zone sur la ligne du titre | ✅ |
| 25 | Changement de zone annoncé tout de suite à toutes les pages (`area`, `changing`) | ✅ |

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
- [x] Reprise de la page `map_overlay.html` du prototype par copie : styles
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

Report de [relay-selection-corrections.md](https://github.com/wvmscs/geohash-relays/blob/strategies/docs/relay-selection-corrections.md) (27/10/2026, copie publique dans geohash-relays) : la
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
l'affichage, comme dans le prototype.

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

### Lot 13 bis : TLS optionnel ✅

- [x] `ServerConfig::tls: Option<TlsFiles { cert, key }>` : certificat et clé PEM fournis par
      l'utilisateur (`--tls-cert`, `--tls-key`), par exemple produits par `mkcert` (D17).
- [x] `server::tls` : boucle d'acceptation `tokio-rustls` + `hyper-util` (`auto::Builder`),
      ALPN `h2` puis `http/1.1` ; poignée de main bornée (10 s) ; à l'arrêt, chaque
      connexion est fermée proprement. Fournisseur crypto `ring` (`install_crypto_provider()`).
- [x] Sans TLS : HTTP/1.1 simple (`axum::serve`), comportement par défaut sur `127.0.0.1`.
- [x] `allow_remote` exige TLS (`Error::RemoteWithoutTls`) ; `insecure_remote` le permet sans,
      avec un WARN au démarrage.
- [x] Contrôle `Host` aligné sur le certificat quand TLS est actif : le nom doit être couvert
      par ses SAN (`rustls::client::verify_server_name`) et le port être celui du serveur ;
      `Origin` attendu en `https://`. Pas de `Strict-Transport-Security`.
- [x] Erreurs claires (`Error::Tls`, avec le chemin) : fichier absent, PEM sans certificat ou
      sans clé, clé ne correspondant pas au certificat (vérifiée par rustls).
- [x] Recette `mkcert` documentée (`AGENTS.md`).
- [ ] Génération automatique d'un certificat autosigné (`rcgen`) : reportée à l'étape 3, si
      WKWebView en a besoin (validation par empreinte côté Swift).

Écarts : **aucune nouvelle dépendance**. Activer `http2` dans reqwest (client de test)
modifiait le `Cargo.lock` du workspace, hors périmètre : modification annulée. HTTP/2 est donc
vérifié par une poignée de main `tokio-rustls` (ALPN `h2` choisi) suivie du préambule HTTP/2 et
de la réponse `SETTINGS` du serveur ; le multiplexage des flux SSE est vérifié à la main avec
`curl --http2`. Le client `reqwest` des tests vérifie le certificat avec webpki
(`tls_certs_only`) : le vérificateur de macOS refuse les certificats valables plus de
825 jours, et celui des tests l'est 100 ans (`mkcert` en fait de 825 jours au plus).

Tests (`tests/tls.rs`, 5 tests) : certificats figés dans `tests/data/tls/` (autorité de test
dont la clé a été détruite, clés jetables, `README.md` de fabrication) ; page, `/api/state`,
flux SSE et commande en HTTPS (HTTP/1.1), `Origin` en clair refusé ; flux fermé à l'arrêt ;
`Host` : `localhost` et `127.0.0.1` acceptés, nom étranger, autre port ou port absent → 421 ;
ALPN `http/1.1` seul → `http/1.1`, `h2` proposé → `h2` et trame `SETTINGS` reçue ; fichier
absent, PEM sans certificat, fichier sans clé, clé discordante → `Error::Tls` explicite ;
adresse distante sans TLS → `Error::RemoteWithoutTls`.

### Lot 14 : page carte ✅

- [x] `index.html`, `app.js`, `app.css` sans étape de build (JavaScript simple, modules ES) ;
      fonctions pures (échelle, libellés, comptes par niveau) dans `view.js`, sans DOM,
      vérifiées par `node --test tests/js/view.test.mjs`.
- [x] Tuiles : URL et attribution reçues dans `hello` (`params`), configurables dans le binaire
      (`--tiles` et `--tiles-attribution`, ensemble ; défaut OSM) ; attribution toujours affichée.
- [x] Un rectangle par canal (bbox fournie par le serveur), couleur selon l'échelle choisie :
      participants ou messages, logarithmique de 1 au maximum du niveau affiché, palette
      viridis (D16) ; « faible » (sous *k*, si un seuil est réglé) hachuré (motif SVG) ; canal
      retiré : contour gris pointillé, sans remplissage ; canal sans activité : contour seul,
      discret.
- [x] **Niveaux superposés** : un niveau visible à la fois (une couche par niveau), le plus fin
      par défaut ; sélecteur avec, par niveau, cellules actives / toutes.
- [x] Survol : infobulle (à gauche ou à droite du pointeur selon la place) avec les compteurs,
      `last_seen` relatif, relais avec rang iOS et rang(s) Android (`#2+3`), statut coloré,
      livraison, et un avertissement de divergence. Avertissement fixe dans le panneau :
      présences non comparables entre niveaux.
- [x] Panneau : flux (vivant, connexion, reconnexion), départ, canaux surveillés / retirés,
      relais écoutés / écartés, connexions ouvertes, événements non comptés ; construit une
      fois, seuls textes et options changent (une liste ouverte reste ouverte) ; repliable,
      replié d'office sous 600 px.
- [x] Mise à jour en place (`setStyle`, couches jamais recréées ; recoloriage du niveau
      affiché seulement quand son maximum change) ; reconnexion `EventSource` automatique,
      reprise complète par `hello`, nouvelle connexion sur un trou de `seq`.
- [x] Flux qui ne s'ouvre pas (pas de `hello` en 5 s, ou refus 503) : « too many tabs open on
      this server: close one, or enable TLS (HTTP/2) », nouvel essai toutes les 5 s.
- [x] Clair et sombre (`prefers-color-scheme`), utilisable à 360 px ; textes en anglais ;
      icône vide (`data:,`) pour éviter la requête `favicon.ico`.

Écarts : les tuiles reçoivent l'origine de la page comme `Referer` (sinon OSM répond 403 sous Firefox, voir le journal). La CSP autorise `data:` pour les images (image vide de Leaflet pour les tuiles en
erreur, bloquée sinon). 

Vérifié dans Chrome headless piloté par le protocole DevTools (script hors dépôt), sur le
réseau réel, `u09` profondeur 2 (1 056 canaux) : **1 024 rectangles dessinés en 150 à 410 ms**
après le chargement, flux « live », infobulle complète, console sans erreur, en clair, en
sombre et à 360 px. Le `hello` de ce cas pèse 1,3 Mo (relais répétés par cellule) : sans
gêne en local ; compression ou relais par référence à envisager si la page sert à distance.
À vérifier à la main au lot 18 : message des 7 onglets en HTTP/1.1, reconnexion après un
redémarrage du serveur.

### Lot 15 : interaction ✅

- [x] Clic sur une cellule surveillée : menu « Descend to level N » (lot 11) ; refus du
      serveur affiché en clair, avec l'effectif réel pour le plafond (`refusalText`) ; au
      niveau 7 : « finest channel level: nothing below ».
- [x] Clic sur une cellule retirée : « Reinstate » ; bouton « Reinstate all retired
      channels » dans le panneau quand il y en a, avec confirmation (D15).
- [x] Après une descente, la vue passe au niveau des enfants et cadre la cellule ; fil
      d'Ariane `#départ (all) › #u09t0 › #u09t00` pour remonter (affichage seulement, la
      descente n'est pas annulée) ; « (all) » revient au niveau affiché avant la première
      descente.
- [x] Remontée depuis le menu de la cellule (demande du 27/10/2026 : seule la descente y
      figurait) : un bouton par niveau supérieur dessiné, du plus proche au plus lointain
      (« Up to #u09t00 · level 6 », « Up to #u09t0 · level 5 »…) ; affiche ce niveau, cadré sur
      la cellule parente.
- [x] Remontée par la liste des niveaux (ou l'URL) : la carte se recadre sur les cellules
      du niveau choisi visibles à l'écran, entières (demande du 27/10/2026 : l'échelle de la
      carte restait celle du niveau fin).
- [x] **Aperçu au survol** (D21, demande du 27/10/2026) : survoler une cellule surveillée
      400 ms demande au serveur d'écouter pour un temps les canaux du niveau inférieur
      (`POST /api/preview`, `Monitor::preview`) ; ils sont dessinés dans la cellule, sur leur
      propre échelle, sans capter le pointeur, et l'infobulle résume « level 6 below: N active
      / 32, messages, presences ». Rien n'est enregistré ; les canaux quittent le plan 5 min
      après la dernière demande (`preview_for`), ne sont jamais « retirés », cèdent la place
      (le plus ancien d'abord) sous le plafond, et une descente les garde.
- [x] Échelle et niveau gardés dans l'URL (`#level=5&scale=participants`), lus au chargement et
      à chaque changement d'URL (lien partagé entre onglets).
- [x] **Défaut corrigé** (trouvé en testant) : une cellule sans couleur était dessinée sans
      remplissage, et en SVG l'intérieur d'une forme non remplie ne reçoit ni survol ni clic ;
      elle a maintenant un remplissage transparent.

Tests : commandes couvertes au lot 13 ; fonctions pures (`childLevel`, `ancestors`,
`parseHash`, `formatHash`, `refusalText`) dans `tests/js/view.test.mjs` (6 tests `node`).
Parcours vérifié dans Chrome headless piloté par DevTools (script hors dépôt), sur le réseau
réel, `u09t` : vue lue dans l'URL, descente ville → quartier → bloc par clics (32 canaux
ajoutés à chaque fois côté serveur, niveau, URL et fil d'Ariane à jour), menu du niveau 7,
remontée par le fil d'Ariane et « (all) », changement d'URL externe appliqué, console sans
erreur. Restent pour le parcours manuel du lot 18 : refus au plafond affiché (le binaire n'a
pas encore `--max-cells`), réintégration d'une cellule retirée (15 min d'inactivité).

### Lot 16 : calques relais et mailles ✅

- [x] Calque « relais » (case « relays », désactivé par défaut) : position de chaque relais
      du plan, écouté ou écarté, couleur selon le statut (`ok`, `auth`, `refused`, `down`,
      `dead`, `unprobed`), rond si les deux apps le choisissent, carré si une seule, estompé
      s'il n'est pas écouté ; anneau pour les relais aux mêmes coordonnées ; fenêtre au clic
      (adresse, statut, apps, canaux, connexions). Survol d'une cellule → traits vers ses
      relais, pleins si les deux apps les choisissent, tiretés sinon.
- [x] Calque « mailles » (liste « sum of the channels inside (meshes) ») : somme des
      canaux contenus (`MeshStats`) aux niveaux 2 à 6, niveau 3 compris, distincte de
      l'activité propre du canal ; *k* (s'il est réglé) appliqué comme pour les cellules.
      Côté serveur : `meshes` dans `hello`, message SSE `meshes` (liste entière des mailles
      actives quand elle change), `Params.mesh_levels`. L'infobulle prévient que les
      participants d'une maille sont des clés distinctes (une clé par canal et par personne).
- [x] Calques et niveau de maille gardés dans l'URL (`&show=meshes`, `&relays=1`).

Tests : `MeshView` sous *k* → `"low"` (lot 12) ; mailles dans `hello` et en delta, envoyées
entières (`tests/model.rs`), et dans le test de vie privée ; flux réel `meshes` après un
événement (`tests/server.rs`) ; anneau et regroupement par coordonnées (`ringAnchors`,
`ringed`), URL (`tests/js/view.test.mjs`). Le cas « relais aux coordonnées partagées » est
traité côté page (`ringed`), `RelayView` ne change pas.

### Lot 17 : CLI `bitchat-geohash` ✅

- [x] Options de la spec : `<geohash> [--depth N] [--levels 4,5,6] [--max-cells N]
      [--window 15m] [--inactivity 15m] [--relays-csv …] [--android-csv … | --no-android]
      [--no-probe] [--db …] [--reinstate-all] [--forget-descents] [--listen 127.0.0.1:8080]
      [--allow-remote] [--insecure-remote] [--tls-cert <pem>] [--tls-key <pem>] [--tiles <url>] [--tiles-attribution <text>] [--open] [--verbose]`.
- [x] Analyse des arguments partagée avec `examples/watch.rs` (module `cli` interne, sans
      nouvelle dépendance) pour ne pas diverger. `cli::parse` lit les options du moniteur
      et passe les autres à l'appelant (`--listen`… pour le binaire, `--refresh`,
      `--duration` pour `watch`) ; messages d'erreur précis (« --depth needs a value »).
- [x] `--no-drop-dead` de la spec : non retenu (l'invariant 3 l'interdit : un relais mort
      n'est jamais remplacé, et l'écouter quand même ne sert à rien) ; spec corrigée (D14).
- [x] Au démarrage : plan et verdicts résumés sur stderr (canaux, relais avec statut, apps
      et nombre de canaux), puis `map: <url>` ; `--open` lance le navigateur (`open` /
      `xdg-open`). Journaux sans codes couleur hors terminal.
- [x] Code de sortie 2 pour une ligne de commande fausse (avec l'usage), 1 sinon, et message
      dans les termes du CLI : geohash invalide, plafond (« raise --max-cells »), aucune
      liste (« --relays-csv »), port occupé (vérifié avant les probes : « --listen »),
      adresse non locale (« --allow-remote »).

Tests : analyse des arguments (unitaires) ; démarrage du binaire sur listes figées et
`TestRelay`, `GET /api/state` répond (`tests/cli.rs`).

### Lot 18 : validation réelle ✅

- [x] Boucle fermée sur `$BITCHAT_TEST_RELAY` (liste à ce seul relais, `nak`, clé
      jetable) : l'événement apparaît **sur la carte** en moins de 5 s, dans la bonne cellule
      et elle seule (critère de la spec). Mesuré : 1,8 s, `u09tvw` seule.
- [x] Message de l'app BitChat de l'utilisateur dans un canal observé, puis descente au clic
      dans ce canal et second message à un niveau plus fin (`#u09tz` puis `#u09tzj`).
- [x] `u09` profondeur 2 sur les listes officielles, 30 min : fluidité, mémoire du serveur et
      du navigateur, débit SSE (octets par minute), retraits visibles sur la carte.
- [x] Contrôle des messages envoyés au navigateur (capture de `/api/events`) : aucune pubkey,
      aucun nickname, aucun contenu.
- [x] Safari (essai de l'utilisateur, 28/10/2026) ; Chromium ; largeur téléphone (360 px).
- [x] TLS, HTTP/2 négocié, 8 onglets ouverts sans blocage : vérifié dans Chrome avec le
      certificat de test du dépôt (Chrome dédié qui ne fait confiance qu'à cette clé). Reste
      l'essai dans le navigateur de l'utilisateur avec le certificat `mkcert` (fait au test de l'app ci-dessus) (installé le
      28/10/2026 ; `localhost+2.pem` créé dans la crate, ignoré par git ; `curl` : HTTP/2, 200).
- [x] Sans TLS : le critère « message too many tabs au 7ᵉ onglet » est **inatteignable** :
      au-delà de 6 flux, la page même du nouvel onglet ne se charge plus. Corrigé autrement
      (D22) : un onglet caché depuis 10 s libère son flux et le reprend quand il redevient
      visible.
- [x] Mesures et captures dans le journal ; capture d'écran dans le README (`docs/map.jpg`).

Fini quand : tous les critères d'acceptation de la spec sont vérifiés, navigateur compris.

### Lot 19 : changement de la zone surveillée ✅

Demandé par l'utilisateur le 28/10/2026 :
la carte doit suivre la cellule de bitchat-console (zone = parent de la cellule de la console),
avec toutes ses fonctions, et sa page doit permettre de changer de zone.

- [x] `POST /api/area {"geohash"}` : la zone est vérifiée d'abord (`monitor::check_area` :
      geohash, plafond de canaux), sans rien arrêter si elle est refusée ; puis le monitor est
      arrêté et relancé sur la nouvelle zone avec la même base (retraits, descentes et probes
      gardés ; seuls les canaux de la zone sont restaurés). Échec du démarrage : retour à la
      zone précédente, base rouverte. Même zone : rien. Une zone fixe (`serve`, sans
      `Relaunch`) répond `409 area_fixed` (D23).
- [x] Pages ouvertes : un `hello` complet sur leur flux, la numérotation continue ; la page
      repart de zéro (niveau, focus) et se cadre sur la nouvelle zone.
- [x] Page : champ « zone » et bouton « Watch » dans le panneau ; refus en clair.
- [x] Tests : changement atteignant le flux, commandes suivant la nouvelle zone, refus sans
      effet, zone fixe ; essai réel dans Chrome avec bitchat-console par-dessus (`u09t` →
      `u09w` depuis la console en 8,5 s, retour par le champ de la page).

### Lot 20 : changement de zone plus rapide ✅

Mesuré par l'utilisateur : un changement de zone affiche « changing… » une dizaine de secondes
(`sp` → `u0` : listes 0,5 s, plan 0,1 s, sondage de 75 relais 9,1 s).

- [x] Listes : demande de l'utilisateur, « inutile au sein de la session courante, et à
      limiter à 1 fois à 24 h (sauf demande explicite de rechargement) en cas de relance ».
      Au démarrage, une liste de la même source chargée il y a moins de `reload_every` est
      prise dans la base ; le rechargement suivant vient 24 h après ce chargement ;
      `--reload-lists` force le téléchargement ; un changement de zone ne le force jamais.
      Mesuré : listes en 1 ms au lieu de 0,5 s.
- [x] Sondages : chaque démarrage sondait tous les relais de la zone (8 s au plus par relais) ;
      un seul relais mort faisait attendre 8 s (`u09t` → `u09w` : 8,1 s, dont 8 s pour
      `relay.cypherflow.ai`). Précision de l'utilisateur : ce qui compte d'abord, ce sont les
      quelques relais du canal de la zone (`g` = la zone, une dizaine au pire) ; les autres
      servent aux statistiques et aux subdivisions et peuvent être sondés **après avoir rendu la
      main**. Fait (D24) : verdicts de moins d'une heure repris de la base ; avant d'écouter,
      seuls les relais du canal de la zone sans verdict récent sont sondés ; les autres en
      arrière-plan, écartés au fil des verdicts. Mesuré : `u09t` → `u09w` en 0,5 s (serveur
      10 ms) ; zone jamais vue (`spey`) en 0,5 s.
- [x] Page : après un changement de zone, niveau des subdivisions de la zone (et non celui
      gardé dans l'URL pour l'ancienne), bug signalé par l'utilisateur.

### Lot 21 : annuaire et sélection pris dans `geohash-relays` ✅

Prévu depuis le début (D2 de bitchat-console) : une seule copie du parsing des listes et de la
sélection des relais, celle de `geohash-relays`, que bitchat-console utilise déjà.

- [x] `directory` : `RelayDirectory`, `DirectoryRelay`, `Platform`, `normalize_url` et les URL
      des listes réexportés ; `ListSource`, `fetch` et le délai de téléchargement restent ici.
- [x] `select` : `GeoRelay`, `Selected` réexportés ; `geohash_center`, `closest`, `select_both`
      enveloppés pour garder l'`Error` de la crate (`From<geohash_relays::Error>`).
- [x] `GEO_RELAY_COUNT` réexporté ; `server::model` prend le rectangle d'une cellule dans
      `geohash_relays::geohash::bbox` ; dépendance `geohash` retirée.
- [x] `tests/select.rs` supprimé (copie identique de celui de `geohash-relays`).
- [x] Chemins publics inchangés ; 346 lignes supprimées, 54 ajoutées.

### Lot 22 : derrière un proxy TLS ✅

Demande de l'utilisateur (29/10/2026) : « le critère d'exigence pour le TLS est trop fort. il
suffit de l'assurance de disposer en amont d'un proxy mettant en œuvre un TLS et redirigeant
vers le port HTTP » ; configurations pour Cloudflare, NGINX et un autre proxy ; vérifier
l'existence du proxy (choix : par requête et par une sonde au démarrage).

- [x] `server::proxy` : `ProxyConfig` (nom public, `from`, `check`), `Net` (adresse ou réseau,
      IPv4 vue en IPv6 comprise), `Public` (`Host` du nom public, port 443 implicite),
      `forwarded_https` (`X-Forwarded-Proto`, `Forwarded`, `CF-Visitor` : au moins un, tous
      `https`), sonde `GET https://<nom>/.well-known/bitchat-proxy-check` (reqwest, 15 s,
      sans redirection) qui attend le jeton tiré au démarrage.
- [x] `guard.rs` : adresse du pair (`ConnectInfo`) dans `--proxy-from`, HTTPS en amont, nom
      public dans `Host` et `X-Forwarded-Host`, `Origin` des commandes = origine publique ;
      refus `403 not_from_proxy`, `403 not_https_upstream`, `421 wrong_host`.
- [x] `serve_with` : proxy et TLS exclusifs ; adresse non locale sans TLS admise derrière un
      proxy (avec `allow_remote`) ; sonde après le démarrage, arrêt propre et
      `Error::Proxy` si elle échoue ; `Server::public_url`.
- [x] Binaire : `--behind-proxy`, `--proxy-from` (répétable, liste), `--no-proxy-check` ; ligne
      `public: <url>` après `map:` ; `--open` ouvre le nom public.
- [x] Tests : 4 unitaires (noms, réseaux, en-têtes, jetons), 1 de ligne de commande, 3 de
      serveur (contrôles, autre adresse, sonde ratée / TLS / adresse distante).
- [x] `docs/behind-proxy.md` : Cloudflare Tunnel (et Access), NGINX, Caddy, proxy sur une
      autre machine ; README, spec.
- [x] Essais réels : Caddy 2.11.7 et NGINX 1.31.8 (certificat mkcert, `https://localhost:8443` et `:8444`) : sonde passée, page, commandes et flux SSE à travers les deux, HTTP/2 vers le client, accès direct au port refusé (`not_https_upstream`), mauvais nom refusé (421) ; fichier `cloudflared` validé (`ingress validate`).

### Lot 23 : zone donnée par un lieu, nom de la zone ✅

Demande de l'utilisateur (30/10/2026) : référencer la crate `geohash-lookup` dans Console et
Geohash pour les champs de saisie de cellule et les lignes de commande, saisir `#u09t` ou un
texte ; géocodage inverse affiché ; intégration sous forme de feature ; requêtes en direct
(sans Tor) ; base GeoIP en option, téléchargée si absente.

- [x] Feature `geocode` (par défaut), dépendance `geohash-lookup` `v0.1.1` (dépôt public,
      tag), `deny.toml` autorise son dépôt.
- [x] `geocode::Places` : `resolve` (`#u09t` et `u09t` tels quels, un lieu ou une IP vers la
      cellule la plus fine qui le contient, bornée par `area_bounds` : 2 à un niveau
      au-dessus du plus fin surveillé), `name` (géocodage inverse, mis en cache, 256 noms).
- [x] Serveur : `POST /api/area` résout le texte (réponse `place`), `GET /api/place`,
      `params.geocode`, erreurs `404 place_not_found` et `502 geocode` ; le texte tapé n'est
      jamais journalisé.
- [x] Binaire : zone de départ donnée par un lieu (ligne `area: #… for …` avec le crédit des
      données), `--geoip-db`.
- [x] Page : champ « area: u09t, a place or an IP address », nom approximatif sous le champ
      (crédit en infobulle).
- [x] Tests : 2 unitaires (`geocode`), 2 de serveur (hors ligne), 2 JS ; essai réel : « Rue de
      Rivoli, Paris » → `#u09tv` (« Paris, Ile-de-France… »), « Tokyo » → `xn`, texte inconnu
      → `place_not_found`, aucun texte dans les journaux.

### Lot 24 : version et nom sur la ligne du titre ✅

Demande de l'utilisateur (30/10/2026) : le résultat de `geohash-lookup` sur la ligne de « BitChat
geohash activity », avec la version (7 caractères du commit si non versionné).

- [x] `build.rs` : tag `v*` du commit, sinon 7 caractères du commit ; `BITCHAT_BUILD_VERSION`
      pour une copie sans `.git` (`release.sh` de bitchat-web), sinon la version de `Cargo.toml`.
- [x] `VERSION`, `params.version` ; page : version et nom dans l'en-tête du panneau (nom tronqué
      s'il est long, entier en infobulle).
- [x] Test de serveur ; vérifié dans le navigateur (`ff1f009`, « ≈ Hauts-de-Seine… »).
- [x] Retour de l'utilisateur (capture) : sur la ligne du titre il ne restait pas de place pour
      le nom. Le nom passe sur la ligne suivante, après la cellule (« live · #u09t ≈ … ») ; la
      version reste à côté du titre.

### Lot 25 : changement de zone annoncé tout de suite ✅

Demande de l'utilisateur (30/10/2026) : quand la console demande un changement de cellule, la
carte se met à jour immédiatement, geohash et bouton Watch indiquant le traitement en cours.

- [x] Le serveur annonce la zone (événement SSE `area`, numéroté ; `changing` dans `hello`, pour
      un onglet qui se connecte pendant le changement), une fois la demande vérifiée et son tour
      venu : rien pour un refus immédiat ni pour la zone déjà surveillée. Sans changement côté
      console : elle passe déjà par `POST /api/area`.
- [x] Page : champ sur la nouvelle cellule, bouton « ⏳ changing… » désactivé, « ⏳ #cellule… » sur
      la ligne de la zone, jusqu'au `hello` de la nouvelle zone ; retour en arrière si ça échoue.
- [x] Tests : modèle, serveur.

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
| D22 | HTTP/1.1 : 6 connexions par hôte, chaque onglet ouvert en garde une | Un onglet caché depuis 10 s ferme son flux SSE (« paused while the tab is hidden ») et le rouvre, avec un nouveau `hello`, quand il redevient visible ; HTTPS (HTTP/2, D17) reste la solution complète | ✅ |
| D21 | Aperçu du niveau inférieur au survol : écoute durable ou temporaire ? | Temporaire : canaux ajoutés au plan sans rien enregistrer, retirés du plan 5 min après la dernière demande (pas « retirés » au sens de l'invariant 5, qui ne vaut que pour les canaux surveillés pour de bon) ; ils comptent dans le plafond mais cèdent la place, le plus ancien d'abord, à un autre aperçu ou à une descente ; une descente les rend permanents. Délai d'appui 400 ms côté page, au plus une demande par cellule et par minute. Limite : les événements sont éphémères, l'aperçu ne montre que l'activité qui arrive pendant l'écoute (plus le peu que `since` ramène) | ✅ (à valider par l'utilisateur) |
| D23 | Changer la zone surveillée en direct (lot 19) | Arrêt et relance du monitor sur la nouvelle zone avec la même base (le code de démarrage, déjà éprouvé, plutôt qu'un replanning à chaud) ; les pages reçoivent un nouveau `hello` ; quelques secondes de coupure (listes rechargées, nouveaux relais sondés) ; pas de retour en arrière automatique si le démarrage échoue autrement qu'en revenant à la zone précédente — *prise par l'agent, à revoir* | ✅ |
| D24 | Sondage au démarrage (lot 20) | Verdicts de moins d'une heure repris de la base ; avant d'écouter, seuls les relais du canal de la zone sans verdict récent ; les autres en arrière-plan (un relais mort est écouté quelques secondes, ses connexions échouent, puis il est écarté) ; `probe_in_background` à faux rend l'ancien comportement — *précision de l'utilisateur, détails par l'agent* | ✅ |
| D25 | Derrière un proxy TLS (lot 22) | `--behind-proxy https://<nom>` remplace l'exigence de TLS dans le serveur : HTTP clair, mais seulement depuis les adresses du proxy (`--proxy-from`, boucle locale par défaut), avec la preuve d'un HTTPS en amont (`X-Forwarded-Proto`, `Forwarded`, `CF-Visitor`) et le nom public dans `Host`/`Origin`. Sonde au démarrage vers le nom public, jeton aléatoire, refus de démarrer sinon (`--no-proxy-check`) — *demande et choix de l'utilisateur, détails par l'agent, à valider* | ✅ |
| D26 | Zone donnée par un lieu (lot 23) | Feature `geocode` (par défaut) avec `geohash-lookup` ; requêtes en direct vers BAN et Nominatim (pas de Tor, choix de l'utilisateur), dit dans le README ; IP par fichier DB-IP local en option (`--geoip-db`, téléchargé si absent) ; un geohash nu reste un geohash (compatibilité), un mot mêlant lettres et chiffres qui n'en est pas un est refusé — *demande et choix de l'utilisateur, détails par l'agent* | ✅ |

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
| 27/10/2026 | 11 | `ListenPlan::extend`, `Monitor::descend` / `watch_channels` / `relays` / `subscribe`, migration `0002_channel_origin`. Probe des nouveaux relais en tâche de fond plutôt qu'avant l'écoute (pas d'attente au clic). 108 tests (104 sans la feature), stables sur 3 lancements. **Boucle fermée** sur `$BITCHAT_TEST_RELAY` : `watch u09tv --depth 0`, `descend u09tv` (32 canaux ajoutés sur la connexion ouverte, sans reconnexion), kind 20000 publié par `nak` avec une clé jetable dans `u09tvw` → compté dans `u09tvw` seul. |
| 27/10/2026 | — | Lot 11 commité (`730dd49`). Nouveau lot 11 bis : report des corrections de sélection du prototype. D18 (liste iOS en conflit refusée, repli) et D19 (rejet des coordonnées hors bornes) validées. |
| 27/10/2026 | 11 bis | Sélection des relais corrigée (une règle par app, plus d'ex æquo, `Selected.android: Vec<usize>`, union par serveur) ; D18 (liste iOS en conflit refusée, repli sur la dernière valide, au rechargement la liste courante est gardée) ; D19. Résultats identiques à `geohash_map` (prototype) corrigé sur `u09tv`, `u0`, `9q8yy`, `gcpvj`, `u1`, `gb`, `9q` ; `9q8yy` passe de 10 à 8 serveurs. 117 tests (113 sans la feature), stables sur 3 lancements ; clippy propre dans les deux configurations. Listes officielles du jour : iOS identique à `tests/data/` (sans conflit) ; `watch u09tv` réel : 8 serveurs, un relais mis de côté (`dead`). |
| 27/10/2026 | — | Lot 11 bis commité (`98a72e8`). |
| 27/10/2026 | 12 | `server::model` : vues, `Push` (SSE), `Published` (deltas). D20 validée : tous les chiffres affichés, seuil *k* désactivé par défaut (`k_threshold` 3 → 0, gardé en option ; seuls les participants passent à `"low"` s'il est réglé) ; spec §5 et invariant 1 corrigés. 123 tests (114 sans la feature), stables sur 3 lancements ; clippy propre dans les deux configurations. |
| 27/10/2026 | — | Lot 12 commité (`2bc7165`). |
| 27/10/2026 | 13 | `server::serve`, `Server`, routes `/api/*`, SSE, diffusion des deltas, protections D13, binaire minimal (geohash, `--listen`, `--allow-remote`, `--db`). 132 tests (114 sans la feature), stables sur 3 lancements ; clippy propre dans les deux configurations. Essai réel du binaire sur `u09tv` (listes officielles) : 33 cellules, 8 relais dont 1 écarté (certificat), `hello` puis deltas dans le flux, `Host` étranger → 421, Ctrl-C → arrêt propre. |
| 27/10/2026 | — | Lot 13 commité (`825ebc9`). |
| 27/10/2026 | 13 bis | TLS optionnel (`tokio-rustls` + `hyper-util`, ALPN `h2`/`http/1.1`), `Host` contrôlé par les SAN du certificat, `Error::Tls` et `Error::RemoteWithoutTls`, options `--tls-cert`, `--tls-key`, `--insecure-remote` du binaire. Aucune dépendance ajoutée (reqwest `http2` écarté : il modifiait le verrou du workspace). 137 tests (114 sans la feature), stables sur 3 lancements ; clippy propre ; tout en `--locked`. Essai réel du binaire en HTTPS (certificat de test) : page en HTTP/2 et HTTP/1.1, **8 flux SSE sur une seule connexion TCP** (`curl --http2 -Z`), HTTP en clair sur le port TLS rejeté, Ctrl-C → arrêt propre. |
| 27/10/2026 | — | Lot 13 bis commité (`cbcdaaa`). |
| 27/10/2026 | 14 | Page carte : `app.js` (couches par niveau, mise à jour en place, infobulle, panneau, flux SSE et reprise), `view.js` (fonctions pures, test `node`), `app.css` (clair/sombre, 360 px). Binaire : `--depth`, `--tiles`, `--tiles-attribution`. CSP : `data:` autorisé pour les images. 137 tests Rust (114 sans la feature), stables sur 3 lancements ; 3 tests `node`. Chrome headless sur `u09` profondeur 2 réel : 1 024 rectangles en moins de 0,5 s, console propre ; `hello` de 1,3 Mo noté comme risque. |
| 27/10/2026 | — | Lot 14 commité (`baed9df`). Vérification réelle sur `#u09ky` (2 participants affichés) avec `nak` et l'app iOS 1.8.0 de l'utilisateur : compte juste (l'utilisateur et `anon#9113`, présent sans écrire). Lu dans le code iOS : les présences ne partent que vers les canaux de la position GPS réelle, jamais vers un canal téléporté ; même clé par geohash pour présences et messages. Spec (« Présence partielle ») et `AGENTS.md` complétés. |
| 27/10/2026 | 15 | Menu au clic (descendre, réintégrer), « Reinstate all » avec confirmation, fil d'Ariane, vue dans l'URL. Défaut corrigé : cellules sans couleur non cliquables (intérieur SVG non rempli). 137 tests Rust (114 sans la feature), 6 tests `node`. Parcours dans Chrome headless sur `u09t` réel : descentes jusqu'au niveau 7, remontée, URL, console propre. |
| 27/10/2026 | 15 | Remontée ajoutée au menu de la cellule (un bouton par niveau supérieur), à la demande de l'utilisateur. Vérifié dans Chrome headless : bloc → « Up to … level 5 » affiche le niveau 5 cadré sur la ville parente, console propre. |
| 27/10/2026 | 14 | **Défaut corrigé** (signalé sous Firefox) : tuiles OSM « Access blocked » (403). La page envoie `Referrer-Policy: no-referrer`, que Firefox applique aux tuiles, or la politique d'usage d'OSM exige un `Referer`. Les tuiles seules reçoivent désormais l'origine de la page (option Leaflet `referrerPolicy: strict-origin-when-cross-origin`), jamais le chemin ni le `#level` ; le reste garde `no-referrer`. Vérifié dans Chrome (DevTools) : 12 requêtes de tuiles avec `Referer: http://127.0.0.1:…/`, toutes en 200. |
| 27/10/2026 | 15 | Demandes de l'utilisateur : (1) remonter recadre la carte (liste des niveaux, URL) ; (2) **aperçu au survol** (D21) : `ListenPlan::remove`, `Aggregator::forget`, `Monitor::preview`, `MonitorConfig::preview_for` (5 min), `MonitorEvent::Removed`, `CellStats.preview` / `CellView.preview`, `POST /api/preview` ; page : survol de 400 ms → aperçu dessiné dans la cellule, résumé dans l'infobulle, niveau vidé par la fin d'un aperçu → niveau plus grossier. 142 tests Rust (118 sans la feature), nouveaux tests stables sur 3 lancements ; 8 tests `node` ; clippy propre. Chrome headless sur `u09t` réel : survol de `#u09th` → 32 canaux en aperçu côté serveur, 32 rectangles dans la cellule, infobulle « listening… » puis « 0 active / 32 … (preview …) », rien après la sortie du pointeur ; liste 6 → 4 : cellule de 32 px à 1 024 px ; console propre. |
| 27/10/2026 | — | Lot 15 commité (`00a6640`). |
| 27/10/2026 | 16 | Calques relais (épingles par statut et plateforme, anneau, fenêtre, traits au survol) et mailles (niveaux 2 à 6, `meshes` dans `hello` et en delta, `Params.mesh_levels`, `Push::Hello.params` en `Box` pour clippy). Chrome headless sur `u09` profondeur 2 réel : 13 épingles, 8 traits au survol (6 tiretés : relais d'une seule app), mailles de niveau 4 (13 actives) et 3 (`#u09` : 72 canaux actifs, 222 présences), URL `&show=meshes&relays=1` relue, console propre. Défaut corrigé : infobulle écrasée au bord de la carte (largeur minimale). |
| 27/10/2026 | 17 | Module `cli` partagé avec `watch`, binaire complet (options de la spec, résumé au démarrage, `--open`, `--verbose`, codes de sortie et messages). `tests/cli.rs` : binaire lancé sur une liste locale et un `TestRelay`, `GET /api/state`, arrêt propre sur SIGINT ; échecs (option inconnue, geohash invalide, plafond, port occupé, adresse non locale, pas de liste). 156 tests Rust (123 sans la feature), stables sur 3 lancements, `--locked` ; 10 tests `node` ; clippy propre. Essai réel `u09tv` : résumé (7 relais écoutés, 1 écarté), port occupé → message et code 1, Ctrl-C → arrêt propre. |
| 27/10/2026 | — | Lots 16 (`ef893f1`) et 17 (`959b346`) commités. |
| 28/10/2026 | 18 | **Boucle fermée** (`u09tv`, liste au seul `$BITCHAT_TEST_RELAY`, kind 20000 de `nak` avec clé jetable dans `u09tvw`) : sur la page 1,8 s après le début de la publication, `u09tvw` seule modifiée, relais « delivered » ; ni pubkey, ni id, ni contenu dans l'état. **30 min sur `u09` profondeur 2** (listes officielles, 1 056 canaux, 12 relais écoutés, 1 écarté) : serveur 23 à 34 Mo (RSS) sans croissance ; onglet Chrome : tas JS 6 à 8,4 Mo, 1 024 rectangles, console propre ; flux SSE 7,2 Mo dont `hello` 1,47 Mo, puis ~200 Ko/min (1 604 messages : 473 `cells`, 465 `meshes`, 467 `status`, 198 `relays`) ; 974 canaux retirés après 15 min sans événement (journalisés), 4 relais déconnectés faute de canal surveillé ; 6 044 événements non comptés, presque tous des doublons entre relais. Capture du flux : aucune suite hexadécimale de plus de 10 caractères, aucune clé de champ personnelle. **HTTP/1.1** : au 6ᵉ-7ᵉ onglet, la page ne se charge plus du tout (le message prévu ne peut s'afficher) → D22, vérifié (5 onglets cachés « paused », 3 nouveaux en direct, l'onglet rendu visible reprend). **HTTPS** (certificat de test, Chrome dédié) : 8 onglets en direct, tout en `h2`, les 8 flux `/api/events` sur une seule connexion. 360 px : pas de défilement horizontal. Base SQLite : 136 Ko après 30 min. |
| 28/10/2026 | 18 | D22 validée. Safari essayé par l'utilisateur. `mkcert` installé par l'utilisateur ; certificat `localhost+2.pem` créé (ignoré par git) ; serveur en HTTPS : HTTP/2 et 200 sur `localhost` et `127.0.0.1`. Un serveur orphelin d'un essai raté de `tests/cli.rs` arrêté. |
| 28/10/2026 | 17 | **Défaut corrigé** (signalé par l'utilisateur, lancement depuis le dossier parent avec des chemins relatifs à la crate) : un certificat ou une clé introuvable n'était signalé qu'après les listes et les probes (8 s). Vérifié désormais avant tout, avec le dossier de départ des chemins relatifs ; test ajouté dans `tests/cli.rs`. |
| 28/10/2026 | 18 | **App réelle** (iOS 1.8.0 de l'utilisateur, carte en HTTPS `mkcert` sur `u09t`) : message dans `#u09tz` (niveau 5) → 1 message, 1 participant, livré par `nostr.wild-vibes.ts.net` et `relay-fra.zombi.cloudrodion.com` ; descente au clic dans `u09tz` ; message dans `#u09tzj` (niveau 6) → 1 message, 1 participant, livré par `nostr.wild-vibes.ts.net`. Aucune présence (utilisateur téléporté, conforme au comportement de l'app). 544 cellules de niveau 6 au total : la descente et des aperçus de survol (D21), temporaires. |
| 28/10/2026 | 18 | `README.md` ajouté (présentation, lancement HTTP et HTTPS, usage de la carte), capture `docs/map.jpg` (300 Ko, carte réelle de `u09t` en HTTPS, colorée par participants, relais affichés). Lot 18 terminé. |
| 28/10/2026 | — | Tag `v0.1.0` posé sur `961b88b` (version 0.1.0). |
| 28/10/2026 | 19 | Changement de zone en direct (`POST /api/area`, champ « zone » de la page), D23 ; version 0.2.0. Demande de l'utilisateur pour bitchat-console. 159 tests Rust, 10 JS ; Chrome headless : `u09t` → `u09w` en 8,5 s depuis la console, retour par le champ. |
| 28/10/2026 | 20 | Listes de relais prises dans la base si chargées il y a moins de 24 h (demande de l'utilisateur), `--reload-lists`. 160 tests. Mesure réelle : le changement `u09t` → `u09w` reste à 8,1 s, presque tout le sondage d'un relais mort ; vérification à la demande de l'utilisateur : 75 relais pour toute la zone `u0` (1 089 canaux), 6 à 9 par cellule. |
| 28/10/2026 | 20 | Sondages : verdicts récents repris, relais du canal de la zone d'abord, les autres en arrière-plan (D24) ; page : niveau des subdivisions après un changement de zone. 163 tests. Mesuré : démarrage 4 ms (listes et verdicts de la base), `u09t` → `u09w` 0,5 s, `spey` (jamais vue) 0,5 s. Version 0.2.1. |
| 28/10/2026 | — | Workspace Cargo propre (`[workspace]` dans `Cargo.toml`, `/target` ignoré), comme bitchat-console et bitchat-launcher, à la demande de l’utilisateur ; retiré du workspace local qui l’englobait. `Cargo.lock` de la crate à jour (il datait de la 0.1.0). Version 0.2.2. |
| 29/10/2026 | 21 | Annuaire et sélection pris dans `geohash-relays` (DRY), à la demande de l'utilisateur ; dépendance `geohash` retirée, `tests/select.rs` supprimé (copie). 147 tests Rust (111 sans la feature), 10 JS ; clippy propre dans les deux configurations ; `reuse lint` conforme. Version 0.3.0. |
| 29/10/2026 | 22 | Derrière un proxy TLS (D25, demande de l'utilisateur), sans nouvelle version. 155 tests Rust (111 sans la feature) ; clippy propre. Caddy 2.11.7 et NGINX 1.31.8 (certificat mkcert, `https://localhost:8443` et `:8444`) : sonde passée, page, commandes et flux SSE à travers les deux, HTTP/2 vers le client, accès direct au port refusé (`not_https_upstream`), mauvais nom refusé (421) ; fichier `cloudflared` validé (`ingress validate`). |
| 29/10/2026 | — | Dépendance à geohash-relays `v0.1.1` (ouverture au public, même code que `v0.1.0`), à la demande de l'utilisateur. 155 tests (111 sans la feature), clippy, `cargo deny`. |
| 29/10/2026 | — | Version 0.3.1 (tag `v0.3.1`), avant l'ouverture au public, à la demande de l'utilisateur. |
| 30/10/2026 | 23 | Zone donnée par un lieu ou une IP et nom approximatif de la zone (D26), feature `geocode` avec `geohash-lookup` `v0.1.1`, à la demande de l'utilisateur. 159 tests Rust (111 sans la feature), 11 JS ; clippy propre dans les trois configurations. Essai réel : « Rue de Rivoli, Paris » → `#u09tv`, « Tokyo » → `xn`. |
| 30/10/2026 | 24 | Version et nom de la zone sur la ligne du titre, à la demande de l'utilisateur. Vérifié dans le navigateur. |
| 30/10/2026 | 25 | Changement de zone annoncé tout de suite à toutes les pages, à la demande de l'utilisateur. 161 tests. Console sur la carte, `spey3` : carte à jour en 180 ms, nouvelle zone à 720 ms. |
| 30/10/2026 | 24 | Nom de la zone déplacé après la cellule (« live · #u09t ≈ … »), retour de l'utilisateur : plus de place sur la ligne du titre. |
| 30/10/2026 | — | Version 0.4.0 (tag `v0.4.0`), à la demande de l'utilisateur. |
