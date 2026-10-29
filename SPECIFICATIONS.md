# bitchat-geohash : spécifications

## Objectifs

À partir du travail déjà effectué dans un prototype antérieur, non publié (exemples
`geohash_map` et `geohash_bot`, fonctions `geo.rs` et `probe.rs`), implémenter un outil permettant de
visualiser **en direct** la répartition de l'activité BitChat dans les canaux geohash d'une zone,
sur une carte OpenStreetMap.

## Rappels du protocole qui contraignent la conception

* **Un geohash est un canal, pas un sous-ensemble de son parent.** Un message du canal `u09tv`
  porte le seul tag `g=u09tv`. Chaque niveau doit donc être observé séparément, et chaque canal a
  son propre jeu de relais (les 5 plus proches de *son* centre).
* **Niveaux de canal de l'app** : 2 (région), 4 (province), 5 (ville), 6 (quartier), 7 (bloc). Le
  niveau 3 n'est pas un niveau de canal : il ne sert ici que de maille d'agrégation sur la carte.
  Le niveau 7 n'est pas observé par défaut ; il peut être demandé explicitement.
* **Kinds 20000 (message) et 20001 (présence) éphémères** : les relais les relaient en direct et
  ne les stockent presque pas. L'outil est un **moniteur**, pas un scanner d'historique ; la
  `since` initiale (1 h) ne ramène qu'un complément marginal.
* **Présence partielle** : l'app n'émet de heartbeat qu'aux précisions 2, 4 et 5. Au niveau 6, un
  participant n'est visible que s'il écrit. Les compteurs de présence ne sont pas comparables
  d'un niveau à l'autre ; l'interface doit le signaler. Les heartbeats ne partent que vers les
  canaux de la **position GPS réelle** de l'appareil, jamais vers un canal où l'utilisateur
  s'est téléporté : un utilisateur téléporté n'est compté que s'il écrit. iOS 1.8.0 :
  `GeohashPresenceService` (un heartbeat toutes les 40 à 80 s, app au premier plan et Tor
  prêt) envoie vers `LocationStateManager.availableChannels`, calculé par `computeChannels`
  depuis le GPS seul ; présences et messages d'un canal sont signés par la même clé, dérivée
  du geohash (`deriveIdentity(forGeohash:)`), donc une personne compte une fois. Vérifié le
  27/10/2026 sur `#u09ky` (message téléporté sans présence, avec observation `nak`).
* **Deux listes de relais** : iOS lit `online_relays_gps.csv` (relue, en retard), Android
  `nostr_relays.csv` (amont, quotidienne). Les deux apps peuvent choisir des relais différents
  pour un même canal ; l'outil écoute l'union et mesure la divergence.

## Principes

### 1. Énumération des canaux

* Entrée : un geohash de départ (2 à 6 caractères), une **profondeur** (nombre de niveaux
  de canal à descendre, défaut 1), la liste des niveaux à observer.
* Sortie : les canaux de chaque niveau observé contenus dans le geohash de départ, y compris le
  départ lui-même s'il est un niveau de canal.
* Garde-fou : un plafond sur le nombre de canaux (défaut 2 048). Ordres de grandeur :

  | Départ | → 4c | → 5c | → 6c |
  |---|---|---|---|
  | 2c | 1 024 | 32 768 | 1 048 576 |
  | 4c | — | 32 | 1 024 |
  | 5c | — | — | 32 |

  Un dépassement est une erreur qui donne l'effectif réel. Au-delà, on ne descend qu'à la
  demande (clic sur une cellule, cf. §4).

### 2. Plan d'écoute : inversion canal → relais

* Listes de relais : hors tests, rechargées une fois toutes les 24 h ; le plan est recalculé et
  seules les différences sont appliquées aux connexions. Dans les tests, les CSV sont figés
  (copies dans `tests/data/`) pour la reproductibilité.
* Pour chaque canal, calculer les relais iOS et Android (`select_both`), chacun avec la règle de
  son app : iOS dédoublonne par hôte normalisé et trie par (distance, hôte) ; Android garde les
  lignes brutes (`host` et `host:443` distincts) et trie par distance seule, ordre du CSV en cas
  d'égalité ; 5 relais exactement, sans ex æquo au-delà. Une liste iOS où un hôte figure à deux
  coordonnées est refusée comme par l'app, au profit de la dernière liste valide (D18 de l'étape 2).
