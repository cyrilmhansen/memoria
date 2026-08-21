# Campagne 1M — recherche structurée Gmail

## Objectif et périmètre

Cette campagne mesure exclusivement `SearchRequest` sur un corpus Gmail
synthétique de 1 000 000 de messages. Elle n'a pas modifié le format RAW, le
catalogue SQLite ni le contrat de l'indexeur. Le seul changement de code
nécessité par une anomalie mesurée est l'ajout de champs dérivés
`sender_filter` et `recipient_filter`, afin que les adresses complètes
contenant `-`, `@` et `.` soient filtrables sans post-filtrage.

Le probe reproductible est
[`structured-search-benchmark.rs`](../projects/mail-archive/src/bin/structured-search-benchmark.rs).
Les données ont été écrites sous `/var/tmp/atlas-structured-search-*` et
supprimées automatiquement par `trap` après chaque exécution. Aucun corpus
volumineux ne reste dans `/tmp` ou `/var/tmp`.

Commandes principales :

```text
cargo build --release -p mail-archive-experiment --bin structured-search-benchmark
target/release/structured-search-benchmark \
  --messages 1000000 --seed 20260821 --out /var/tmp/atlas-structured-search-<unique>
```

Linux x86-64, Tantivy 0.26.1, build release, segments de 64 MiB. Les
latences sont 100 exécutions par famille, après une requête de warm-up ; le
résultat retourné est limité à 50 documents. Les p50/p95/p99 sont des temps
de `search_request` complets, conversion des documents comprise.

## Distributions synthétiques

Ces paramètres sont conçus pour tester des sélectivités, pas pour prétendre
mesurer la population mondiale des boîtes mail.

- **Messages :** un corps texte court avec les termes fréquents `project`,
  `archive`, `meeting`, et un terme rare déterministe par ID.
- **Correspondants :** distribution décroissante : contact 0000 = 22 %,
  contact 0001 = 12 %, contact 0002 = 9 %, puis 997 correspondants dans la
  queue restante (~57 %). Les destinataires sont synthétiques et constants.
- **Dates :** 45 % dans les 0–2 dernières années, 30 % dans les 2–5 ans,
  15 % dans les 5–8 ans et 10 % dans les 8–11 ans, avec un offset uniforme
  dans chaque tranche. La borne testée 2024-01-01 → 2026-01-01 couvre
  450 549 messages (45,05 %).
- **Labels :** INBOX 55 %, ARCHIVE 45 %, WORK 30 %, SENT 18 %, STARRED 8 %,
  IMPORTANT environ 7,7 %. Les labels sont multi-valués et leurs
  corrélations sont déterministes.
- **Pièces jointes :** 30 % des messages, un objet par message dans ce probe.
  Parmi tous les messages : image/jpeg 10,50 %, PDF 7,50 %, Office/OpenXML
  4,50 %, ZIP 3,00 %, CSV 4,50 %. Les tailles sont asymétriques : la grande
  majorité fait quelques KiB et une minorité 32–256 KiB selon le MIME.

À 1M, les comptes générés sont : 300 000 messages avec pièce jointe,
104 999 images, 75 001 PDF, 29 998 ZIP, 45 000 Office, 80 000 STARRED,
300 000 WORK et 220 000 messages du correspondant dominant.

## Coût du corpus et de l'index

| mesure | 100k, même probe | 1M, même probe | facteur approximatif |
|---|---:|---:|---:|
| génération/archive | 2 082 ms | 17 193 ms | 8,3× |
| archive physique | 737 053 182 B | 7 384 599 096 B | 10,0× |
| indexation Tantivy | 3 587 ms | 17 073 ms | 4,8× |
| index dérivé | 14 057 107 B | 136 607 247 B | 9,7× |
| RSS de pointe | 200 548 KiB | 1 255 920 KiB | 6,3× |

Les temps d'indexation ne sont pas une loi linéaire fiable : cache, débit du
filesystem et fusion des segments interviennent. La taille de l'index est
proche de la croissance linéaire sur cet intervalle.

Le RSS de 1 255 920 KiB est la mesure la plus importante. Il comprend le
processus complet et notamment la collecte des lignes du catalogue avant
l'indexation. C'est un signal de risque pour plusieurs millions de messages,
pas une preuve que l'archive ou Tantivy ne peuvent pas fonctionner à cette
échelle.

## Latences structurées

Toutes les valeurs sont en microsecondes, sous la forme p50 / p95 / p99.

