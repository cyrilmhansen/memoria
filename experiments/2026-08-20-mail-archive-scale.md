# Mail archive — changement d'échelle et réduction des biais

## Objectif et méthode

Cette expérience reprend le prototype du 2026-08-20 sans modifier sa
séparation archive/catalogue/index. Les indexeurs suivent désormais le chemin
réaliste `archive segmentée → catalogue → lecture ciblée → parsing → index`.
Le générateur n'est utilisé que pour créer l'archive initiale.

Mesures du 2026-08-20 sur Linux x86_64, build `--release`, avec Rust et les
versions déclarées dans `Cargo.lock` : rusqlite 0.40 (`bundled`), Tantivy
0.26.1, flate2 1.1 et zstd 0.13.

Commandes reproductibles :

```text
cargo build -p mail-archive-experiment --release
target/release/mail-archive-experiment benchmark --messages 100000 --seed 42 --attachment-rate 0 --queries 1 --segment-bytes 67108864 --out /tmp/mail-archive-100k-final
target/release/mail-archive-experiment benchmark --messages 1000000 --seed 42 --attachment-rate 0 --queries 1 --segment-bytes 67108864 --out /tmp/mail-archive-1m-64
target/release/mail-archive-experiment generate --messages 100000 --seed 42 --attachment-rate 0 --segment-bytes 16777216 --out /tmp/mail-archive-seg-16777216
target/release/mail-archive-experiment generate --messages 10000 --seed 42 --attachment-rate 30 --max-attachment-bytes 65536 --out /tmp/mail-archive-att-30
cargo test --workspace
```

Le cache système n'a pas été vidé artificiellement. `*_open_us` mesure une
ouverture d'artefact existant, puis `*_first_query_us` la première recherche.
Les percentiles sont ensuite mesurés sur des recherches répétées. Il s'agit
d'un test cold-open/warm-process, pas d'un cache-disque froid.

## Comparaison symétrique

Les deux moteurs indexent les mêmes champs : `sender`, `recipients`, `subject`,
`body`, `folder` et `account`, avec tokenizer Unicode. Tantivy stocke les
champs textuels et `doc_id`/date pour la restitution ; SQLite utilise une
table FTS5 avec contenu et une table `attrs` pour les attributs structurés.
Cette duplication SQLite est mesurée explicitement.

La recherche de date Tantivy renvoie les 20 meilleurs documents, alors que
`sqlite_date_hits` compte toutes les lignes : ces compteurs ne sont donc pas
comparables. La requête texte+date est néanmoins exécutée dans les deux
moteurs. Le classement reste celui du moteur et n'est pas couplé à l'archive.

## 100 000 messages

Corpus sans pièces jointes, seed 42, segments 64 MiB :

| mesure | valeur |
|---|---:|
| archive / bruts | 80,324,775 / 77,124,775 octets |
| min / médiane / max | 397 / 656 / 2,614 octets |
| import | 8,942 ms |
| lecture archive / parsing (deux indexations) | 538,801 / 467,480 µs |
| construction SQLite / Tantivy | 985 / 1,165 ms |
| catalogue | 30,986,240 octets |
| SQLite / Tantivy | 97,005,568 / 41,722,632 octets |
| RSS maximale | 113,972 KiB |
| ouverture SQLite / Tantivy | 64 / 714 µs |
| première requête `quartz` | 1,738 / 467 µs |

Sur 130 requêtes mélangées, p50/p95/p99 valent 8,854/56,926/57,783 µs
pour SQLite et 188/3,943/3,980 µs pour Tantivy. Pour le terme fréquent
`archive`, le p95 vaut 57,270 contre 1,897 µs ; pour le terme rare `quartz`,
1,536 contre 219 µs. Tantivy garde donc un avantage net, particulièrement
quand beaucoup de candidats doivent être classés.

L'index chaud des 270 derniers messages coûte 290,816 octets (SQLite) et
97,359 octets (Tantivy). Ses p95 sont 152 et 61 µs. Cela mesure deux index
indépendants ; le fallback chaud→global et la fusion restent ouverts.

## 1 000 000 messages

Même corpus, 64 MiB : archive 805,161,653 octets, bruts 773,161,653,
catalogue 311,488,512, SQLite 967,118,848 et Tantivy 250,582,139 octets.
Durée totale 1 min 16,2 s, RSS maximale 244,864 KiB, construction SQLite /
Tantivy 10,066 / 5,196 ms. Les p50/p95/p99 du mélange sont
62,552/398,847/407,468 µs pour SQLite et 355/15,296/15,386 µs pour Tantivy.