* Probe des relais une fois au démarrage (`probe.rs`) ; les relais morts sont écartés, pas remplacés.
* Inverser en `relais → [canaux]`, puis ouvrir **une connexion par relais** avec des filtres
  `{"kinds":[20000,20001], "#g":[…]}`, découpés en paquets (taille de filtre et nombre de
  souscriptions par connexion bornés, valeurs configurables).
* Lecture seule : aucune publication, pas d'AUTH NIP-42 (comme l'app), reconnexion avec backoff,
  plafond global de connexions.

### 2 bis. Retrait des canaux inactifs

* Un canal sans aucun événement (message ou présence) depuis un délai configurable (défaut
  **15 min**, compté depuis le début de son observation) est **retiré du monitoring** : il sort
  des filtres envoyés aux relais, et un relais qui n'a plus de canal est déconnecté.
* Il n'est réintégré que sur **demande explicite** (un canal, une liste, ou tous).
* Chaque retrait et chaque réintégration est journalisé au niveau **INFO**.
* L'état (surveillé ou retiré) est conservé en base et survit à un redémarrage.

### 3. Agrégation

* Les événements sont dédupliqués par `id` (un même événement arrive par plusieurs relais).
* Fenêtre glissante configurable (défaut 15 min) ; un participant est « actif » s'il a été vu
  dans la fenêtre.
* Modèle par cellule :

  | Champ | Contenu |
  |---|---|
  | `geohash`, `level` | identité du canal |
  | `messages`, `presences` | nombre d'événements de chaque kind dans la fenêtre |
  | `participants` | nombre de pubkeys distinctes dans la fenêtre |
  | `last_seen` | horodatage du dernier événement |
  | `relays` | relais iOS et Android, rang, verdict du probe, relais ayant livré des événements |
  | `divergence` | relais choisis par une seule des deux apps |

* Agrégat optionnel « somme des enfants » pour les mailles parentes (dont le niveau 3),
  distinct de l'activité propre du canal.
* Signatures vérifiées ; un événement hors plan (tag `g` inconnu) est rejeté.

### 3 bis. Persistance

* Base **SQLite** locale.
* Schéma **versionné** : migrations numérotées, appliquées dans l'ordre au démarrage, chacune
  dans une transaction ; la version courante est enregistrée dans la base.
* Contenu : chargements des listes de relais, verdicts de probe, état de chaque canal
  (surveillé ou retiré), compteurs agrégés par cellule et par tranche de temps.
* Conformément au §5 : ni pubkey, ni nickname, ni contenu de message en base.

### 4. Carte et interaction

* Page Leaflet sur tuiles OSM (fournisseur configurable, attribution affichée), reprenant
  la page `map_overlay.html` du prototype.
* Chaque cellule est un rectangle coloré selon une échelle (participants ou messages, au choix).
* Mise à jour poussée en direct (WebSocket ou SSE), pas de rechargement.
* **Survol** d'une cellule : affichage du modèle du §3.
* **Clic** : descente d'un niveau (énumération et écoute à la demande des enfants). Les
  canaux ajoutés comptent dans le plafond, sont écoutés aussitôt et conservés après un
  redémarrage (sauf `--forget-descents`).
* **Aperçu** au survol prolongé d'une cellule : les canaux du niveau inférieur sont écoutés
  pour un temps (5 min après le dernier survol), dessinés dans la cellule et résumés dans
  l'infobulle, sans rien enregistrer ; une descente les garde (D21 de `PLAN_ET2.md`). Comme
  les événements sont éphémères, l'aperçu ne montre que l'activité qui arrive pendant l'écoute.
* Calque optionnel « relais » : position des relais, colorés par verdict, comme dans le mode `overlay` de `geohash_map` (prototype).

### 5. Vie privée (non-objectifs)

* Seuls des **compteurs** sont exposés : ni contenu, ni nickname, ni pubkey dans l'interface.
* Aucun suivi d'une pubkey d'une cellule à l'autre.
* Aucun contenu de message n'est stocké ; les pubkeys ne sont gardées en mémoire que le temps
  de la fenêtre.
