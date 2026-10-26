# Étape 1 : librairie Rust : plan de réalisation et avancement

Référence : [SPECIFICATIONS.md](SPECIFICATIONS.md), §1 à §3 bis et §5.
Règle : seuls les fichiers de `bitchat-geohash/` sont modifiés ; `bitchat-envelope` est une
dépendance par chemin, en lecture seule.

**Légende** : ⬜ à faire · 🟨 en cours · ✅ fait · ⏸ en attente d'une décision

## Tableau de bord

| Lot | Contenu | État |
|---|---|---|
| 0 | Mise en place de la crate | ✅ |
| 1 | Annuaire de relais et sélection par canal | ✅ |
| 2 | Énumération des canaux | ✅ |
| 3 | Plan d'écoute dynamique (inversion canal → relais, retrait, réintégration) | ✅ |
| 4 | Persistance SQLite et migrations | ✅ |
| 5 | Probe des relais | ✅ |
| 6 | Agrégation et détection d'inactivité | ✅ |
| 7 | Écoute des relais (WebSocket) | ⬜ |
| 8 | Rechargement des listes de relais toutes les 24 h | ⬜ |
| 9 | Exemple `watch` et validation sur le réseau réel | ⬜ |

Ordre : 0 → 1 → 2 → 3 sont hors réseau et s'enchaînent ; 4, 5 et 6 sont indépendants entre
eux ; 7 dépend de 3 à 6 ; 8 dépend de 7 ; 9 clôt l'étape.

## Architecture visée

```
src/
  lib.rs         ré-exports, constantes du protocole (kinds 20000/20001, GEO_RELAY_COUNT = 5)
  error.rs       Error (thiserror)
  directory.rs   RelayDirectory : CSV analysé une seule fois, Platform { Ios, Android }, source
  select.rs      closest(), select_both() → Vec<Selected>        (repris de examples/common/geo.rs)
  channels.rs    Level, enumerate(start, depth, levels, max) → Vec<String>
  plan.rs        ListenPlan : relais → [canaux], filtres, retrait/réintégration, diff entre deux plans
  store/         Store (rusqlite) : connexion, migrations, requêtes
    migrations/  0001_init.sql, 0002_….sql (embarquées par include_str!)
  probe.rs       probe() → Verdict { status: Status, text }    (repris de examples/common/probe.rs)
  aggregate.rs   Aggregator : déduplication, fenêtre glissante, inactivité, CellStats
  listener.rs    une tâche par relais, Observation envoyées par mpsc, application des diffs de plan
  monitor.rs     Monitor : assemble le tout, API reinstate(), rechargement 24 h
examples/
  watch.rs       CLI terminal : plan, probe, écoute, tableau rafraîchi, commandes de réintégration
tests/
  data/          CSV figés (iOS et Android)
  select.rs  channels.rs  plan.rs  store.rs  aggregate.rs  listener.rs  monitor.rs
```

Flux : `RelayDirectory` + `enumerate` → `ListenPlan` → `probe` (écarte les morts) →
`listener` (Observation) → `Aggregator` → `snapshot()` (compteurs seulement) et `Store`.
Boucle de contrôle du `Monitor` : canaux inactifs → retrait du plan → diff appliqué aux
connexions ; demande de réintégration → ajout au plan → diff ; toutes les 24 h → rechargement
des listes → nouveau plan → diff.

## Lots

### Lot 0 : mise en place de la crate ✅

- [x] `Cargo.toml` : `bitchat-envelope` (chemin), `geohash 0.13`, `tokio`, `tokio-tungstenite`
      (rustls, racines webpki), `rustls` (ring), `futures-util`, `serde`, `serde_json`,
      `thiserror`, `tracing`, `rusqlite` (feature `bundled`), `reqwest` (rustls, pour D5),
      `sha2` (empreintes des listes) ; en dev : `bitchat-envelope` avec
      la feature `mock-ws`, `tracing-subscriber`.
- [x] Remplacer le `add()` généré par les modules vides et `error.rs`.
- [x] Déplacer le paquet Swift (`Package.swift`, `Sources/`, `Tests/`, `.gitignore`) dans
      `swift/`, comme prévu par la spec.
