# Mail archive — CAS et contrat de conservation

## Objectif

Cette expérience mesure le coût réel d'une externalisation des pièces jointes.
Un CAS ajouté à côté d'une copie MIME intacte ne ferait que doubler les
octets ; les variantes ci-dessous remplacent donc le payload dans le magasin
de messages par une référence reconstructible. L'archive append-only actuelle
reste inchangée.

Commandes reproductibles :

```text
cargo build -p mail-archive-experiment --release
target/release/mail-archive-experiment cas-benchmark --profile personal --messages 10000 --seed 42 --out /tmp/mail-archive-cas-personal-10000
target/release/mail-archive-experiment cas-benchmark --profile heavy --messages 1000 --seed 42 --out /tmp/mail-archive-cas-heavy-1000
cargo test -p mail-archive-experiment
```

Les variantes écrivent un magasin de messages, des blobs immuables segmentés
à 64 MiB et un manifeste TSV. Le hash est Blake3. Les tests n'utilisent ni
serveur ni service externe.

## Niveaux de fidélité

- **Byte-exacte :** les octets EML exportés sont identiques. Il faut conserver
  les headers, les séparateurs, les encodages de transfert et l'ordre exact,
  ainsi que les références et les bytes du payload. La variante `cas-exact`
  vise ce contrat et vérifie `original == reconstruct(...)`.
- **MIME :** les parties, headers, types, encodages et contenus sont
  préservés, mais des repliements ou représentations d'encodage peuvent
  changer. Ce niveau autorise une normalisation contrôlée mais ne doit pas
  être appelé byte-exact.
- **Fonctionnelle :** un client reçoit les mêmes informations, corps et
  pièces jointes, sans identité MIME stricte. Ce niveau est insuffisant pour
  une migration conservatrice sans copie byte-exacte séparée.

Le générateur actuel produit déjà des payloads synthétiques décodés. Dans ce
corpus, `cas-exact` et `cas-decoded` ont donc le même hash et le même coût.
Cela ne prouve pas qu'un MIME base64 et un MIME quoted-printable équivalents
seraient dédupliqués : un décodeur MIME réel et une politique de canonisation
restent nécessaires pour mesurer ce cas.

## Résultats `personal`, 10 000 messages

Contrôle inline : 578,392,463 octets physiques. La RSS maximale du processus
complet était 483,712 KiB ; le temps wall incluant les cinq variantes était
8,33 s.

| variante | physique | économie vs inline | blobs | objets externalisés | hash | import |
|---|---:|---:|---:|---:|---:|---:|
| inline | 578,392,463 | 0 | 0 | 0 | 0,09 s | 1,37 s |
| cas-exact | 507,701,683 | 70,690,780 (12,2 %) | 2 057 | 3 035 | 0,50 s | 1,76 s |
| cas-decoded | 507,701,683 | 70,690,780 (12,2 %) | 2 057 | 3 035 | 0,51 s | 1,73 s |
| hybride 64 KiB | 523,369,277 | 55,023,186 (9,5 %) | 903 | 1 051 | 0,50 s | 1,72 s |
| hybride 256 KiB | 537,559,551 | 40,832,912 (7,1 %) | 358 | 397 | 0,51 s | 1,74 s |

Le CAS exact récupère presque toute l'économie théorique du profil : les
71,098,368 octets de doublons théoriques deviennent 70,690,780 après
références et manifeste. L'hybride 64 KiB conserve environ 78 % du gain exact
avec moins de blobs ; 256 KiB conserve environ 58 %.

## Résultats `heavy`, 1 000 messages

Ce corpus représente 956,070,420 octets inline. Il est suffisamment lourd
pour mesurer le coût des accès et du hash, mais les taux de duplication sont
plus variables qu'à 5 000 messages.

| variante | physique | économie | blobs | objets externalisés | hash | import |
|---|---:|---:|---:|---:|---:|---:|
| inline | 956,070,420 | 0 | 0 | 0 | 0,08 s | 2,23 s |
| cas-exact | 947,085,026 | 8,985,394 (0,94 %) | 504 | 614 | 0,92 s | 3,04 s |
| cas-decoded | 947,085,026 | 8,985,394 (0,94 %) | 504 | 614 | 0,95 s | 3,05 s |
| hybride 64 KiB | 947,502,173 | 8,568,247 (0,90 %) | 496 | 523 | 0,92 s | 3,01 s |
| hybride 256 KiB | 947,502,173 | 8,568,247 (0,90 %) | 496 | 523 | 0,91 s | 2,99 s |

Une tentative heavy 5 000 avec les cinq variantes a rempli le volume temporaire
après environ 12 Go de variantes partielles. Ce n'est pas une économie ou une
latence valide ; c'est un résultat opérationnel : une campagne multi-variante
doit être exécutée séquentiellement ou sur un volume dimensionné pour les
copies intermédiaires.

## Performance et accès

Le hash Blake3 représente environ 0,5 ms pour 555 Mo de payload personal et
0,9 s pour 951 Mo heavy. Le coût CPU du hash n'est pas dominant ici ; les
écritures et la génération dominent l'import.