* Tous les compteurs sont affichés, niveau 6 compris (D20 de l'étape 2). Un seuil *k* reste
  disponible en option (`AggregatorConfig::k_threshold`, défaut 0 = aucun) : au niveau 6, une
  cellule de moins de *k* participants affiche alors « activité faible » au lieu de leur nombre.

## Plan

1. **Librairie Rust** (`bitchat-geohash`)
   * Reprendre `geo.rs` et `probe.rs` du prototype par copie dans la lib (la sélection est
     depuis dans [geohash-relays](https://github.com/wvmscs/geohash-relays)). La crate ne
     dépend pas du prototype : le sous-ensemble NIP-01 utile
     (événement, vérification de signature, filtre) est dans `src/nostr.rs`.
   * Énumération (§1), plan d'écoute (§2), agrégation (§3).
   * Tests hors ligne : énumération, sélection et inversion des relais, agrégation avec
     un relais NIP-01 de test intégré (`tests/common/relay.rs`).
2. **Serveur web interne en Rust** (axum + tokio) : sert la page, pousse les mises à jour
   (§4). Un binaire CLI unique ; pas de Node.js.
3. **Interface Swift** (macOS, iOS)
   * Liaison via `uniffi`.
   * Première version : `WKWebView` sur la page de l'étape 2 ; MapKit natif ensuite si besoin.
   * iOS : écoute au premier plan seulement (pas de WebSockets en arrière-plan).
   * Le paquet Swift vit dans un sous-dossier `swift/`, séparé de la crate.

## Entrées du CLI (étape 2)

```
bitchat-geohash <geohash> [--depth N] [--levels 4,5,6] [--max-cells N] [--window 15m]
    [--inactivity 15m] [--relays-csv online_relays_gps.csv]
    [--android-csv nostr_relays.csv | --no-android] [--no-probe] [--db <fichier>]
    [--reinstate-all] [--forget-descents] [--listen 127.0.0.1:8080] [--allow-remote]
    [--tls-cert <pem> --tls-key <pem> | --insecure-remote |
     --behind-proxy https://<nom> [--proxy-from 127.0.0.1/8,::1] [--no-proxy-check]]
    [--tiles <url> --tiles-attribution <html>] [--open] [--verbose]
```

* Au démarrage, résumé sur stderr (canaux, relais et verdicts) puis l'URL de la carte.
* Code de sortie 2 pour une ligne de commande fausse, 1 si le moniteur ou le serveur ne
  peut démarrer (geohash invalide, plafond, aucune liste, port occupé), avec un message.

* Les relais morts sont toujours écartés, jamais remplacés : pas d'option pour les garder.
* Écoute locale (`127.0.0.1`) par défaut ; une autre adresse exige `--allow-remote` et TLS
  (`--insecure-remote` pour s'en passer, avec avertissement). TLS permet HTTP/2.
* Derrière un proxy TLS (D25, `--behind-proxy https://<nom>`) : HTTP clair vers le proxy
  seulement. Chaque requête doit venir d'une adresse de `--proxy-from` (par défaut la boucle
  locale), dire qu'elle est arrivée en HTTPS (`X-Forwarded-Proto`, `Forwarded` ou `CF-Visitor`)
  et porter le nom public (`Host`, `Origin` des commandes). Au démarrage, une sonde demande
  `https://<nom>/.well-known/bitchat-proxy-check` et attend le jeton tiré à ce démarrage ;
  sinon la carte ne démarre pas (`--no-proxy-check` pour s'en passer). Pas de `--allow-remote`
  sans TLS ici ni proxy. Configurations : [docs/behind-proxy.md](docs/behind-proxy.md).
* Détail des options et décisions : [PLAN_ET2.md](PLAN_ET2.md).

## Critères d'acceptation

* `u09` profondeur 2 (canaux 4c et 5c, 1 056 canaux) : plan d'écoute calculé en moins d'une
  seconde, une connexion par relais distinct vivant, plus une par tranche de 10 souscriptions
  au-delà de la première (un relais très chargé peut recevoir plusieurs connexions).
* Un message publié dans un canal observé (relais de test, clé jetable) apparaît sur la carte en moins de
  5 s, dans la bonne cellule et elle seule.
* Aucune donnée personnelle (pubkey, nickname, contenu) dans les messages envoyés au navigateur.
