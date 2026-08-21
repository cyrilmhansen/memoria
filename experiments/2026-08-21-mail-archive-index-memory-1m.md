# Mémoire de reconstruction Tantivy — 100k à 1M

## Périmètre

Cette expérience mesure uniquement la mémoire du probe de recherche structurée
Gmail synthétique. Elle ne modifie ni les frames RAW, ni le schéma SQLite de
l'archive, ni le schéma de recherche. Le seed est `20260821`, les segments font
64 MiB et le corpus est celui du rapport
[`structured-search-1m`](2026-08-21-mail-archive-structured-search-1m.md).

Les gros corpus ont été créés séquentiellement sous `/var/tmp` puis supprimés
par `find <répertoire-de-la-campagne> -depth -delete`. Aucun corpus ne reste
dans `/tmp` ou `/var/tmp`.

Commandes reproductibles :

```text
cargo build --release -p mail-archive-experiment --bin structured-search-benchmark
target/release/structured-search-benchmark \
  --messages 1000000 --seed 20260821 --out /var/tmp/<campagne>
```

RSS = `VmRSS` de `/proc/self/status`. Un échantillonneur séparé relève le pic
global ; des points synchrones sont enregistrés au changement de phase. Les
valeurs peuvent donc différer de quelques MiB selon le cache et le moment de
l'échantillonnage.

## Attribution avant correction

La version initiale collectait toutes les lignes du catalogue dans une
`Vec<GmailCatalogRow>`, puis construisait un `HashSet` de tous les `doc_id`.
Elle conservait aussi toutes les lignes `state_upserts` jusqu'au commit de
l'index. Le writer Tantivy et ses documents restaient actifs pendant la
construction.

Chronologie représentative, version initiale :

| phase | 100k RSS | 1M RSS |
|---|---:|---:|
| démarrage | 4 MiB | 4 MiB |
| catalogue ouvert | 6 MiB | 6 MiB |
| archive générée | 9 MiB | 9 MiB |
| catalogue entièrement collecté | 38 MiB | 259 MiB |
| writer ouvert | 41 MiB | 280 MiB |
| pendant l'indexation / avant commit | 192 MiB | 1 224 MiB |
| après commit Tantivy | 173 MiB | 1 219 MiB |
| après retour de l'indexeur | 146 MiB | 985 MiB |
| pic échantillonné | 200 MiB | 1 228 MiB |

À 1M, la matérialisation du catalogue est donc visible mais n'explique pas
seule le pic : la mémoire augmente encore d'environ 965 MiB entre la collecte
des lignes et la fin de l'indexation. Cette phase correspond aux documents,
structures de segments et allocations du pipeline Tantivy, ainsi qu'aux
copies de `state_upserts`.

## Corrections mesurées

### Itération du catalogue

`gmail_catalog_rows` a été remplacé par `for_each_gmail_catalog_row`. SQLite
produit une ligne, le code lit/parses le RAW et l'envoie au writer, puis la
ligne est libérée. Le `HashSet` des IDs n'est construit que lorsqu'un index
existant doit être comparé pour détecter des documents disparus ; il est
inutile lors d'une reconstruction initiale avec un état Tantivy vide.

### État dérivé borné

Les `state_upserts` et `state_deletes` ne sont plus conservés en vecteurs. Les
modifications de `tantivy-state.sqlite` sont exécutées dans une transaction
SQLite unique pendant l'itération, puis validées après le commit Tantivy. En
cas d'erreur avant validation, la transaction SQLite est abandonnée ; les
RAW restent indépendants et l'index dérivé reste reconstructible.

Ces deux changements ne modifient pas le format d'autorité. Ils ne changent
pas le schéma Tantivy ni la limite de résultats.

## Comparaison d'échelle

| messages | RSS avant | RSS après | réduction | index après | indexation après |
|---:|---:|---:|---:|---:|---:|
| 100k | 200 548 KiB | 160 844 KiB | 19,8 % | ~15 MiB | 2,64 s |
| 300k | 496 112 KiB | 379 092 KiB | 23,6 % | ~40 MiB | 5,28 s |
| 500k | 891 688 KiB | 667 992 KiB | 25,1 % | ~65 MiB | 8,72 s |
| 1M | 1 255 920 KiB | 816 236 KiB | 35,0 % | ~130 MiB | 26,76 s |

Les valeurs 300k et 500k après correction ont été mesurées avec le même
générateur et le même seed ; les valeurs avant proviennent de la baseline
instrumentée. Les temps d'indexation varient avec le cache et le filesystem,
et ne constituent pas une amélioration garantie. Le coût d'indexation 1M
reste du même ordre qu'avant.

La dernière exécution 1M a indexé 1 000 000/1 000 000 messages, conservé un
index d'environ 136 MiB et retourné 50 résultats pour les familles sélectives
et fréquentes attendues (zéro résultat pour la requête absente). Les p50/p95
observés restent dans les mêmes ordres : environ 0,5–4,4 ms pour les filtres
seuls, 5,3–11,6 ms pour les combinaisons texte+filtre. Une mesure isolée de
la requête date présentait un p95 à 6,5 ms ; elle est considérée comme du
bruit de système, pas comme une régression d'architecture.

## Ce qui vit encore en mémoire

- **Produit/archive :** les segments RAW et le catalogue SQLite ne sont pas
  chargés entièrement ; l'accès reste localisé par frame.
- **Produit/indexation :** le writer Tantivy, les documents en cours et les
  structures de fusion restent la source principale après les corrections.
  Le pic passe d'environ 1,23 GiB à environ 0,80 GiB à 1M et croît encore
  avec le nombre de documents.
- **État de reconstruction :** l'itération SQLite et la transaction bornent
  maintenant les lignes et les mises à jour d'état ; le `known`/`current`
  complet n'est nécessaire que pour une réconciliation d'un index déjà rempli.
- **Générateur :** les buffers d'un message synthétique sont temporaires et
  le générateur ne contribue pas au pic d'indexation une fois l'archive écrite.
- **Recherche :** les requêtes sont exécutées après l'indexation ; leur RSS
  augmente légèrement pendant l'ouverture/mmap et les workloads, mais elles
  ne sont pas la cause du pic de reconstruction.

La chronologie ne justifie pas encore de changer le moteur, le format, ou de
réduire arbitrairement le budget du writer. Elle justifie en revanche de
considérer le reliquat comme une limite Tantivy/allocateur/fusion à mesurer
séparément avant toute projection vers plusieurs millions.

## Conclusion

**Fait vérifié :** la matérialisation complète du catalogue et le vecteur des
états modifiés étaient deux coûts mémoire inutiles. Leur remplacement par une
itération SQLite et une transaction d'état bornée conserve les tests, les
résultats structurés et la reconstruction tout en réduisant le pic 1M
d'environ 35 %.

**Fait vérifié :** après cette réduction, Tantivy devient le principal poste
observable du pipeline d'indexation ; le RSS corrigé reste proche de 0,8 GiB
à 1M. Cela ne permet pas encore d'extrapoler linéairement à plusieurs millions
sans isoler les buffers/fusions Tantivy.

**Décision de projet :** conserver les deux corrections bornées, ne pas
modifier le RAW/catalogue, ne pas changer de moteur et ne pas introduire de
spool disque à cette étape.

**Prochaine incertitude minimale :** mesurer le coût du writer Tantivy seul
(budget mémoire, nombre de segments et fusion) sur le même corpus, avec une
seule variation contrôlée à la fois. Cette expérience doit rester séparée de
la dette Gmail qui conserve les IDs énumérés avant traitement.
