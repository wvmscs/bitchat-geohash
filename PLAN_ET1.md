# Étape 1 : librairie Rust : plan de réalisation et avancement

Référence : [SPECIFICATIONS.md](SPECIFICATIONS.md), §1 à §3 bis et §5.
Règle : seuls les fichiers de `bitchat-geohash/` sont modifiés ; `bitchat-envelope` est une
dépendance par chemin, en lecture seule.

**Légende** : ⬜ à faire · 🟨 en cours · ✅ fait · ⏸ en attente d'une décision

## Tableau de bord

| Lot | Contenu | État |
|---|---|---|
| 0 | Mise en place de la crate | ⬜ |
| 1 | Annuaire de relais et sélection par canal | ⬜ |
| 2 | Énumération des canaux | ⬜ |
| 3 | Plan d'écoute dynamique (inversion canal → relais, retrait, réintégration) | ⬜ |
| 4 | Persistance SQLite et migrations | ⬜ |
| 5 | Probe des relais | ⬜ |
| 6 | Agrégation et détection d'inactivité | ⬜ |
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

### Lot 0 : mise en place de la crate ⬜

- [ ] `Cargo.toml` : `bitchat-envelope` (chemin), `geohash 0.13`, `tokio`, `tokio-tungstenite`
      (rustls, racines webpki), `rustls` (ring), `futures-util`, `serde`, `serde_json`,
      `thiserror`, `tracing`, `rusqlite` (feature `bundled`), `reqwest` (rustls, pour D5),
      `sha2` (empreintes des listes) ; en dev : `bitchat-envelope` avec
      la feature `mock-ws`, `tracing-subscriber`.
- [ ] Remplacer le `add()` généré par les modules vides et `error.rs`.
- [ ] Déplacer le paquet Swift (`Package.swift`, `Sources/`, `Tests/`, `.gitignore`) dans
      `swift/`, comme prévu par la spec.
- [ ] Copier les deux CSV dans `tests/data/` : ils restent figés, tous les tests les utilisent.

Fini quand : `cargo build -p bitchat-geohash` et `cargo test -p bitchat-geohash` passent.

### Lot 1 : annuaire de relais et sélection par canal ⬜

- [ ] `RelayDirectory::parse(csv)` : normalisation de l'app (`wss://`, `/` final, `:443`,
      minuscules), doublons retirés, lignes invalides ignorées et comptées.
- [ ] `closest(dir, geohash, count)` : tri par distance puis par hôte, égalités du dernier
      rang incluses (comportement identique à `geo.rs`).
- [ ] `select_both(ios, android, geohash)` : union, rang par plateforme.
- [ ] Le CSV est analysé une seule fois, pas à chaque canal.

Tests (CSV figés) : résultats identiques à `geohash_map` sur quelques canaux (`u09tv`, `u09`,
`9q8yy`), égalités au 5ᵉ rang, liste absente pour une plateforme, CSV vide ou corrompu.

### Lot 2 : énumération des canaux ⬜

- [ ] `Level` : niveaux de canal 2, 4, 5, 6 par défaut, 7 sur demande (D1) ; le niveau 3 est une
      maille d'agrégation seulement.
- [ ] `enumerate(start, depth, levels, max)` : canaux des niveaux demandés contenus dans `start`,
      `start` compris s'il est un niveau de canal ; `depth` compte des niveaux de canal.
- [ ] Validation : alphabet geohash (base32 sans `a i l o`), longueur 2 à 7, minuscules.
- [ ] Dépassement de `max` (défaut 2 048) : erreur avec l'effectif réel (D2).

Tests : effectifs du tableau de la spec (32, 1 024, 32 768 pour le comptage seul), `u09`
profondeur 2 → 1 056 canaux, départ de niveau 3, `levels` filtrant, niveau 7 explicite,
entrées invalides.

### Lot 3 : plan d'écoute dynamique ⬜

- [ ] `ListenPlan::build(dir_ios, dir_android, channels)` : sélection par canal, puis
      inversion `relais → [canaux]` (ordre déterministe).
- [ ] Divergence par canal : relais choisis par une seule plateforme.
- [ ] `filters(relay)` : `{"kinds":[20000,20001], "#g":[…], "since": now-1h}` découpés en
      paquets de 100 canaux, au plus 10 REQ par connexion (D3, paramètres ajustables).
