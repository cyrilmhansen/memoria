# Mail archive — profils de corpus et duplication

## Objectif

Le premier générateur réutilisait un petit pool implicite de payloads : une
expérience produisait 3 040 objets pour seulement 88 hashes. Cette note remplace
cette hypothèse par trois profils déterministes et paramétrables. Elle ne
prétend mesurer aucune population réelle d'e-mails.

Le prototype expose `--profile light|personal|heavy`, `--duplicate-rate P`,
`--attachment-rate P`, `--max-attachment-bytes N` et `--compression`. Le seed
contrôle toutes les décisions. La compression est désactivée par défaut pour
les corpus volumineux afin de ne pas confondre génération et mesure de codec.

## Profils choisis

- `light` : taux d'attachements 3 %, petits logos et quelques documents,
  contenu principalement texte ; plafond d'attachement 64 KiB.
- `personal` : 30 %, avec documents partagés, documents renommés, logos,
  photos/blobs, ZIP et contenu compressible ; plafond 1 MiB.
- `heavy` : 65 %, tailles d'attachements et corps beaucoup plus larges ;
  plafond 8 MiB.

Dans les trois profils, la taille des corps suit une loi asymétrique : la
majorité est petite, une fraction est moyenne, une fraction décroissante est
grande et une queue rare est très grande. Les pièces jointes distinguent
logos partagés, documents partagés, documents transférés/renommés, contenus
uniques, images, blobs déjà compressés et contenus répétitifs compressibles.
`duplicate-rate` agit sur le choix des documents partagés ; il ne contrôle pas
les logos, qui sont un cas structurel séparé.

## Résultats de taille

Seed 42, compression désactivée. Les résultats light et personal utilisent
10 000 messages ; heavy utilise 5 000 messages afin de produire plusieurs Go
sans remplir inutilement le disque.

| profil | messages | moyenne | p50 | p90 | p99 | max | texte MIME | pièces jointes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| light | 10 000 | 1 077 | 631 | 1 128 | 7 328 | 207 926 | 9,10 Mo (84,4 %) | 1,68 Mo (15,6 %) |
| personal | 10 000 | 57 807 | 697 | 131 594 | 1 049 323 | 1 159 564 | 22,56 Mo (3,9 %) | 555,52 Mo (96,1 %) |
| heavy | 5 000 | 1 001 115 | 262 690 | 1 055 431 | 8 392 326 | 8 558 408 | 28,25 Mo (0,6 %) | 4 977,33 Mo (99,4 %) |

Le p50 reste petit dans personal et heavy malgré une moyenne élevée : cette
propriété est volontaire et représente la queue asymétrique du profil, pas une
mesure du monde réel. Le corpus heavy atteint 5,006 Go d'archive physique.

## Pièces jointes et CAS théorique

| profil | objets | hashes uniques | octets totaux | octets uniques | économie maximale | objets dupliqués |
|---|---:|---:|---:|---:|---:|---:|
| light | 313 | 286 | 1,678,824 | 1,542,656 | 135,168 (8,1 %) | 27 (8,6 %) |
| personal | 3 035 | 2 057 | 555,515,904 | 484,417,536 | 71,098,368 (12,8 %) | 978 (32,2 %) |
| heavy | 3 264 | 2 419 | 4,977,334,272 | 4,542,210,048 | 435,124,224 (8,7 %) | 845 (25,9 %) |

La taille des objets dupliqués est également importante : p50/p90/max
valent 5 KiB/5 KiB/5 KiB pour light, 32 KiB/128 KiB/1 MiB pour personal et
5 KiB/1 MiB/8 MiB pour heavy. Le nombre d'objets et les octets économisables
ne racontent donc pas la même histoire. Par exemple, les petits logos sont
nombreux mais économiquement secondaires devant les documents lourds.

Ces valeurs sont des économies théoriques : elles ne déduisent ni manifeste,
ni index de blobs, ni fsync, ni coût d'accès. Elles ne justifient pas un CAS
universel, mais rendent l'expérience CAS utile pour personal/heavy.

## Compression ciblée

La compression a été activée seulement sur 1 000 messages pour limiter le
coût CPU. Les mesures séparent la partie texte MIME et la partie attachment
du flux synthétique ; elles incluent les limites de frame/headers dans la
partie texte.

| profil | texte brut | gzip texte | zstd texte | attachments bruts | gzip attachments | zstd attachments |
|---|---:|---:|---:|---:|---:|---:|
| light | 818,9 KiB | 382,1 KiB | 386,1 KiB | 232,5 KiB | 251,5 KiB | 240,4 KiB |
| personal | 1,87 MiB | 376,6 KiB | 377,4 KiB | 52,45 MiB | 46,16 MiB | 46,24 MiB |

Sur light, les petits blobs pseudo-aléatoires gonflent légèrement sous
compression. Sur personal, la masse d'attachements bénéficie d'environ 12 %
de gain, mais ce résultat mélange images/blob incompressibles et contenu
répétitif. Le profil heavy n'a pas été compressé : sa campagne ciblée serait
coûteuse et ne changerait pas la question principale avant la mesure CAS.

## Échelle en Go

Le corpus heavy de 5 000 messages produit 5,006 Go, 3 264 pièces jointes et
14,96 s d'import sur la machine de mesure. C'est désormais une échelle en
octets utile pour les tests de stockage, tout en restant assez petite pour
être régénérée. Une tentative antérieure de 100 000 personal avec un plafond
plus élevé a généré plusieurs Go avant manifeste et a été interrompue : elle
a révélé un défaut de garde de la distribution, pas un résultat scientifique.
Les plafonds actuels rendent ce risque explicite et configurable.

## Projection vers 300 Go

Conversion arithmétique avec les moyennes observées, sans prétendre modéliser
des boîtes réelles :

- light : environ 278 millions de messages ;
- personal : environ 5,2 millions de messages ;
- heavy : environ 0,30 million de messages.

Cette différence confirme que « 300 Go » est une contrainte plus informative
que « plusieurs millions de messages ». Les coûts de catalogue, index,
merges et recherche ne s'extrapolent pas à partir de ces seuls ratios.

## Conclusions

- **Fait vérifié :** le taux de duplication peut maintenant être modifié et
  observé séparément en objets et en octets ; il n'est plus imposé par un
  pool minuscule caché.
- **Fait vérifié :** les profils couvrent des régimes de volume très
  différents et le profil heavy atteint plusieurs Go de façon reproductible.
- **Hypothèse :** personal et heavy sont les seuls profils où un CAS devrait
  probablement apporter un gain spatial assez visible ; cette hypothèse reste
  à vérifier avec les coûts d'indexation et d'accès.
- **Décision de projet :** ne pas relancer FTS5/Tantivy sur ces profils et ne
  pas modifier le format de l'archive avant l'expérience CAS.

## Prochaine expérience

La prochaine expérience reste un CAS minimal, mais limité aux profils
`personal` et `heavy`, avec une variante inline comme contrôle. Le profil
`light` ne doit pas piloter cette décision : son économie théorique est faible.

## Références

Le générateur est synthétique ; aucune source publique de distribution réelle
n'est utilisée pour calibrer les profils. Les références moteur restent dans
[`2026-08-20-mail-archive-scale.md`](2026-08-20-mail-archive-scale.md).