- [x] Copier les deux CSV dans `tests/data/` : ils restent figés, tous les tests les utilisent
      (`tests/data_frozen.rs` vérifie l'en-tête et le nombre de lignes : 441 iOS, 381 Android).
- [x] `CHANGELOG.md`, `.gitignore` de la crate (bases SQLite, `.env`).

Fini quand : `cargo build -p bitchat-geohash` et `cargo test -p bitchat-geohash` passent.

### Lot 1 : annuaire de relais et sélection par canal ✅

- [x] `RelayDirectory::parse(csv)` : normalisation de l'app (`wss://`, `/` final, `:443`,
      minuscules), doublons retirés, lignes invalides ignorées et comptées.
      Doublon = même hôte **et** mêmes coordonnées ; un hôte listé avec d'autres coordonnées
      est gardé, la sélection prend son entrée la plus proche (comme `geo.rs`). Coordonnées
      hors bornes rejetées (nouveau par rapport à `geo.rs`).
- [x] `closest(dir, geohash, count)` : tri par distance puis par hôte, égalités du dernier
      rang incluses (comportement identique à `geo.rs`).
- [x] `select_both(ios, android, geohash)` : union, rang par plateforme.
- [x] Le CSV est analysé une seule fois, pas à chaque canal.
- [x] Geohash invalide → `Error::InvalidGeohash` (au lieu d'une liste vide dans `geo.rs`).

Tests (CSV figés, `tests/select.rs`, 11 tests) : résultats identiques à `geohash_map` sur
`u09tv`, `u09`, `9q8yy`, `gcpvj`, `u0`, égalités au 5ᵉ rang, liste absente pour une plateforme, CSV vide ou corrompu.

### Lot 2 : énumération des canaux ✅

- [x] `Level` : niveaux de canal 2, 4, 5, 6 par défaut, 7 sur demande (D1) ; le niveau 3 est une
      maille d'agrégation seulement.
- [x] `enumerate(start, depth, levels, max)` : canaux des niveaux demandés contenus dans `start`,
      `start` compris s'il est un niveau de canal ; `depth` compte des niveaux de canal.
- [x] Validation (`normalize`, qui passe aussi en minuscules) : alphabet geohash (base32 sans `a i l o`), longueur 2 à 7, minuscules.
- [x] Dépassement de `max` (défaut 2 048) : `Error::TooManyChannels { count, max }`, compté
      avant toute construction (`count()`) (D2).

Tests (`tests/channels.rs`, 10 tests) : effectifs du tableau de la spec (32, 1 024, 32 768 pour le comptage seul), `u09`
profondeur 2 → 1 056 canaux, départ de niveau 3, `levels` filtrant, niveau 7 explicite,
entrées invalides.

### Lot 3 : plan d'écoute dynamique ✅

- [x] `ListenPlan::build(dir_ios, dir_android, channels)` : sélection par canal, puis
      inversion `relais → [canaux]` (ordre déterministe).
- [x] Divergence par canal : relais choisis par une seule plateforme.
- [x] `subscriptions(relay)` / `Subscription::filter(since)` : `{"kinds":[20000,20001], "#g":[…], "since": now-1h}` découpés en
      paquets de 100 canaux, au plus 10 REQ par connexion (D3, paramètres ajustables).
- [x] `drop_relays(dead)` : retire les relais morts sans les remplacer.
- [x] `retire(channels)` / `reinstate(channels)` / `reinstate_all()` : un canal retiré sort des filtres ; un relais
      sans canal disparaît du plan.
- [x] `diff(old, new)` → par relais : à connecter, à déconnecter, REQ à remplacer (même id de
      souscription, ce qui remplace le filtre côté relais).
- [x] Groupes stables : les canaux d'un relais (retirés compris) sont découpés une fois pour
      toutes en groupes de 100 ; retirer ou réintégrer un canal ne change que la souscription de
      son groupe (`geoN`), et un groupe vidé est fermé (`CLOSE`).
- [x] Au-delà de 10 souscriptions, un relais reçoit une connexion de plus
      (`connections(relay)`, affectation stable par groupe) : voir D7.

Tests (`tests/plan.rs`, 11 tests + 1 de chrono) : un canal → ses relais ; deux canaux voisins → relais partagés, une seule entrée par
relais ; découpage aux bornes ; retrait du dernier canal d'un relais → déconnexion dans le
diff ; réintégration → retour à l'état initial ; plan de `u09` profondeur 2 calculé en moins
d'une seconde (en `--release`).

### Lot 4 : persistance SQLite et migrations ✅

- [x] `Store::open(path)` et `Store::open_in_memory()` (tests) ; WAL et clés étrangères activés.
- [x] Migrations versionnées : fichiers `store/migrations/NNNN_nom.sql` embarqués, appliqués
      dans l'ordre, chacun dans une transaction ; version courante dans `PRAGMA user_version`.
      Refus d'ouvrir une base plus récente que le code (erreur explicite). Pas de migration
      descendante : une correction se fait par une nouvelle migration.
- [x] `0001_init.sql` :
  - `relay_list_load` (plateforme, source, date, empreinte SHA-256, nb de relais, nb de lignes rejetées) ;
  - `relay_list_csv` (empreinte, contenu) : chaque contenu distinct une seule fois, repli de D5 ;
  - `probe_result` (relais, date, statut, texte) ;
  - `channel` (geohash, niveau, état `watched`/`retired`, date du dernier changement, motif) ;
  - `cell_bucket` (geohash, début de tranche, messages, présences, participants) en tranches
    d'une minute.
- [x] Écritures depuis le code async : `StoreWriter`, un thread dédié qui reçoit les écritures
      par canal et les regroupe en transactions (512 au plus) ; un lot en échec est rejoué
      écriture par écriture pour ne pas perdre les autres ; `flush()` rend la première erreur.
- [x] Aucune pubkey, aucun nickname, aucun contenu dans le schéma (§5), vérifié par un test.

Tests (`tests/store.rs`, 10 tests ; migrations en tests unitaires, 5 tests) : base vide → dernière version ; base à la version N → migrée jusqu'à la dernière ;
base plus récente → erreur ; migration en échec → base inchangée (rollback) ; schéma sans
colonne interdite.

### Lot 5 : probe des relais ✅

- [x] Reprise de `probe.rs`, avec `status` en `enum Status { Ok, Auth, Refused, Down, Dead, Unprobed }`.
- [x] Un probe par relais (et non par canal), sur le premier canal de sa liste,
      concurrence bornée (par défaut 32) : `probe_relays(plan, …)`.
- [x] Les relais `Auth` comptent comme inutilisables par l'app, mais restent affichés :
      `Status::worth_listening()` écarte `Dead` et `Auth` ; `Down` et `Refused` restent écoutés
      (reconnexion avec backoff au lot 7).
- [x] Résultats enregistrés dans `probe_result` (`Verdict::to_write`).
- [x] `install_crypto_provider()` : ring installé une fois pour `wss://` et HTTPS.
- [x] `normalize_url` garde un `ws://` explicite (relais de test locaux ; absent des listes publiées).

Tests (`tests/probe.rs`, 9 tests ; serveurs de test dans `tests/common/`) : `WsRelay` local → `Ok` ;
port fermé → `Dead` ; hôte inconnu → `Dead` ; HTTP 404 → `Dead`, 503 → `Down` ; pas de réponse →
`Down` ; `CLOSED auth-required:` → `Auth` ; autre `CLOSED` → `Refused` ; challenge AUTH seul → `Ok`.

### Lot 6 : agrégation et détection d'inactivité ✅

- [x] `Aggregator::observe(Observation { relay, event }, now)` : signature vérifiée
      (`Event::verify`, D4), kind 20000/20001 uniquement, tag `g` présent dans le plan, sinon
      rejet compté (`Rejection`, `rejected()`). Signature vérifiée seulement à la première
      réception d'un id : les copies ne font que noter le relais qui les a livrées.
- [x] Déduplication par `id` ; mémorisation des relais qui ont livré chaque canal.
- [x] Fenêtre glissante (défaut 15 min) sur `created_at`, horloge injectable pour les tests ;
      purge des pubkeys et des `id` sortis de la fenêtre (en mémoire seulement).
- [x] `snapshot(now, plan, verdicts)` → `Vec<CellStats>` (`geohash`, `level`, `messages`, `presences`,
      `participants`, `last_seen`, `relays`, `divergence`) : **aucune pubkey dans le type**.
- [x] Seuil *k* = 3 au niveau 6 : `participants` vaut `None` (« faible »). En base, les
      compteurs par minute restent bruts (des nombres, sans identifiant).
- [x] Somme des enfants (`mesh(precision, now)`) pour les mailles parentes (niveau 3 compris), distincte de l'activité propre.
- [x] `inactive(now)` (délai par défaut 5 min) : canaux sans événement depuis le délai, compté
      depuis le dernier événement ou, à défaut, depuis le début de l'observation du canal
      (`watch()` le remet à zéro). Activité = `created_at` plafonné à l'heure de réception :
      un horodatage dans le futur ne garde pas un canal en vie.
- [x] Compteurs par minute transmis au `Store` (`drain_buckets(now)`) : minutes complètes
      seulement, une minute recomptée si un événement en retard y arrive (dans la fenêtre).

Tests (`tests/aggregate.rs`, 11 tests) : même événement reçu de 3 relais → compté une fois ; signature invalide rejetée ;
événement d'un canal hors plan rejeté ; sortie de fenêtre ; seuil *k* ; somme des enfants ;
canal muet 5 min → inactif ; canal réintégré → délai reparti de zéro.

### Lot 7 : écoute des relais ⬜

- [ ] Une tâche tokio par relais : connexion, REQ du plan, lecture `EVENT`/`EOSE`/`CLOSED`/`NOTICE`.
- [ ] Application d'un diff de plan à chaud : REQ remplacées, `CLOSE` des souscriptions vides,
      déconnexion des relais sans canal, connexion des nouveaux.
- [ ] `AUTH` ignoré (comme l'app) ; `CLOSED auth-required:` → relais marqué `Auth`, pas de retry.
- [ ] Reconnexion avec backoff exponentiel (1 s → 5 min, avec gigue) ; relais `Dead` abandonné.
- [ ] État de connexion par relais exposé (connecté, en attente, abandonné).
- [ ] Arrêt propre (canal de fermeture) ; plafond global de connexions.
- [ ] `Monitor` : boucle de contrôle qui retire les canaux inactifs (lot 6) et applique le diff ;
      `reinstate(channels)` et `reinstate_all()` ; état des canaux lu et écrit dans `channel`,
      donc conservé après un redémarrage (D6) ; option `--reinstate-all` au lancement.
- [ ] Journal `tracing` niveau INFO pour chaque retrait (« canal u09tv retiré : inactif depuis
      5 min ») et chaque réintégration.

Tests (`WsRelay` local, feature `mock-ws`, horloge injectée) : événement publié → Observation
reçue ; fermeture du serveur → reconnexion ; REQ découpées conformes au plan ; canal muet →
retiré des REQ et log INFO émis ; dernier canal d'un relais retiré → déconnexion ;
réintégration → REQ rétablie ; redémarrage du `Monitor` → canaux retirés toujours retirés.

### Lot 8 : rechargement des listes de relais toutes les 24 h ⬜

- [ ] Source de chaque liste : URL officielle par défaut, ou chemin local (D5) ; dernier
      contenu valide gardé en base comme repli (téléchargement en échec au démarrage).
- [ ] Rechargement une fois toutes les 24 h (délai configurable, désactivé dans les tests) ;
      chaque chargement enregistré dans `relay_list_load`.
- [ ] Si l'empreinte n'a pas changé : rien à faire. Sinon : nouveau plan, probe des nouveaux
      relais seulement, diff appliqué.
- [ ] Échec du rechargement : on garde la liste en cours, log WARN, nouvel essai au cycle suivant.

Tests : liste modifiée → diff attendu (relais ajouté, relais retiré) ; liste identique → aucun
diff ; source en échec → plan inchangé.

### Lot 9 : exemple `watch` et validation réelle ⬜

- [ ] `cargo run -p bitchat-geohash --example watch -- u09 --depth 2` : affiche le plan
      (canaux, relais, divergence), les verdicts de probe, puis un tableau des cellules actives
      rafraîchi toutes les 5 s ; les retraits apparaissent dans le journal INFO.
- [ ] Commandes sur l'entrée standard : `reinstate <geohash>…` et `reinstate all`.
- [ ] Validation : un message de `geohash_bot` dans un canal observé apparaît en moins de 5 s,
      dans la bonne cellule et elle seule (critère de la spec).
- [ ] Mesures notées dans le journal : nombre de relais vivants, d'événements par minute,
      nombre de canaux retirés après 5 et 15 min, mémoire, taille de la base.

Fini quand : critères d'acceptation de la spec vérifiés, sauf ceux qui concernent le navigateur
(étape 2).

## Décisions

| # | Question | Décision | État |
|---|---|---|---|
| D1 | Niveau 7 (bloc) observé ? | Non par défaut, activable via `levels` | ✅ |
| D2 | `max` dépassé : erreur ou troncature ? | Erreur avec l'effectif réel | ✅ |
| D3 | Taille des filtres | 100 `#g` par filtre, 10 REQ par connexion, ajustables après mesures (lot 9) | ✅ |
| D4 | Vérifier les signatures ? | Oui ; à revoir si le lot 9 montre un goulot | ✅ |
| D5 | Source des listes rechargées toutes les 24 h | Par défaut les URL officielles (`bitchat/relays/online_relays_gps.csv` et `georelays/nostr_relays.csv`), dernier contenu valide gardé en base comme repli ; un chemin local reste possible | ✅ |
| D6 | Les canaux retirés le restent-ils après un redémarrage ? | Oui (état en base), avec une option `--reinstate-all` au lancement | ✅ |
| D7 | Un relais avec plus de 1 000 canaux (100 × 10) : `u09` profondeur 2 donne 14 relais mais 22 connexions (8 relais de Paris à 1 042–1 056 canaux, 11 souscriptions chacun). Le critère de la spec « connexions = relais distincts » n'est donc pas tenu | Plusieurs connexions par relais si besoin, au plus 10 souscriptions chacune ; critère de la spec corrigé ; le lot 9 dira si les relais l'acceptent | ✅ |

## Risques

- **Limites des relais non documentées** (taille de filtre, nombre de REQ, débit) : les valeurs
  de D3 sont des hypothèses, à mesurer au lot 9.
- **Peu de trafic réel** : les canaux sont souvent vides ; la validation passe par `geohash_bot`.
- **Retrait rapide des canaux** : la plupart des canaux sont muets, donc la plupart seront
  retirés au bout de 5 min ; une activité qui démarre ensuite dans un canal retiré n'est pas
  vue tant qu'il n'est pas réintégré. Comportement voulu, à quantifier au lot 9.

## Journal

| Date | Lot | Avancement |
|---|---|---|
| 26/10/2026 | — | Plan initial. |
| 26/10/2026 | — | Ajouts : retrait des canaux inactifs (5 min) et réintégration explicite, CSV figés en test et rechargés toutes les 24 h, persistance SQLite avec migrations versionnées. D1 à D4 validées. Nouveaux lots 4 (SQLite) et 8 (rechargement), lots renumérotés. |
| 26/10/2026 | — | D5 et D6 validées. `AGENTS.md` créé. |
| 26/10/2026 | 0 | Crate en place : dépendances (reqwest 0.13 sans fournisseur crypto embarqué, ring installé au démarrage, cf. lot 8), modules vides, `Error`, constantes, paquet Swift dans `swift/` (`swift build` OK), CSV figés. fmt, tests, clippy `-D warnings` : OK. |
| 26/10/2026 | 1 | `directory.rs` et `select.rs` ; sorties identiques à `geohash_map --no-probe` sur 5 canaux (dont égalités iOS et Android). Listes figées : iOS 326 relais (115 doublons), Android 332 (49 doublons), 0 ligne rejetée. Erreur WebSocket mise en `Box` (clippy `result_large_err`). |
| 26/10/2026 | 2 | `channels.rs` : `Level`, `normalize`, `count`, `enumerate`, `children`. La profondeur compte les niveaux de canal de l'app, observés ou non (depuis `u0`, profondeur 1 = précision 4) ; le filtre `levels` s'applique ensuite. Ordre : niveau le plus grossier d'abord, ordre geohash dans un niveau. |
| 26/10/2026 | 3 | `plan.rs` : `ListenPlan`, `Subscription`, `PlanDiff`. `u09` profondeur 2 : 1 056 canaux → 14 relais, plan en 130 ms (release). API publique sans durée de vie explicite (règle FFI d'AGENTS.md). D7 validée, critère de la spec corrigé. |
| 26/10/2026 | 4 | `store/` : `Store`, `StoreWriter`, migrations (`PRAGMA user_version`), `0001_init.sql`. Les heures sont toujours fournies par l'appelant (horloge injectée). La colonne du CSV s'appelle `csv` pour que le test de vie privée puisse interdire `content`. |
| 26/10/2026 | 5 | `probe.rs` repris avec `Status`, `probe_relays` (un probe par relais du plan) et l'enregistrement en base. Serveurs de test réutilisables (`tests/common/`) pour le lot 7. |
| 26/10/2026 | 6 | `aggregate.rs` : `Aggregator`, `CellStats`, `MeshStats`, `Rejection`. Aucune pubkey dans ce qui sort (test sur la sortie `Debug`). Lots 4, 5 et 6 commités séparément. |