La reconstruction d'un échantillon est de l'ordre de la microseconde car les
blobs sont déjà dans le cache du processus. La lecture aléatoire d'un premier
enregistrement est également de quelques microsecondes sur le SSD/cache
chaud. Ces chiffres ne constituent pas une mesure cold-disk : ils ne vident
pas les caches et ne mesurent pas encore une sélection aléatoire de milliers
de pièces jointes.

Le coût de complexité est réel : référence par message, manifeste, segments
de blobs, scrubber/GC et chemin d'export supplémentaire. Un blob non référencé
peut rester présent ; le modèle n'a pas de compteur de références
transactionnel et compte sur un GC ultérieur.

## Reconstruction et cas MIME difficiles

Les tests vérifient la reconstruction byte-exacte de toutes les variantes sur
un corpus de 50 messages et d'un fixture MIME contenant multipart imbriqué,
headers repliés, nom encodé, Content-Transfer-Encoding base64 et séparateurs.
Le fixture n'est pas externalisé par le parseur synthétique, mais son absence
de transformation est vérifiée.

Les cas suivants sont donc conservés comme invariants de test futurs :
quoted-printable, base64 avec longueurs de ligne variables, inline image avec
Content-ID, attachment sans nom et plusieurs attachments identiques sous des
noms différents. Le prototype actuel ne décode pas encore ces MIME réels ; il
ne doit pas annoncer une fidélité MIME décodée supérieure à ce qu'il teste.

## Crash, orphelins et sauvegarde

Le CAS écrit les blobs avant le manifeste final et ne réécrit pas un blob dont
le hash existe déjà. Cette organisation rend acceptable un blob orphelin
après interruption ; la reconstruction d'un message précédemment validé ne
dépend pas d'un compteur de références. Un test couvre aussi la présence d'un
blob supplémentaire non référencé et la reconstruction reste valide.

La sauvegarde incrémentale n'est pas encore mesurée par une séquence de deux
lots dans le même magasin : le benchmark actuel reconstruit chaque variante
depuis zéro. Il établit seulement la direction attendue : inline ajoute des
frames de messages, tandis que CAS ajoute les nouveaux blobs et références,
sans recopier les blobs déjà présents. Cette question reste ouverte et ne doit
pas être présentée comme un résultat mesuré.

## Réponses aux questions

1. **CAS exact :** 12,2 % d'économie physique sur personal 10k ; 0,94 % sur
   heavy 1k. Le taux dépend fortement de la stabilité de la distribution.
2. **CAS décodé :** aucun gain supplémentaire observé, car le corpus actuel
   contient déjà des payloads décodés et sans variantes d'encodage.
3. **Prix performance :** environ +0,4 s sur personal et +0,8 s sur heavy,
   avec un hash Blake3 non dominant ; accès/export ajoutent un chemin.
4. **Prix complexité :** manifeste, références, segmentation de blobs,
   scrubber/GC et invariants d'export.
5. **Hybride :** 64 KiB conserve l'essentiel du gain personal ; l'effet est
   presque nul sur heavy 1k car les doublons sont majoritairement au-dessus
   des deux seuils.
6. **Byte-for-byte :** il vaut son coût pour une archive de migration, mais
   il faut conserver le chemin exact et tester les vrais MIME ; le CAS ne doit
   pas remplacer l'original sans ce contrat.
7. **Sauvegardes :** non concluantes, la séquence incrémentale reste à mesurer.
8. **8–13 % :** intéressant pour personal, mais le gain réel dépend de la
   queue et du coût des métadonnées ; il est faible dans l'échantillon heavy.
9. **Principe CAS :** probable comme stockage facultatif/artefact, pas encore
   figé comme unique représentation d'autorité.
10. **Contrat recommandé :** conserver une représentation byte-exacte
    exportable ; autoriser un CAS reconstructible en dessous, sans promettre
    une déduplication décodée tant que le parseur MIME n'est pas validé.

## Matrice de décision

| sujet | statut | décision actuelle |
|---|---|---|
| conservation byte-exacte | **FIGER** | contrat recommandé pour migration/restauration |
| externalisation des attachments | **PROBABLE** | utile si la représentation est transformée et reconstructible |
| CAS sur bytes MIME | **PROBABLE** | gain réel sur personal, simple à vérifier |
| CAS sur contenu décodé | **OUVERT** | aucun bénéfice mesuré sans vrais encodages MIME |
| hybride avec seuil | **PROBABLE** | 64 KiB bon compromis preliminaire pour personal |
| hash Blake3 | **PROBABLE** | coût faible et hash moderne ; validation de politique encore ouverte |
| blobs orphelins | **PROBABLE** | tolérer puis détecter, éviter les compteurs transactionnels |
| garbage collection | **OUVERT** | scrubber nécessaire, algorithme non mesuré |
| compression individuelle blobs | **OUVERT** | ne pas mélanger avec cette décision |

## Prochaine expérience

Mesurer une séquence réellement incrémentale en deux lots `personal` : même
magasin inline, CAS exact et hybride 64 KiB, avec export d'un échantillon et
volume de fichiers modifiés/ajoutés après le second lot. Cela réduira
l'incertitude sauvegarde/maintenance qui reste la plus importante avant de
figer le CAS.
