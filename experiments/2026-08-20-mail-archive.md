# Expérimentation mail-archive — 2026-08-20

## Objectif et environnement

Prototype local, Linux x86_64, `rustc 1.96.0`, `cargo 1.96.0`, SQLite système
3.53.4 pour contrôle externe. Le crate utilise `rusqlite 0.40.2` bundled,
`tantivy 0.26.1` et `flate2 1.1`. Les données sont générées avec seed `42`.

## Commandes reproductibles

```text
cargo fmt --all
cargo check --workspace
cargo test -p mail-archive-experiment
cargo build -p mail-archive-experiment --release
target/release/mail-archive-experiment benchmark --messages 10000 --seed 42 --attachment-rate 0 --queries 20 --out /tmp/mail-archive-bench-release3
/usr/bin/time -f 'wall_seconds=%e max_rss_kb=%M exit=%x' target/release/mail-archive-experiment benchmark --messages 3000 --seed 42 --attachment-rate 30 --max-attachment-bytes 1048576 --queries 10 --out /tmp/mail-archive-bench-release-att
MAIL_ARCHIVE_SEGMENT_BYTES=4096 cargo run -p mail-archive-experiment -- generate --messages 100 --seed 42 --attachment-rate 0 --out /tmp/mail-archive-segmented
cargo run -p mail-archive-experiment -- recover-demo --messages 10 --out /tmp/mail-archive-recover
```

## Résultats mesurés

### 10 000 messages, sans pièces jointes, release

```text
raw_bytes=7713414
archive_bytes=8033414
compressed_bytes=3828252
min_bytes=402 median_bytes=655 max_bytes=2597
import_ms=765
sqlite_index_ms=60
tantivy_index_ms=64
sqlite_bytes=9584640
tantivy_bytes=2472518
sqlite_hot_bytes=282624
tantivy_hot_bytes=97528
sqlite_p50_us=1995 sqlite_p95_us=12532 sqlite_p99_us=12650
tantivy_p50_us=122 tantivy_p95_us=957 tantivy_p99_us=1008
sqlite_hot_p50_us=91 sqlite_hot_p95_us=256 sqlite_hot_p99_us=270
tantivy_hot_p50_us=44 tantivy_hot_p95_us=99 tantivy_hot_p99_us=125
sqlite_date_hits=268 tantivy_date_hits=20
max_rss_kb=445324 wall_seconds=1.85
```

`tantivy_date_hits` est limité aux 20 premiers résultats, contrairement au
compte SQLite ; ce champ vérifie le chemin de requête de date, pas l’égalité
des cardinalités.

### 3 000 messages, taux de pièces jointes 30 %, release

```text
raw_bytes=153568272
archive_bytes=153664272
compressed_bytes=152507034
attachments=930 unique_attachment_hashes=132
import_ms=3340
sqlite_index_ms=319 tantivy_index_ms=811
sqlite_bytes=2895872 tantivy_bytes=806455
sqlite_p50_us=636 sqlite_p95_us=3811 sqlite_p99_us=3845
tantivy_p50_us=123 tantivy_p95_us=495 tantivy_p99_us=554
max_rss_kb=53316 wall_seconds=4.87
```

La compression gzip économise environ 50 % sur le corpus textuel sans
pièces jointes, mais moins de 1 % sur ce corpus avec payloads binaires
pseudo-aléatoires. Ce résultat ne justifie pas une compression aveugle de
l’archive ; une politique par type ou par bloc devra être mesurée.

### Segmentation et reprise

Avec `MAIL_ARCHIVE_SEGMENT_BYTES=4096`, 100 messages produisent 24 segments,
aucun message n’est réparti entre deux frames. Le format ajoute 32 octets par
message (`magic`, id, longueur, checksum). `recover-demo` ajoute 17 octets
incomplets et vérifie : `recovered_frames=10`, `truncated_bytes=17`,
`archive_tail_recovered=true`.

Les offsets du catalogue permettent de lire un message dans un segment sans
parcourir les autres frames ; un test Rust couvre cette propriété.

## Réponses provisoires aux questions

1. **Stockage robuste et simple :** l’append-only segmenté avec bytes bruts,
   checksum et catalogue séparé paraît le meilleur socle. SQLite convient au
   catalogue mais ne doit pas devenir l’unique copie des originaux.
2. **Segmentation :** 64 MiB est un défaut raisonnable de prototype ; des
   segments de quelques KiB sont trop nombreux, tandis qu’un fichier géant
   complique sauvegarde, corruption et reprise. La valeur devra être mesurée
   avec les tailles de messages réelles.
3. **Compression :** prometteuse pour texte homogène, presque inutile pour
   les pièces jointes déjà compressées/aléatoires. Décision ouverte, pas de
   compression obligatoire dans le format actuel.
4. **SQLite FTS5 :** déjà très utilisable à 10 000 messages, avec champs,
   phrases, Unicode et filtres structurés. Le p95 release mesuré est cependant
   sensible aux requêtes fréquentes ; la baseline doit être testée à plusieurs
   millions.
5. **Tantivy :** avantage significatif dans cette expérience : p95 lexical
   release environ 0,96 ms contre 12,5 ms pour SQLite et index plus petit.
   Cette conclusion est provisoire car les options de stockage des champs ne
   sont pas symétriques.
6. **Index chaud :** prometteur : sur 10 000 messages, le p95 passe à 0,256 ms
   SQLite et 0,099 ms Tantivy, avec des index très petits. Il faut vérifier le
   coût de fusion, la couverture des requêtes et la maintenance incrémentale.
7. **Extrapolation 300 Go :** le ratio de 32 octets/frame et les coûts de
   segments sont extrapolables arithmétiquement. Les latences, RSS, fsync,
   merges et sauvegardes ne le sont pas depuis 10 000 messages.
8. **Décisions pouvant être figées :** originaux immuables, métadonnées et
   index dérivés séparés, offsets de catalogue, checksum de frame, index
   reconstructibles et classement séparé de la récupération.
9. **Décisions ouvertes :** format exact du catalogue, taille des segments,
   compression, déduplication, SQLite ou Tantivy par défaut, politique des
   index chauds, tokenizer multilingue et stockage du texte dans les index.
10. **Prochaine expérience minimale :** corpus de 100 000 messages sans
    pièces jointes puis avec une distribution d’attachements issue de mesures
    réelles, sur SSD, avec arrêt/reprise et benchmark après fermeture/réouverture
    des index. Elle réduira mieux l’incertitude que de viser immédiatement
    300 Go synthétiques.

## Faits, hypothèses, décisions et limites

- **Fait vérifié :** génération déterministe, trois tests Rust, frames
  checksum, reprise de queue et recherches SQLite/Tantivy fonctionnent.
- **Décision de projet :** le prototype n’écrit aucune donnée originale dans
  les index de recherche comme source d’autorité.
- **Hypothèse :** la segmentation 64 MiB restera pratique à grande échelle.
- **Hypothèse infirmée partiellement :** « la compression est toujours
  rentable » est faux dès que les pièces jointes dominent les bytes.
