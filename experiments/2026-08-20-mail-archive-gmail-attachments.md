# Expérience Gmail réelle — messages avec pièces jointes

Date : 2026-08-20  
Statut : échantillons read-only validés ; aucune synchronisation complète
exécutée et aucun CAS créé.

Les archives expérimentales sont séparées de l’archive principale. Aucun
contenu de message, header, adresse, sujet, nom de fichier, token ou secret
n’est présent dans ce rapport.

## Échantillons Gmail

| requête | messages Gmail examinés | nouveaux | bytes réseau | durée |
|---|---:|---:|---:|---:|
| `has:attachment` | 37 | 37 | 78 015 772 | 31 082 ms |
| `has:attachment larger:1M` | 7 | 7 | 75 470 708 | 14 397 ms |

La limite était 100 dans les deux cas ; Gmail n’a retourné que 37 puis 7
résultats dans les vues testées.

## Échantillon `has:attachment`

Analyse locale des 37 RAW :

| agrégat | nombre | octets décodés |
|---|---:|---:|
| messages multipart | 37 | — |
| parties MIME totales | 250 | — |
| feuilles | 174 | — |
| `Content-Disposition: attachment` | 25 | 41 977 800 |
| `Content-Disposition: inline` | 149 | 746 677 |
| `filename` ou `name` | 102 | 42 340 230 |
| `Content-ID` | 82 | 24 159 059 |
| `image/*` | 78 | 373 180 |
| `application/pdf` | 6 | 15 108 294 |
| ZIP | 0 | 0 |
| Office/OpenDocument | 1 | 11 166 962 |
| autres `application/*` | 3 | 15 563 605 |

Les 102 parties portant `filename` ou `name` correspondent exactement aux
102 candidats comptés par le chemin d’import. Les 25 `attachment` strictes
sont un sous-ensemble ; plusieurs parties nommées sont inline et/ou portent
un Content-ID.

| définition | objets | octets encodés | objets uniques encodés | octets décodés | objets uniques décodés |
|---|---:|---:|---:|---:|---:|
| attachment strict | 25 | 57 439 280 | 22 | 41 977 800 | 21 |
| candidat `attachment ∪ filename/name` | 102 | 57 935 218 | 35 | 42 340 230 | 34 |

Les octets uniques sont égaux aux octets totaux dans les deux cas : aucune
économie de déduplication n’est observée dans cet échantillon. Tous les
candidats encodés au-dessus de 64 KiB représentent 57 273 676 octets ; le gain
théorique de la variante hybride 64 KiB est donc également nul.

## Échantillon `has:attachment larger:1M`

Analyse locale des 7 RAW :

| agrégat | nombre | octets décodés |
|---|---:|---:|
| messages multipart | 7 | — |
| parties MIME totales | 36 | — |
| feuilles | 22 | — |
| `Content-Disposition: attachment` | 8 | 41 238 689 |
| `Content-Disposition: inline` | 14 | 116 239 |
| `filename` ou `name` | 8 | 41 238 689 |
| `Content-ID` | 3 | 23 485 793 |
| `image/*` | 0 | 0 |
| `application/pdf` | 4 | 14 508 122 |
| ZIP | 0 | 0 |
| Office/OpenDocument | 1 | 11 166 962 |
| autres `application/*` | 3 | 15 563 605 |

Les 8 candidats sont distincts : 56 452 384 octets encodés et 41 238 689
octets décodés, avec économie CAS exacte, décodée et hybride 64 KiB égale à
zéro. Les 56 452 384 octets encodés dépassent le seuil 64 KiB.

## Pourquoi Gmail et MIME ne comptent pas exactement la même chose

`has:attachment` est une recherche Gmail, pas une promesse que chaque message
retourné possède une feuille avec exactement `Content-Disposition: attachment`.
Dans les RAW observés, Gmail retourne aussi des parties nommées inline,
notamment des ressources associées à un Content-ID. Le prototype distingue :

- `attachment` strict : disposition MIME `attachment` ;
- `attachment_candidate` : disposition `attachment` ou paramètre `filename`/
  `name`, définition utilisée par l’import et les statistiques CAS ;
- `inline` et `Content-ID` comme catégories séparées.

Il n’y a donc pas d’anomalie inexpliquée : les 37 résultats Gmail contiennent
25 attachments strictes et 102 candidats nommés. L’écart est expliqué par la
classification, sans inspection de contenu personnel.

## Fidélité et structure physique

Les deux archives ont été analysées hors ligne par `gmail-report` : chaque
localisation catalogue a été lue via `read_record`, son identifiant comparé au
catalogue et son checksum vérifié.

- archive `has:attachment` : 37 frames, 1 segment, 58 512 963 octets ;
- archive `larger:1M` : 7 frames, 1 segment, 56 603 246 octets.

Les RAW archivés restent byte-exacts par rapport aux bytes décodés lors de
l’import ; le parsing n’écrit aucune donnée et ne modifie aucune frame. Le
CAS n’est pas activé.

## Conclusion

**Faits vérifiés :** la classification MIME explique l’écart avec Gmail, les
payloads lourds sont bien observés, aucun doublon de contenu n’apparaît dans
ces deux petits échantillons et les checksums sont valides.

**Limite :** l’absence de duplication ici ne permet pas d’inférer le taux du
compte complet. Elle invalide seulement toute conclusion synthétique selon
laquelle ce compte offrirait déjà un gain CAS mesurable.

**Recommandation :** les résultats sont suffisamment propres pour lancer la
synchronisation complète du compte en RAW-inline. Ne pas créer de CAS réel et
ne pas modifier le format d’archive ; conserver les statistiques MIME/CAS
comme analyse dérivée reconstructible.