Un million reste raisonnable sur cette machine. Cela ne prouve pas la tenue de
centaines de millions : merges, checkpoints, fragmentation et collecteurs
peuvent changer de régime.

## Segmentation

Génération seule de 100 000 messages :

| cible | fichiers | import | archive |
|---:|---:|---:|---:|
| 16 MiB | 5 | 2,703 ms | 80,324,775 |
| 64 MiB | 2 | 2,775 ms | 80,324,775 |
| 256 MiB | 1 | 2,599 ms | 80,324,775 |

Il n'y a pas d'effet non linéaire visible. 64 MiB est une valeur de travail
raisonnable ; 16 MiB augmente le nombre de fichiers sans gain observé, et
256 MiB réduit la granularité de reprise et de sauvegarde. Ces deux derniers
coûts ne sont pas encore mesurés sur une vraie sauvegarde.

## Compression et déduplication

Sur 100 000 messages textuels : brut 77,124,775, gzip 38,404,869 et zstd
niveau 3 38,690,365 octets. Sur 10 000 messages avec 30 % de pièces jointes
pseudo-aléatoires : brut 106,382,143, gzip 102,800,685 et zstd 103,053,272.
La compression par message n'est donc pas convaincante sur ce corpus ; les
pièces jointes incompressibles dominent. Un corpus dédié de pièces jointes
compressibles reste nécessaire avant toute décision.

La mesure de contenu adressé, sans en faire le format de l'archive, donne
3,040 pièces jointes, 98,467,328 octets, 88 hashes et 1,436,160 octets
uniques pour 10 000 messages à 30 %. Le potentiel d'économie est important,
mais le coût d'accès, des blobs orphelins, des checksums et de la sauvegarde
n'est pas encore mesuré avec un CAS écrit sur disque.

## Crash, reprise et index chauds

La reprise valide les frames complètes puis tronque 17 octets de queue
incomplète. Une frame est lue par `(segment, offset, frame_bytes)` sans
parcourir un gros fichier. Une archive peut donc rester cohérente lorsqu'une
mise à jour catalogue n'est pas encore arrivée. Les index sont dérivés et
reconstructibles depuis catalogue+archive.

Les interruptions au milieu d'un commit SQLite/Tantivy et la sauvegarde
incrémentale multi-segments nécessitent encore une expérience dédiée.

## Projection prudente

À distribution identique, 1M messages représentent environ 0,805 Go d'archive,
0,311 Go de catalogue, 0,967 Go SQLite et 0,251 Go Tantivy. Une extrapolation
linéaire donne environ 4,03 Go d'archive pour 5M messages, mais pas une RSS ou
une latence fiable.

À 300 Go, la moyenne observée correspondrait à environ 373M messages. Cette
conversion n'est valable que pour la moyenne de ce corpus : pièces jointes,
messages réels, fragmentation, merges et maintenance peuvent l'invalider.
Les tailles de frames et les octets par message s'extrapolent mieux que les
latences, la RSS et le coût des termes fréquents.

## Matrice de décision

| décision | statut | résultat |
|---|---|---|
| archive append-only | **FIGER** | autorité séparée et reprise de queue validées |
| segmentation | **PROBABLE** | utile et indépendante des index |
| taille des segments | **OUVERT** | 64 MiB raisonnable, backup réel non mesuré |
| catalogue SQLite | **PROBABLE** | positions/attributs simples ; très grande échelle ouverte |
| Tantivy | **PROBABLE** | avantage de latence net à 100k/1M |
| FTS5 | **PROBABLE** | baseline simple, mais p95 fréquent élevé ici |
| index chaud | **OUVERT** | gain visible, fallback et fusion non mesurés |
| compression | **OUVERT** | gzip/zstd peu convaincants dans les variantes testées |
| déduplication des pièces jointes | **PROBABLE** | gain potentiel fort, CAS réel non mesuré |
| checksum | **FIGER** | nécessaire à la reprise et à l'intégrité |
| format des frames | **OUVERT** | header length/checksum utile ; versionnage/codec ouverts |

## Une seule prochaine expérience

Construire un CAS minimal sur disque pour 100 000 messages mixtes (blobs
hashés, manifeste, lecture et reconstruction), puis mesurer import, espace,
fsync et sauvegarde incrémentale contre la variante inline. C'est la prochaine
expérience qui réduit le plus l'incertitude sans figer le format des frames.

## Références externes

- [SQLite FTS5](https://www.sqlite.org/fts5.html)
- [Tantivy 0.26.1](https://docs.rs/tantivy/0.26.1/tantivy/)
- [rusqlite bundled](https://github.com/rusqlite/rusqlite)
