# Campagne Gmail réelle — validation 100 → 1 000

Date : 2026-08-20  
Statut : campagne limitée validée ; aucune synchronisation complète exécutée.

Le compte et les identifiants source ne sont pas mentionnés dans ce rapport.
Aucun sujet, header, nom de fichier, corps, adresse, token ou secret n’a été
enregistré.

## Séquence exécutée

La même archive locale a été utilisée pour toute la campagne.

1. import borné à 100 messages ;
2. relance identique ;
3. réparation des métadonnées après découverte du mapping JSON camelCase ;
4. passage incrémental sans changement ;
5. extension bornée à 1 000 messages ;
6. relance `--max-messages 1000` pour la validation finale ;
7. analyse MIME hors ligne des 1 000 RAW.

La dernière relance Gmail a produit :

```text
full_sync=true
examined=1000
new_messages=0
network_bytes=76802908
archive_bytes_added=0
duration_ms=66315
```

`full_sync=true` est attendu ici : l’import reste borné et le catalogue garde
`complete=false`. Il s’agit d’une réconciliation bornée, pas d’une preuve que
le compte entier a été énuméré.

Le checkpoint incrémental précédent a produit :

```text
full_sync=false
examined=0
new_messages=0
network_bytes=0
archive_bytes_added=0
duration_ms=148
```

## Intégrité et catalogue

État final vérifié hors ligne :

| propriété | résultat |
|---|---:|
| messages catalogués | 1 000 |
| Gmail IDs distincts | 1 000 |
| doc IDs distincts | 1 000 |
| états `present` | 1 000 |
| frames vérifiées par checksum | 1 000 |
| fichiers de segment | 1 |
| taille physique archive | 57 633 178 octets |
| taille brute totale déduite | 57 601 178 octets |

La limite de segment est 64 MiB, soit 67 108 864 octets. L’unique fichier de
57,6 MiB contient donc 1 000 frames de 32 octets d’en-tête suivies de leur
payload ; il ne s’agit pas d’une frame unique. La somme des `frame_bytes` du
catalogue correspond à la taille physique du segment.

La seconde synchronisation finale n’a ajouté aucune frame et aucun nouveau
message. Les compteurs structurants du catalogue sont restés à 1 000. Les
timestamps de suivi peuvent naturellement être rafraîchis lors d’une full
sync bornée ; ils ne constituent pas une modification des données faisant
autorité.

## Taille RAW

Percentiles calculés hors ligne sur les 1 000 frames, sans afficher les bytes :

| métrique | octets |
|---|---:|
| p50 | 54 803 |
| p90 | 101 889 |
| p99 | 178 363 |
| maximum | 267 410 |

Mesures d’import :

| étape | examinés | nouveaux | bytes réseau | bytes archive ajoutés | durée |
|---|---:|---:|---:|---:|---:|
| 100 initial | 100 | 100 | 6 549 816 | 4 915 464 physiques | 10 228 ms |
| 100 → 1 000 | 1 000 | 900 | 76 802 908 | 52 717 714 | 70 222 ms |
| relance 1 000 | 1 000 | 0 | 76 802 908 | 0 | 66 315 ms |

Le premier affichage de `archive_bytes_added` avait un bug de comptage et
doublait les payloads. Le code a été corrigé pour compter `frame_bytes`; la
taille physique et les checksums sont la référence pour cette campagne.

## Analyse MIME hors ligne

Le rapport local `gmail-report` a relu les RAW déjà archivés et vérifié chaque
checksum. Il n’a effectué aucun accès Gmail et n’affiche aucune valeur de
header.

| agrégat | nombre | octets décodés |
|---|---:|---:|
| messages `multipart/*` | 938 | — |
| parties MIME totales | 2 907 | — |
| feuilles MIME | 1 969 | — |
| Content-Disposition `attachment` | 0 | 0 |
| Content-Disposition `inline` | 1 969 | 46 811 806 |
| partie avec `filename` ou `name` | 0 | 0 |
| partie avec `Content-ID` | 0 | 0 |
| `image/*` | 0 | 0 |
| `application/pdf` | 0 | 0 |
| `application/zip` | 0 | 0 |
| Office/OpenDocument | 0 | 0 |
| autres `application/*` | 0 | 0 |

Les catégories de type sont comptées sur les feuilles ; les parties
multipart conteneurs ne sont pas additionnées comme payloads.

### Pourquoi `attachments=0` ?

Ce n’est pas un bug de parcours multipart : `mailparse 0.16.1` fournit une
itération en profondeur et l’analyse hors ligne a trouvé 2 907 parties et
1 969 feuilles. Dans cet échantillon, aucune feuille ne porte `attachment`,
`filename`, `name` ou `Content-ID`; toutes les feuilles sont exposées par le
parseur avec la disposition inline par défaut. Les 0 pièces jointes sont donc
une observation du corpus réel testé, pas un compteur synthétique masqué par
un défaut de récursion.

Cette observation ne permet pas de conclure que le compte complet ne contient
aucune pièce jointe : seuls les 1 000 messages de la campagne sont mesurés.

## CAS théorique

Les agrégats MIME ne contiennent aucun payload identifié comme pièce jointe :

```text
CAS exact théorique       = 0 octet économisable
CAS contenu décodé        = 0 octet économisable
CAS hybride seuil 64 KiB  = 0 octet économisable
```

Cela ne teste pas l’intérêt du CAS sur ce compte ; cela signifie seulement que
la tranche 1 000 ne fournit aucun objet candidat selon les invariants MIME
retenus.

## Anomalies rencontrées et corrigées

- Le callback OAuth lisait jusqu’à la fermeture de la connexion ; il pouvait
  rester bloqué avec un navigateur HTTP keep-alive. Lecture bornée des
  en-têtes et timeout de 300 secondes ajoutés.
- Les champs Gmail camelCase n’étaient pas décodés vers les structures Rust.
  `historyId`, `threadId`, `labelIds` et `internalDate` sont maintenant
  correctement conservés.
- Le point de départ initial d’historique utilisait le dernier message au lieu
  de `users.getProfile.historyId`.
- Une campagne bornée pouvait être marquée complète après un passage
  incrémental. La progression `complete=false` est désormais conservée.
- Le compteur `archive_bytes_added` doublait les payloads ; il compte désormais
  directement les bytes de frame.

Les corrections ne réécrivent pas les RAW et ne changent pas le format des
segments.

## Conclusion

**Faits vérifiés :** le téléchargement RAW, la conservation byte-exacte, les
metadata Gmail séparées, l’idempotence et l’ouverture de l’archive résistent à
1 000 messages sur cette campagne. Le chemin incrémental sans changement est
vide. Les checksums des 1 000 frames sont valides.

**Limite :** cette campagne n’apporte aucune observation sur les pièces
jointes, la déduplication ou le CAS, car aucun candidat MIME n’a été trouvé.
Elle ne doit donc pas remplacer un corpus de test contenant volontairement
des pièces jointes.

**Décision :** les résultats sont suffisamment propres pour lancer une
synchronisation complète contrôlée du compte, en conservant `RAW-inline` et
en surveillant les statistiques agrégées. La synchronisation complète devra
être traitée comme une nouvelle étape et ne devra pas promouvoir de décision
CAS à partir de cette tranche sans pièces jointes.
