# Réglage du pipeline Tantivy à 1M

## Périmètre et configuration réelle

Cette expérience reprend exactement le corpus structuré déterministe du
rapport 1M : seed `20260821`, 1 000 000 messages, segments RAW de 64 MiB,
build Linux x86-64 release. Les RAW, SQLite, documents Tantivy et requêtes
ne changent pas entre variantes.

Le chemin produit appelait `Index::writer(50_000_000)` avec Tantivy 0.26.1.
D'après le code de Tantivy :

- le budget global est de 50 000 000 octets ;
- au plus 8 workers sont tentés ;
- avec un minimum de 15 000 000 octets par worker, cette machine choisit
  3 workers (`50 000 000 / 3`) ;
- `IndexWriterOptions` utilise 4 threads de merge par défaut ;
- la politique est `LogMergePolicy` par défaut.

Le probe expose maintenant ces paramètres explicitement, sans figer le chemin
produit : `GmailIndexWriterConfig::default()` conserve la sélection dynamique
de `Index::writer`. Les variantes avec worker explicite utilisent 3 workers,
afin de ne faire varier qu'une variable à la fois.

Tantivy ne fournit pas d'estimation publique du `SegmentWriter` agrégée au
niveau de `IndexWriter`. `SegmentWriter::mem_usage` existe dans son pipeline
interne, mais n'est pas accessible au probe sans modifier la crate. La mesure
utilisée est donc `VmRSS`, complétée par les bornes de phase et les segments.

## Méthode

Chaque variante a été exécutée sur un répertoire neuf avec :

```text
cargo build --release -p mail-archive-experiment --bin structured-search-benchmark
target/release/structured-search-benchmark \
  --messages 1000000 --seed 20260821 --out /var/tmp/<unique> [options]
```

Les options sont `--writer-budget`, `--writer-workers`, `--merge-threads`
et `--no-merge`. Les artefacts volumineux ont été supprimés à la fin de la
campagne. RSS est échantillonnée toutes les 25 ms ; les points synchrones
sont pris à l'ouverture du writer, aux bornes de 100k documents, avant/après
commit et après retour de l'indexeur.

## Résultats principaux

Les latences indiquées sont p95 en microsecondes pour : texte fréquent / date
sélective / texte+attachment / texte+MIME PDF / aucun résultat.

| variante | budget | workers | mergeurs | peak RSS | indexation | segments avant → après commit → final | index | p95 contrôles |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| baseline | 50 MB | 3 | 4 | 799 MiB | 18,6 s | 0 → 9 → 9 | 136,3 MB | 596 / 2 579 / 9 316 / 5 564 / 18 |
| writer 64 MiB | 64 MB | 3 | 4 | 1 061 MiB | 18,1 s | 0 → 9 → 9 | 138,6 MB | 480 / 2 601 / 9 979 / 5 394 / 18 |
| writer minimum valide | 45 MB | 3 | 4 | 841 MiB | 23,2 s | 0 → 16 → 9 | 136,3 MB | 748 / 4 018 / 21 691 / 5 434 / 23 |
| 1 worker | 50 MB | 1 | 4 | 1 350 MiB | 17,9 s | 0 → 9 → 9 | 170,3 MB | 724 / 2 841 / 7 223 / 5 083 / 18 |
| 1 merger | 50 MB | 3 | 1 | 776 MiB | 17,9 s | 0 → 10 → 10 | 136,5 MB | 467 / 2 471 / 9 463 / 5 339 / 20 |
| `NoMergePolicy` | 50 MB | 3 | 4 | 1 297 MiB | 27,1 s | 0 → 157 → 157 | 156,2 MB | 1 060 / 4 093 / 10 051 / 5 820 / 291 |

Les temps et p95 varient avec le cache et le filesystem ; les écarts de
quelques pourcents ne sont pas interprétés comme des gains. Les résultats
retournent les mêmes cardinalités : 50 résultats pour les workloads avec
résultats, zéro pour la requête absente.

## Chronologie RSS

Valeurs en MiB. Chaque séquence contient : ouverture du writer, puis les dix
bornes 100k, puis avant commit, après commit et après retour de l'indexeur.

| variante | chronologie représentative |
|---|---|
| baseline | 13, 155, 240, 368, 488, 600, 600, 602, 604, 671, 796, 796, 781, 734 |
| writer 64 MiB | 13, 166, 330, 401, 394, 427, 555, 676, 785, 916, 1 044, 1 044, 987, 979 |
| writer 45 MB | 13, 155, 244, 374, 489, 560, 560, 560, 598, 703, 829, 829, 825, 765 |
| 1 worker | 13, 174, 292, 430, 555, 681, 802, 924, 1 044, 1 164, 1 287, 1 287, 1 345, 1 346 |
| 1 merger | 13, 155, 250, 372, 494, 594, 594, 595, 597, 665, 786, 786, 785, 724 |
| `NoMergePolicy` | 13, 190, 317, 441, 562, 685, 805, 929, 1 051, 1 171, 1 293, 1 293, 1 293, 1 224 |

Dans la baseline, 64 MiB et 45 MB, le maximum survient avant commit. La
réduction à un seul merger reste dans le même régime. `NoMergePolicy` confirme
que laisser 157 segments n'est pas une solution mémoire : le RSS et la
latence augmentent, et la recherche sans résultat devient sensiblement plus
lente.

Le cas 1 worker atteint son maximum pendant/après commit, ce qui indique que
la phase de flush/commit et les structures d'un gros segment peuvent être
plus coûteuses que plusieurs petits workers. Les mesures ne montrent pas de
pic attribuable uniquement aux threads de merge ; elles montrent surtout le
coût des segments et de leur finalisation. L'instrumentation actuelle ne
chronomètre pas chaque tâche de merge individuellement.

## Décision

**Fait vérifié :** le RSS d'environ 0,8 GiB n'est pas principalement le budget
de 50 MB pris au pied de la lettre. Avec la collecte catalogue et l'état
dérivé déjà bornés, ce budget est un régime efficace sur ce corpus.

**Fait vérifié :** augmenter à 64 MB ou passer à 1 worker dégrade le RSS sans
gain d'indexation utile. Réduire au minimum valide économise peu et dégrade
le débit ainsi que plusieurs latences. Un seul merger est quasiment neutre,
mais ne procure pas une réduction substantielle reproductible.

**Fait vérifié :** `NoMergePolicy` est uniquement un diagnostic et doit rester
hors produit : index plus fragmenté, RSS plus élevé, index plus gros et
recherches moins bonnes.

**Décision de projet :** conserver la configuration produit actuelle et
dynamique (`Index::writer(50_000_000)`, politique de merge par défaut). Ne pas
modifier le moteur, le budget, le nombre de workers ou les mergeurs sur la
base de cette campagne.

La prochaine investigation, si elle devient nécessaire, serait une mesure
plus fine des allocations/fusions internes Tantivy. Elle ne justifie pas une
nouvelle optimisation applicative dans cette passe.
