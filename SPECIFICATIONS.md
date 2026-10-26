# bitchat-geohash : spécifications

## Objectifs

À partir du travail déjà effectué dans la crate `bitchat-envelope` (exemples `geohash_map` et
`geohash_bot`, helpers `examples/common/{geo,probe}.rs`), implémenter un outil permettant de
visualiser **en direct** la répartition de l'activité BitChat dans les canaux geohash d'une zone,
sur une carte OpenStreetMap.

## Rappels du protocole qui contraignent la conception

* **Un geohash est un canal, pas un sous-ensemble de son parent.** Un message du canal `u09tv`
  porte le seul tag `g=u09tv`. Chaque niveau doit donc être observé séparément, et chaque canal a
  son propre jeu de relais (les 5 plus proches de *son* centre).
* **Niveaux de canal de l'app** : 2 (région), 4 (province), 5 (ville), 6 (quartier), 7 (bloc). Le
  niveau 3 n'est pas un niveau de canal : il ne sert ici que de maille d'agrégation sur la carte.
  
* **Kinds 20000 (message) et 20001 (présence) éphémères** : les relais les relaient en direct et
  ne les stockent presque pas. L'outil est un **moniteur**, pas un scanner d'historique ; la
  `since` initiale (1 h) ne ramène qu'un complément marginal.
* **Présence partielle** : l'app n'émet de heartbeat qu'aux précisions 2, 4 et 5. Au niveau 6, un
  participant n'est visible que s'il écrit. Les compteurs de présence ne sont pas comparables
  d'un niveau à l'autre ; l'interface doit le signaler.
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

  Au-delà, on ne descend qu'à la demande (clic sur une cellule, cf. §4).

### 2. Plan d'écoute : inversion canal → relais

* Pour chaque canal, calculer les relais iOS et Android (`select_both`, égalités comprises).
* Probe des relais une fois au démarrage (`probe.rs`) ; les relais morts sont écartés, pas remplacés.
* Inverser en `relais → [canaux]`, puis ouvrir **une connexion par relais** avec des filtres
  `{"kinds":[20000,20001], "#g":[…]}`, découpés en paquets (taille de filtre et nombre de
  souscriptions par connexion bornés, valeurs configurables).
* Lecture seule : aucune publication, pas d'AUTH NIP-42 (comme l'app), reconnexion avec backoff,
  plafond global de connexions.

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

### 4. Carte et interaction

* Page Leaflet sur tuiles OSM (fournisseur configurable, attribution affichée), reprenant
  `common/map_overlay.html`.
* Chaque cellule est un rectangle coloré selon une échelle (participants ou messages, au choix).
* Mise à jour poussée en direct (WebSocket ou SSE), pas de rechargement.
* **Survol** d'une cellule : affichage du modèle du §3.
* **Clic** : descente d'un niveau (énumération et écoute à la demande des enfants).
* Calque optionnel « relais » : position des relais, colorés par verdict, comme dans `geohash_map overlay`.

### 5. Vie privée (non-objectifs)

* Seuls des **compteurs** sont exposés : ni contenu, ni nickname, ni pubkey dans l'interface.
* Aucun suivi d'une pubkey d'une cellule à l'autre.
* Aucun contenu de message n'est stocké ; les pubkeys ne sont gardées en mémoire que le temps
  de la fenêtre.
* Au niveau 6, les cellules de moins de *k* participants (défaut 3) sont affichées comme
  « activité faible », sans chiffre.

## Plan

1. **Librairie Rust** (`bitchat-geohash`)
   * Extraire `geo.rs` et `probe.rs` de `bitchat-envelope/examples/common` dans la lib (les
     exemples existants l'utilisent ensuite).
   * Énumération (§1), plan d'écoute (§2), agrégation (§3).
   * Tests hors ligne : énumération, sélection et inversion des relais, agrégation avec
     `MockRelay` / `MockBitchat` (feature `mock` de `bitchat-envelope`).
2. **Serveur web interne en Rust** (axum + tokio) : sert la page, pousse les mises à jour
   (§4). Un binaire CLI unique ; pas de Node.js.
3. **Interface Swift** (macOS, iOS)
   * Liaison via `uniffi`, à mutualiser avec la couche FFI prévue pour `bitchat-envelope`.
   * Première version : `WKWebView` sur la page de l'étape 2 ; MapKit natif ensuite si besoin.
   * iOS : écoute au premier plan seulement (pas de WebSockets en arrière-plan).
   * Le paquet Swift vit dans un sous-dossier `swift/`, séparé de la crate.

## Entrées du CLI (étape 2)

```
bitchat-geohash <geohash> [--depth N] [--levels 4,5,6] [--max-cells N] [--window 15m]
    [--relays-csv online_relays_gps.csv] [--android-csv nostr_relays.csv]
    [--no-probe | --no-drop-dead] [--listen 127.0.0.1:8080] [--tiles <url>]
```

## Critères d'acceptation

* `u09` profondeur 2 (canaux 4c et 5c, 1 056 canaux) : plan d'écoute calculé en moins d'une
  seconde, nombre de connexions égal au nombre de relais distincts vivants.
* Un message envoyé par `geohash_bot` dans un canal observé apparaît sur la carte en moins de
  5 s, dans la bonne cellule et elle seule.
* Aucune donnée personnelle (pubkey, nickname, contenu) dans les messages envoyés au navigateur.