- [ ] `without_relays(dead)` : retire les relais morts sans les remplacer.
- [ ] `retire(channels)` / `reinstate(channels)` : un canal retiré sort des filtres ; un relais
      sans canal disparaît du plan.
- [ ] `diff(old, new)` → par relais : à connecter, à déconnecter, REQ à remplacer (même id de
      souscription, ce qui remplace le filtre côté relais).

Tests : un canal → ses relais ; deux canaux voisins → relais partagés, une seule entrée par
relais ; découpage aux bornes ; retrait du dernier canal d'un relais → déconnexion dans le
diff ; réintégration → retour à l'état initial ; plan de `u09` profondeur 2 calculé en moins
d'une seconde (en `--release`).

### Lot 4 : persistance SQLite et migrations ⬜

- [ ] `Store::open(path)` et `Store::open_in_memory()` (tests).
- [ ] Migrations versionnées : fichiers `store/migrations/NNNN_nom.sql` embarqués, appliqués
      dans l'ordre, chacun dans une transaction ; version courante dans `PRAGMA user_version`.
      Refus d'ouvrir une base plus récente que le code (erreur explicite). Pas de migration
      descendante : une correction se fait par une nouvelle migration.
- [ ] `0001_init.sql` :
  - `relay_list_load` (plateforme, source, date, empreinte SHA-256, nb de relais, nb de lignes rejetées) ;
  - `probe_result` (relais, date, statut, texte) ;
  - `channel` (geohash, niveau, état `watched`/`retired`, date du dernier changement, motif) ;
  - `cell_bucket` (geohash, début de tranche, messages, présences, participants) en tranches
    d'une minute.
- [ ] Écritures depuis le code async : un thread dédié qui reçoit les écritures par canal et les
      regroupe en transactions.
- [ ] Aucune pubkey, aucun nickname, aucun contenu dans le schéma (§5), vérifié par un test.

Tests : base vide → dernière version ; base à la version N → migrée jusqu'à la dernière ;
base plus récente → erreur ; migration en échec → base inchangée (rollback) ; schéma sans
colonne interdite.

### Lot 5 : probe des relais ⬜

- [ ] Reprise de `probe.rs`, avec `status` en `enum Status { Ok, Auth, Refused, Down, Dead, Unprobed }`.
- [ ] Un probe par relais (et non par canal), sur un canal quelconque de sa liste,
      concurrence bornée (par défaut 32).
- [ ] Les relais `Auth` comptent comme inutilisables par l'app, mais restent affichés.
- [ ] Résultats enregistrés dans `probe_result`.

Tests : `WsRelay` local → `Ok` ; port fermé → `Dead` ; hôte inconnu → `Dead`.

### Lot 6 : agrégation et détection d'inactivité ⬜

- [ ] `Aggregator::observe(Observation { relay, event })` : signature vérifiée
      (`Event::verify`, D4), kind 20000/20001 uniquement, tag `g` présent dans le plan, sinon
      rejet compté.
- [ ] Déduplication par `id` ; mémorisation des relais qui ont livré chaque canal.
- [ ] Fenêtre glissante (défaut 15 min) sur `created_at`, horloge injectable pour les tests ;
      purge des pubkeys et des `id` sortis de la fenêtre (en mémoire seulement).
- [ ] `snapshot()` → `Vec<CellStats>` (`geohash`, `level`, `messages`, `presences`,
      `participants`, `last_seen`, `relays`, `divergence`) : **aucune pubkey dans le type**.
- [ ] Seuil *k* = 3 au niveau 6 : `participants` remplacé par « faible ».
- [ ] Somme des enfants pour les mailles parentes (niveau 3 compris), distincte de l'activité propre.
- [ ] `inactive_since(delay)` (défaut 5 min) : canaux sans événement depuis le délai, compté
      depuis le dernier événement ou, à défaut, depuis le début de l'observation du canal.
- [ ] Compteurs par minute transmis au `Store` (`cell_bucket`).

Tests : même événement reçu de 3 relais → compté une fois ; signature invalide rejetée ;
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
