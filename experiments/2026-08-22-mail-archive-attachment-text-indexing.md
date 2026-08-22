# Memoria — indexation bornée du texte des pièces jointes

Date : 2026-08-22

## Périmètre

Cette passe ajoute une extraction dérivée et reconstructible pour la recherche.
Le RAW MIME, le framing de l’archive et le catalogue SQLite restent inchangés.
Le texte extrait n’est pas conservé comme copie permanente de la pièce jointe.

Le chemin testé est :

```text
RAW MIME → mailparse → extraction bornée → Tantivy.attachment_text
```

Une erreur ou une absence de provider ne retire pas le document : le message,
ses métadonnées et son corps restent indexés normalement.

## Inventaire anonymisé du corpus réel

Archive locale Gmail : 3 013 messages, 25 pièces jointes téléchargeables.
Les valeurs ci-dessous ne contiennent ni identifiant, nom, adresse ni contenu.

| MIME | count | decoded bytes | messages concerned |
|---|---:|---:|---:|
| `application/octet-stream` | 2 | 11 203 283 | 2 |
| `application/pdf` | 6 | 15 108 294 | 5 |
| `application/vnd.android.package-archive` | 1 | 4 360 322 | 1 |
| `application/vnd.openxmlformats-officedocument.wordprocessingml.document` | 1 | 11 166 962 | 1 |
| `image/png` | 1 | 10 750 | 1 |
| `text/html` | 11 | 126 839 | 5 |
| `text/xml` | 3 | 1 350 | 2 |

Le corpus réel ne justifie pas l’ajout d’un moteur Office généraliste dans
Memoria : il ne contient qu’un DOCX, tandis que PDF et `text/*` offrent une
valeur immédiate avec une frontière plus petite. Aucun ZIP/ODF n’a été observé
dans cet inventaire.

## Stratégies examinées

- `text/*` : décodage UTF-8 avec remplacement explicite des séquences
  invalides. Les pièces `text/html` sont aplaties avec la fonction texte
  dérivée déjà utilisée pour le corps ; aucun HTML actif n’est exécuté.
- PDF : appel direct, sans shell, à `pdftotext` Poppler 26.07.0 lorsque le
  provider est présent. L’absence de l’exécutable donne `Unsupported`.
- Office/OpenDocument : outils système présents localement, notamment
  LibreOffice, mais non retenus. Leur coût et leur surface documentaire sont
  disproportionnés pour le corpus observé.
- Aucun parseur PDF Rust, Apache Tika, LibreOffice headless ou moteur universel
  n’est embarqué.

## Frontière et bornes

Le module interne distingue `Text`, `Unsupported` et `Failed`. Les limites
actuelles sont :

- entrée maximale par pièce : 64 MiB ;
- sortie maximale indexée par pièce : 8 MiB ;
- timeout du provider PDF : 10 secondes ;
- stdout et stderr ne sont jamais journalisés comme contenu utilisateur ;
- le provider externe est lancé avec arguments séparés et ses erreurs restent
  locales à l’extraction.

La taille maximale observée d’une pièce jointe réelle est d’environ 11,2 MiB,
ce qui laisse une marge utile sans faire croire qu’une pièce arbitrairement
volumineuse est sans coût.

## Évolution Tantivy

Le schéma dérivé possède maintenant un champ textuel séparé
`attachment_text`. La recherche libre interroge les champs message et ce champ
avec un boost inférieur (`0.7`) pour éviter qu’un long PDF ne domine
artificiellement un match concis du sujet ou du corps. Les filtres structurés
restent inchangés et sont combinés par AND.

Un index ne possédant pas ce champ est considéré incompatible ; le mécanisme
existant supprime uniquement l’index dérivé et le reconstruit depuis RAW et
catalogue. Aucun RAW ni donnée SQLite n’est migré ou réécrit.

## Mesures de reconstruction réelle

Commande :

```text
cargo run -q -p mail-archive-experiment --bin mail-archive-experiment -- \
  gmail-index --archive .local/gmail-real-20260820
```

Résultat agrégé du rebuild :

```text
examined=3013
indexed=3013
parse_failures=0
attachment_encountered=25
attachment_supported=20
attachment_extracted=19
attachment_unsupported=5
attachment_extraction_failures=0
attachment_decoded_bytes=41977800
attachment_extracted_bytes=97637
attachment_extracted_chars=96537
index_bytes=11514581
wall_ms=7701
```

Un des 20 formats supportés n’a produit aucun texte utile ; il ne s’agit pas
d’un échec de pipeline. L’index précédent mesurait 11 148 600 octets dans le
rapport Tantivy réel ; le nouvel index mesure 11 514 581 octets, soit environ
366 KiB supplémentaires. Les durées historiques (15,5 s contre 7,7 s ici) ne
sont pas comparables scientifiquement : versions de corpus, état des caches et
conditions de build diffèrent.

Le binaire ne gagne aucune dépendance Cargo directe ou transitive :
`pdftotext` est un provider système optionnel hors graphe Cargo. Le binaire
Linux profile `ci` mesuré après changement fait 36 636 032 octets, contre
36 609 152 octets avant cette passe. Cette comparaison reste indicative car
les builds ne sont pas des builds binaires reproductibles.

## Tests

Fixtures couvertes :

- texte uniquement dans une pièce jointe `text/plain` retrouvé par recherche ;
- phrase uniquement dans un PDF retrouvé par recherche quand `pdftotext` est
  disponible ;
- provider PDF absent, PDF invalide et sortie bornée sans panic ;
- pièce jointe non supportée et pièce vide ;
- échec d’extraction sans perte de l’indexation du corps ;
- plusieurs pièces et filtres structurés conservés ;
- évolution de schéma Tantivy déclenchant une reconstruction dérivée.

Les tests ne modifient jamais le RAW de la fixture. Le test PDF est
conditionnel à la présence du provider système afin que l’absence de Poppler
reste une configuration valide.

## Décisions et limites

**Fait vérifié.** La recherche libre retrouve un message grâce à du texte
présent uniquement dans une pièce jointe PDF sur cette machine, sans modifier
le RAW.

**Décision de projet.** Retenir pour l’instant `text/*` et PDF via `pdftotext`;
laisser Office/OpenDocument non supporté plutôt que d’embarquer un moteur lourd.

**Limite.** Le support PDF dépend de Poppler installé sur le poste. Les
documents Office, APK, images et blobs binaires ne sont pas indexés. Il n’y a
ni OCR, ni extraction de tableaux structurée, ni recherche sémantique.

**Limite.** Les statistiques proviennent d’un seul compte réel et ne sont pas
une distribution générale des pièces jointes Gmail.

La prochaine incertitude utile est la valeur réelle de l’indexation DOCX/ODF
sur plusieurs corpus, pas une optimisation immédiate du chemin PDF.
