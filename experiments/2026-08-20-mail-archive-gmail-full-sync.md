# Campagne Gmail réelle complète contrôlée

Date : 2026-08-20  
Statut : full sync terminée et relance incrémentale validée.

Ce rapport ne contient que des compteurs, tailles, durées, types MIME et
hashes agrégés ; aucune donnée personnelle n’y est enregistrée.

## Synchronisation

L’archive contenait 1 000 messages et `complete=false`. La commande complète a
été exécutée sans `--max-messages` et sans `--query`. Les 1 000 messages
existants ont été reconnus par leur identité source.

Résultat :

```text
full_sync=true
examined=3012
new_messages=2012
network_bytes=282328812
archive_bytes_added=154206800
duration_ms=235873
```

Le nombre déjà connu est donc 1 000. Aucun changement de label, suppression ou
erreur API/MIME/archive n’a été compté.

## État final et stockage

| mesure | résultat |
|---|---:|
| messages finaux | 3 012 |
| Gmail IDs distincts | 3 012 |
| états `present` / `deleted` | 3 012 / 0 |
| archive RAW physique | 211 839 978 octets |
| RAW hors en-têtes de frames | 211 743 594 octets |
| catalogue SQLite | 1 687 552 octets |
| segments | 4 |
| checksums valides | 3 012 / 3 012 |
| débit messages examinés | 12,77 messages/s |
| débit nouveaux messages | 8,53 messages/s |
| débit réseau | 1,20 MiB/s |
| débit archive ajoutée | 0,62 MiB/s |

Les segments mesurent 67 085 372, 67 099 315, 59 036 932 et 18 618 359
octets, tous sous 64 MiB. La somme des `frame_bytes` du catalogue correspond
à la taille physique totale. Chaque localisation a été relue avec vérification
du checksum et de l’identifiant de frame ; aucune frame n’est inaccessible.

La moyenne RAW est de 70 300 octets. Les percentiles sont :

| p50 | p90 | p99 | maximum |
|---:|---:|---:|---:|
| 47 899 | 87 665 | 145 799 | 16 865 580 |

## Relance incrémentale sans changement

Après l’enregistrement `complete=true`, la commande normale sans limite ni
requête a produit :

```text
full_sync=false
examined=0
new_messages=0
network_bytes=0
archive_bytes_added=0
duration_ms=78
```

Le chemin `history.list` est donc utilisable après la full sync ; aucune frame
ou ligne de contenu n’est ajoutée en l’absence de changement.

## MIME complet

Analyse locale des 3 012 RAW, sans nouvel accès Gmail :