| workload | sélectivité synthétique | 100k | 1M |
|---|---:|---:|---:|
| texte fréquent | ~100 % | 327 / 344 / 353 | 477 / 486 / 492 |
| texte rare | 10 puis 100 résultats | 63 / 69 / 70 | 197 / 201 / 207 |
| date sélective | 45,05 % | 654 / 678 / 684 | 2 588 / 2 611 / 2 654 |
| avec pièce jointe | 30 % | 509 / 528 / 532 | 1 583 / 1 606 / 1 629 |
| sans pièce jointe | 70 % | 751 / 770 / 790 | 3 361 / 3 382 / 3 393 |
| MIME image/* | 10,50 % | 373 / 393 / 405 | 706 / 713 / 743 |
| MIME PDF exact | 7,50 % | 355 / 380 / 399 | 570 / 576 / 600 |
| label STARRED | 8 % | 357 / 364 / 375 | 595 / 618 / 682 |
| label WORK | 30 % | 507 / 516 / 523 | 1 581 / 1 609 / 1 639 |
| expéditeur dominant | 22 % | 452 / 462 / 472 | 1 223 / 1 237 / 1 272 |
| fragment expéditeur | large | 830 / 844 / 849 | 3 866 / 3 895 / 3 994 |
| texte + date | 45,05 % | 2 079 / 2 103 / 2 113 | 11 927 / 12 004 / 12 136 |
| texte + attachment | 30 % | 1 689 / 1 703 / 1 712 | 9 403 / 9 469 / 9 596 |
| texte + MIME PDF | 7,50 % | 1 071 / 1 087 / 1 091 | 5 318 / 5 536 / 6 425 |
| texte + label STARRED | 8 % | 1 091 / 1 105 / 1 113 | 5 369 / 5 439 / 6 125 |
| aucun résultat | 0 % | 13 / 13 / 14 | 20 / 20 / 23 |

Les requêtes combinées restent sous 12,2 ms au p99 dans ce test. Les
requêtes sans texte qui trient une population large (`sans pièce jointe`)
sont plus coûteuses que les MIME exacts. Le champ adresse complète corrigé
retourne bien les 50 premiers résultats attendus ; la population dominante
est utilisée comme contrôle de sélectivité.

## Comparaison avec le benchmark 100k historique

Le benchmark historique documentait 100k à environ 5,6 GB d'archive,
36,4 MB d'index et un p50 lexical Tantivy de 1,47 ms, mais il utilisait un
autre corpus/profil, une autre campagne et un workload générique FTS5/Tantivy.
Il n'est donc pas comparable numériquement au nouveau probe pour les labels
et les MIME.

La comparaison contrôlée ci-dessus, avec le même générateur et les mêmes
requêtes, est la référence utile : la taille de l'index est quasi linéaire,
la mémoire augmente beaucoup plus vite que le nombre de documents et les
latences restent interactives mais se dégradent de ~1,4× à ~5,7× selon la
sélectivité et la combinaison.

## Conclusions

- **Fait vérifié :** Tantivy traite 1M documents structurés avec des filtres
  exacts/familles, labels multi-valués, dates, correspondants et texte
  combiné, sans post-filtrage après la limite.
- **Fait vérifié :** les latences observées restent de l'ordre de la
  milliseconde au p95 pour les filtres simples ; les combinaisons texte +
  filtre atteignent environ 9,5–12,0 ms au p95 à 1M.
- **Fait vérifié :** l'index croît presque linéairement entre 100k et 1M.
- **Risque mesuré :** le RSS de pointe d'environ 1,2 GiB à 1M est trop
  important pour être ignoré dans une projection à plusieurs millions.
- **Décision de cette passe :** ne pas modifier encore l'architecture
  Tantivy/SQLite ; les performances de recherche ne le justifient pas. Le
  coût mémoire devient la prochaine expérience prioritaire.

## Ce qui reste ouvert

Ce corpus ne mesure pas les effets de plusieurs pièces jointes par message,
de changements de labels incrémentaux, de segments Tantivy très nombreux,
de multi-compte ou de plusieurs centaines de millions de documents. Les
résultats ne permettent pas non plus d'extrapoler directement à 300 Go.

La prochaine expérience minimale devrait mesurer une indexation par lots ou
un catalogue itéré/spoolé à 1M, avec le même schéma et le même workload, afin
de déterminer si le RSS vient principalement de `gmail_catalog_rows` ou d'une
autre phase, sans modifier le format RAW.