| catégorie | nombre | octets décodés |
|---|---:|---:|
| messages multipart | 2 771 | — |
| parties MIME | 8 672 | — |
| feuilles | 5 860 | — |
| `Content-Disposition: attachment` | 25 | 41 977 800 |
| `attachment_candidate` | 102 | 42 340 230 |
| inline | 5 835 | 124 943 515 |
| filename/name | 102 | 42 340 230 |
| Content-ID | 96 | 24 380 807 |
| image/* | 78 | 373 180 |
| PDF | 6 | 15 108 294 |
| ZIP | 0 | 0 |
| Office/OpenDocument | 1 | 11 166 962 |
| autres application/* | 3 | 15 563 605 |

Les candidats représentent environ 20,0 % des octets RAW hors frames ; les
attachments stricts environ 19,8 %.

## Duplication et CAS théorique

Sur les 102 candidats :

| métrique | encodé MIME | contenu décodé |
|---|---:|---:|
| objets totaux | 102 | 102 |
| hashes uniques | 35 | 34 |
| octets totaux | 57 935 218 | 42 340 230 |
| octets uniques | 57 108 032 | 41 706 668 |
| octets dupliqués | 827 186 | 633 562 |
| duplication objets | 65,7 % | 66,7 % |
| duplication octets | 1,43 % | 1,50 % |

La duplication par nombre d’objets est élevée parce que plusieurs parties
référencent le même contenu. Elle ne doit pas être confondue avec la
duplication par octets : le calcul correct additionne une seule taille par
hash distinct.

```text
CAS exact                 = 827 186 octets (1,43 % des candidats encodés)
CAS contenu décodé        = 633 562 octets (1,50 % des candidats décodés)
CAS hybride seuil 64 KiB  = 410 646 octets (0,72 % des candidats >64 KiB)
```

Le sous-ensemble hybride contient 10 objets >64 KiB, 9 hashes uniques,
56 863 030 octets uniques et 410 646 octets dupliqués. Le gain est donc réel mais faible, contrairement à
la conclusion précédente qui provenait du bug d’agrégation.

Le test mathématique `A, A, B, C, C` vérifie explicitement : 5 objets, 3
hashes, 10 octets totaux, 6 octets uniques, 4 octets dupliqués et 40 % de
duplication selon les octets comme selon les objets.

## Comparaison aux profils synthétiques

| profil | p50 | moyenne | p99 | part attachments |
|---|---:|---:|---:|---:|
| light | 631 | 1 077 | 7 328 | 15,6 % |
| personal | 697 | 57 807 | 1 049 323 | 96,1 % |
| heavy | 262 690 | 1 001 115 | 8 392 326 | 99,4 % |
| compte réel | 47 899 | 70 300 | 145 799 | 20,0 % candidats |

La moyenne `personal` est proche de la moyenne réelle, mais son p50 et sa
proportion d’attachments ne décrivent pas ce compte. `light` représente mieux
la proportion d’octets non-attachments, mais pas la taille médiane ni la queue.
`heavy` surestime fortement les octets d’attachments. Aucun recalibrage du
générateur n’est effectué à partir d’un seul compte.

## Anomalies et conclusion

Les corrections OAuth, JSON camelCase, `getProfile.historyId`, progression des
campagnes bornées et compteur `archive_bytes_added` avaient été validées avant
la full sync. L’audit ultérieur a corrigé uniquement l’agrégation des tailles
uniques. Aucune frame RAW n’a été réécrite.

## Réconciliation optimisée

Gmail décrit `historyId` comme l’identifiant du dernier événement ayant
modifié un message et expose `labelIds`, `historyId`, `internalDate` et
`threadId` dans la ressource `Message`. La méthode `users.messages.get` accepte
`format=METADATA`, avec éventuellement `metadataHeaders[]`, tandis que `RAW`
est le format qui transporte le message RFC complet. Voir la
[référence officielle de `messages.get`](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get)
et la [ressource Message](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages).

La réconciliation utilise maintenant `METADATA` pour un Gmail ID déjà connu et
`RAW` uniquement pour un ID nouveau. Une fixture de 1 000 connus + 10 inconnus
vérifie 1 000 appels metadata, 10 appels RAW, l’ajout des 10 frames et la mise
à jour des labels connus.

La full sync historique a transféré 282 328 812 octets, dont environ
83 352 724 octets de RAW base64 correspondant aux 1 000 messages déjà connus
dans les campagnes bornées. Après optimisation, ce composant RAW est évité :
une réconciliation de 3 012 messages connus transfère 0 byte RAW et ne conserve
que les requêtes de liste, d’historique, de profil et les petites réponses
METADATA, dont la taille exacte n’est pas encore instrumentée.

Le stockage actuel ne montre pas de faiblesse bloquante avant l’interface :
RAW, catalogue, segmentation, checksums et incrémental sont cohérents. La
prochaine incertitude importante est la tenue des catalogues et index dérivés
avec plusieurs comptes et plusieurs millions de messages, pas le contrat RAW
de ce compte.

La baseline réelle est établie. La synchronisation complète et la maintenance
incrémentale sans changement sont validées. Aucun CAS réel, changement de format
ou décision d’architecture supplémentaire n’est justifié par cette campagne.
