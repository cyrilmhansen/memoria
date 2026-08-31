# Memoria — M1, modèle persistant d'acquisition et de provenance

Version 0.3 — 31 août 2026

Ce document est la décision de conception pour M1. Il est fondé sur le
catalogue, l'archive et les connecteurs présents dans l'arbre de travail au
31 août 2026. Il ne constitue pas encore une migration ni une API Rust.

Les mots **fait**, **décision**, **hypothèse** et **question ouverte** gardent
leur sens explicite. Une ligne SQLite n'est pas autoritative simplement parce
qu'elle est durable.

## 1. Faits vérifiés

### 1.1 Archive et catalogue

`archive/` contient des frames validées. La validation vérifie le framing, les
coordonnées, le `doc_id` porté par la frame, la longueur et les contrôles
d'intégrité, dont BLAKE3. Le catalogue actuel `messages` conserve ces
coordonnées et cet identifiant de frame, mais `doc_id` n'est pas une identité
physique suffisante : plusieurs frames simultanément présentes peuvent porter
le même `doc_id`. La position physique et le digest sont donc liés à la frame
complète, et non à ce seul nombre.

`messages` contient aussi `message_id`, des champs de navigation et des valeurs
observées ou de remplissage. Il ne constitue pas une preuve de provenance
provider. `attachments` et Tantivy sont descriptifs ou dérivés.

### 1.2 Providers

L'identité Tier A Gmail est exactement
`source_account + gmail_message_id`. Celle d'IMAP est exactement
`source_account + mailbox + UIDVALIDITY + UID`. Ces clés restent dans les
tables possédées par leur provider :

```text
gmail_messages(source_account, gmail_message_id, ...)
imap_messages(source_account, mailbox, uid_validity, uid, ...)
```

Les attributs provider, `source_state` et les frontiers sont des dimensions
distinctes. `first_seen_unix` et `last_seen_unix` gardent leur sémantique
actuelle. Une synchronisation bornée ne prouve pas l'absence d'un message.

### 1.3 Acquisition et publication actuelles

Le catalogue v1 ne persiste pas l'opération d'acquisition, son module, sa
version, ses bornes temporelles ou son résultat. Les chemins actuels appendent
le RAW, passent une barrière durable, puis publient le catalogue sous
single-writer. L'index est publié séparément. Il n'existe donc aucune
acquisition historique à reconstituer pendant la migration.

## 2. Décision : le minimum M1

M1 est une évolution versionnée du catalogue, additive dans ses responsabilités
mais sans moteur de provenance générique. Il sépare :

1. la revendication physique d'un RAW ;
2. l'opération M1 qui l'a réellement produit ;
3. l'identité et les attributs propres à chaque provider ;
4. l'état mutable et le frontier ;
5. la projection de présentation.

Il n'y a ni `provenance_level` global, ni registre de providers, ni trait Rust
universel, ni blob JSON opaque.

### 2.1 `raw_records` : une identité physique indépendante

`raw_records` est la table Tier A des claims physiques. Son minimum est :

```sql
raw_records(
  raw_record_id INTEGER PRIMARY KEY,
  frame_doc_id INTEGER NOT NULL,
  segment TEXT NOT NULL,
  archive_offset INTEGER NOT NULL,
  frame_bytes INTEGER NOT NULL,
  raw_blake3 BLOB NOT NULL CHECK(length(raw_blake3) = 32),
  claim_kind TEXT NOT NULL CHECK(claim_kind IN ('sourced', 'source_less')),
  acquisition_id INTEGER NULL,
  CHECK(frame_bytes > 0),
  UNIQUE(segment, archive_offset, frame_bytes)
)
```

`raw_record_id` est stable, indépendant et opaque. `frame_doc_id` est le
`doc_id` porté par la frame et n'est pas sa clé. Segment, offset, longueur et
BLAKE3 restent liés à ce `raw_record_id`; l'unicité de coordonnées et toute
validation framing/checksum doivent respecter le modèle physique existant.
Une ligne signifie qu'une frame validée est revendiquée avec ces coordonnées;
elle ne prouve pas à elle seule une origine source.

`claim_kind='source_less'` est le contrat explicite pour un inventaire physique
ou un salvage sans occurrence. Une publication provider ordinaire doit être
`sourced`, posséder l'occurrence correspondante et satisfaire la transaction
décrite en §5. `acquisition_id` est nullable pour les RAW historiques ou
source-less sans opération observée. L'absence d'une ligne ne prouve jamais
l'absence d'une frame physique : seul un scan validant l'archive peut l'établir.

Une modification d'une claim physique (coordonnées, longueur, digest ou
association à une autre frame) est une opération Tier A bornée, avec lecture et
validation de la totalité de l'ancienne preuve. M1 n'autorise ni réécriture
implicite, ni adoption par `frame_doc_id`, proximité, MIME ou index.

Une claim physique n'est pas une « version » mutable d'une occurrence. Un
`raw_record_id` ne change donc jamais de coordonnées, de longueur ou de digest.

### 2.2 Acquisitions : seulement les opérations observées

```sql
acquisitions(
  acquisition_id INTEGER PRIMARY KEY,
  module_kind TEXT NOT NULL,
  module_version TEXT NOT NULL,
  source_instance TEXT NULL,
  started_unix INTEGER NOT NULL,
  finished_unix INTEGER NULL,
  outcome TEXT NOT NULL CHECK(outcome IN ('running', 'success', 'partial', 'failure', 'cancelled')),
  CHECK((outcome = 'running') = (finished_unix IS NULL))
)
```

Une acquisition M1 est créée au début d'une opération réellement exécutée
après M1. Tant que l'opération est ouverte, `outcome='running'` et
`finished_unix` est NULL. Le vocabulaire terminal est limité :

- `success` signifie que l'opération prévue est terminée, y compris un scan
  achevé qui n'a publié aucun record, et que tout état/frontier requis a été
  avancé durablement ;
- `failure` signifie une erreur terminale avant tout record Tier A commis ;
- `partial` signifie une erreur terminale après au moins un record Tier A
  commis, notamment l'échec de l'avancement requis de l'état/frontier ;
- `cancelled` signifie une annulation explicite, avec conservation des records
  déjà publiés et sans prétendre avoir achevé le scan.

Un crash ou une terminaison de processus non observée laisse `running`, même
si des records ont été publiés. Aucune déduction rétrospective ne la convertit
en échec ou succès. Une politique explicite de recovery ou de réconciliation
devra constater les preuves disponibles avant de clore cette acquisition ;
cette décision ne fait pas partie de l'append normal. Les transitions
terminales fixent `finished_unix`; les timestamps sont ceux de l'opération,
jamais des valeurs inventées.

La cardinalité retenue est **acquisition 1:N `raw_records`** :
`raw_records.acquisition_id` pointe vers l'opération qui a produit la frame.
Aucun cas concret ne demande qu'une même frame physique soit produite par
plusieurs acquisitions. Un re-fetch exact ou une récupération qui écrit de
nouveaux octets produit une nouvelle frame et un nouveau `raw_record_id`, même
si son contenu ou son `frame_doc_id` est identique. Une table many-to-many
serait donc prématurée. Une même acquisition peut produire plusieurs RAW et
plusieurs occurrences provider.

### 2.3 Occurrences provider et assertions typées

Les tables `gmail_messages` et `imap_messages` restent canoniques. Elles
référencent `raw_record_id NOT NULL` et un `acquisition_id` nullable (nullable
pour les occurrences migrées), sans colonne d'identité concurrente :

```text
gmail: source_account + gmail_message_id
imap:  source_account + mailbox + uid_validity + uid
```

Ces champs sont directement attestés sous le contrat de leur provider.
`thread_id`, labels, flags, dates, tailles et états sont des attributs
observés de ce contrat; `source_state` n'est ni une provenance ni un frontier.
Les occurrences ne reçoivent pas d'assertion générique répétant leur identité.

Le besoin conceptuel de qualification par assertion reste réel, mais M1
l'exprime dans des contrats typés : identité provider directement attestée,
attribut observé, et projection MIME/présentation observée ou dérivée et non
autoritative. L'inconnu est représenté par une absence ou un champ typé
nullable; il ne faut pas fabriquer une assertion `unknown` pour remplir une
ligne. Une future provenance déclarée ou intermédiaire recevra une structure
typée dédiée seulement lorsqu'un producteur concret en aura démontré le
contrat. M1 ne pré-conçoit pas de tables EML, MBOX ou MailStore.

### 2.4 `messages` : projection de présentation conservée

La première migration conserve les champs actuels de navigation et de
présentation de `messages` afin de limiter les régressions UI, recherche et
export. Elle remplace toute référence physique par `raw_record_id` et ne
conserve pas `segment`, `archive_offset`, `frame_bytes` ou `raw_blake3` comme
claim concurrent. `message_id` reste, s'il est conservé pour compatibilité,
un identifiant de présentation/catalogue : il n'est ni l'identité provider ni
l'identité physique. Aucune contrainte de `messages` ne peut concurrencer les
clés canoniques Gmail ou IMAP. Parsing MIME, threading, index et rendu restent
dérivés. Les tables descriptives attachées à une présentation, notamment
`attachments`, suivent également `raw_record_id` et ne deviennent pas des
claims physiques.

`messages.raw_record_id`, ainsi que toute valeur équivalente désignant le RAW
courant dans une table descriptive ou de présentation, est une projection
dénormalisée/dérivée du lien canonique de l'occurrence provider. Elle est
conservée pour la navigation, la migration et la compatibilité avec les
chemins de lecture, recherche et export existants ; elle n'est pas une seconde
autorité Tier A. Une occurrence Gmail ou IMAP et ses projections de RAW
courant doivent être énumérées par le contrat de publication. Ce contrat
exige que toute mise à jour de ces projections, notamment
`messages.raw_record_id` et les liens descriptifs requis, soit effectuée dans
la même transaction SQLite que le déplacement du lien canonique. Il ne peut
donc pas exister d'état commis où l'occurrence pointe vers le nouveau RAW
tandis qu'un chemin normal de catalogue pointe encore vers l'ancien.

Ces projections ne doivent jamais servir à écraser ou reconstruire l'autorité
provider. Si une incohérence est détectée, la lecture ou la publication échoue
fermée, ou une maintenance explicitement contractée reconstruit la projection
à partir de l'occurrence typée canonique. Cette maintenance ne déduit jamais
une provenance et ne relie jamais par `doc_id`, contenu, proximité ou
similarité.

Lors de la migration v1, les valeurs current-RAW sont initialisées depuis
l'occurrence provider canonique après validation complète de celle-ci ; une
valeur présente dans `messages` ne peut jamais être utilisée pour reconstruire
ou choisir cette occurrence.

Pour une occurrence migrée, `acquisition_id` est toujours `NULL`. Pour une
nouvelle occurrence provider, il identifie l'acquisition qui a effectué sa
première publication/attestation et ce lien est immuable. Une acquisition de
re-fetch ultérieure ne le remplace pas : elle est liée au nouveau
`raw_record_id`, et l'historique de remplacement ci-dessous porte cette
opération.

### 2.5 Remplacement RAW borné par R2.1

R2.1 a besoin d'une histoire de remplacement, pas d'une identité physique
mutable. M1 ajoute donc une relation étroite par provider, et non un moteur
générique de provenance ou de supersession. Conceptuellement, chaque provider
possède une table de forme identique, par exemple :

```sql
gmail_raw_replacements(
  replacement_id INTEGER PRIMARY KEY,
  source_account TEXT NOT NULL,
  gmail_message_id TEXT NOT NULL,
  old_raw_record_id INTEGER NOT NULL,
  new_raw_record_id INTEGER NOT NULL,
  acquisition_id INTEGER NOT NULL,
  operation TEXT NOT NULL CHECK(operation = 'exact_refetch'),
  replaced_unix INTEGER NOT NULL,
  CHECK(old_raw_record_id <> new_raw_record_id),
  UNIQUE(source_account, gmail_message_id, old_raw_record_id)
)
```

`imap_raw_replacements` porte de la même manière la clé canonique
`source_account + mailbox + UIDVALIDITY + UID`. Ces relations sont typées par
leur table provider et leur seule opération est le remplacement borné
`exact_refetch`; elles ne sont pas extensibles en assertions polymorphes. Les
deux `raw_record_id` existent : l'ancien reste sa claim physique historique,
le nouveau est une frame distincte, durable et validée, avec
`acquisition_id` égal à l'acquisition de re-fetch. Le lien de l'occurrence vers
le nouveau RAW est la vue courante; la relation conserve l'ancien claim et
l'attribution de la transition.

Dans une transaction single-writer, R2.1 :

1. revalide la clé provider et l'état physique attendu de l'ancien claim ;
2. fetch, vérifie l'identité et l'égalité au digest historique, puis append le
   nouveau RAW et franchit sa barrière de durabilité ;
3. ouvre la transaction SQLite single-writer et vérifie par compare-and-swap
   que l'occurrence pointe encore vers `old_raw_record_id` et que la relation
   n'a pas déjà été publiée ;
4. crée le nouveau `raw_records` validé, insère la relation typée old→new, et
   remplace atomiquement le `raw_record_id` de l'occurrence par
   `new_raw_record_id` ;
5. dans cette même transaction, met à jour `messages.raw_record_id` et chaque
   projection descriptive/current-RAW requise par le contrat de publication
   vers `new_raw_record_id`, puis commit la transaction.

Si le compare-and-swap échoue, aucun lien courant n'est déplacé : la nouvelle
frame durable reste `OrphanValidated`, exportable par R2.2a, et ne peut pas
être adoptée implicitement. Si la transaction réussit, l'occurrence canonique
reste unique, l'ancien RAW n'est pas détaché de son histoire, et le nouveau
RAW est sa cible courante. Une nouvelle tentative ne réutilise jamais un
identifiant physique. Cette relation ne permet ni remplacement arbitraire,
ni mise à jour de coordonnées/digest, ni suppression de l'ancien claim.
Ainsi, un `sourced` non courant n'est pas un claim détaché ambigu : il est
précisément l'ancien côté d'une relation de remplacement provider, ou bien la
publication n'est pas valide.

## 3. Ce que M1 exclut

Le schéma initial ne contient pas `provenance_assertions` générique avec
`assertion_kind`, `proof_class`, valeurs polymorphes, version et
`supersedes_assertion_id`. Les producteurs actuels ne fournissent pas le
contrat nécessaire pour la contraindre correctement; cette forme est trop
proche d'un moteur EAV de connaissance/provenance et dupliquerait les
identités provider canoniques. Elle rendrait aussi ambiguë la différence entre
absence, valeur observée et identité Tier A. Les modèles futurs doivent être
introduits par une structure typée, locale au contrat qui les justifie.

## 4. Migration v1 → M1

La migration est une nouvelle version explicite, fail-closed et transactionnelle
par catalogue. Elle :

1. scanne et valide les frames physiques (framing, coordonnées, longueur,
   `frame_doc_id`, checksum et BLAKE3) avant de les revendiquer ;
2. alloue un `raw_record_id` indépendant pour chaque frame revendiquée et
   préserve les coordonnées exactes, y compris lorsque des `frame_doc_id` se
   répètent ;
3. reconstruit les occurrences Gmail/IMAP avec leur clé canonique et leur
   `raw_record_id`, après vérification complète de la claim physique ;
4. conserve les champs de présentation `messages` sans leur laisser d'autorité
   physique ou provider ;
5. ne crée **aucune acquisition historique** et ne met aucun lien
   `acquisition_id` sur les occurrences ou RAW migrés. Elle ne fabrique ni
   timestamp, ni version, ni outcome, ni assertion `unknown`.

Un audit éventuel est un événement explicitement nommé migration (avec les
métadonnées de migration disponibles); il ne se présente jamais comme
l'acquisition d'origine. `first_seen_unix` et `last_seen_unix` sont copiés
seulement avec leur sémantique v1, sans les réinterpréter.

Toute frame validée sans claim v1 reste une frame **non réclamée de
l'inventaire physique** pendant la migration : elle n'est pas migrée en ligne
`raw_records` `source_less`, ne reçoit aucune occurrence et n'est pas adoptée.
Elle reste éligible au contrat d'export existant `R2.2a
OrphanValidated`. Une publication explicitement source-less pourra être
envisagée plus tard, mais elle exigera son propre contrat de publication et ne
fait pas partie de la migration v1. Une coordonnée ou un digest invérifiable,
une identité provider invalide ou une collision non résolue bloque la
migration; aucune heuristique ne répare la preuve.

La migration ne transforme pas `messages.message_id`, MIME `Message-ID`,
expéditeur, sujet ou date en preuve provider. Elle ne répare pas les claims
contradictoires et ne supprime aucun RAW. Un catalogue interrompu reste à
l'ancienne version ou est rejeté; il ne doit pas être présenté comme M1.

## 5. Publication et crash

Pour une acquisition provider M1, le publisher exécute :

```text
acquisition observée
  → append RAW → durable_barrier
  → une transaction SQLite :
       raw_records (claim physique)
       + occurrence Gmail ou IMAP
       + tous les liens Tier A requis par ce contrat
  → commit SQLite
  → source_state/frontier applicables, séparément
```

La transaction exige la concordance complète du jeton de barrière, des
coordonnées, du digest et de `raw_record_id`, ainsi que l'existence de
l'occurrence canonique. Elle ne peut pas laisser un `raw_records` `sourced`
commis sans cette occurrence. Le contrat `source_less` est le seul cas
explicitement différent.

Avant la barrière, rien n'est publiable. Après la barrière mais avant le commit
SQLite, le frame validé est `OrphanValidated`; il n'y a ni occurrence ni
adoption implicite. Un rollback ou crash pendant la transaction ne publie
aucun de ses claims. Après commit mais avant mise à jour de l'état ou du
frontier, les records sont durables et le frontier reste inchangé/retryable.
L'acquisition reste `running` tant que sa fin n'est pas observée. Si
l'avancement requis échoue après publication d'un ou plusieurs records, elle
se clôt par `partial` (et non `success`); une annulation observée se clôt par
`cancelled`. Une reprise est idempotente par identité provider; un re-fetch
qui append de nouveaux octets ne réutilise pas l'ancien `raw_record_id`.

Le single-writer, les barrières RAW/SQLite et les réglages Tier A restent ceux
du socle existant. R1 reste read-only. M1 n'introduit ni `recover --force`,
ni relink, ni adoption, ni reconstruction par similarité.

## 6. Assurance et tests avant implémentation

La clôture exige une matrice exécutée, notamment :

- migration avec doublons de `frame_doc_id`, claims physiques contradictoires,
  digest/coordonnées invalides, catalogue sans acquisition historique et
  interruption ;
- unicité des coordonnées, liaison complète frame/longueur/digest et
  distinction `raw_record_id`/`frame_doc_id` ;
- publication atomique avant/après barrière, transaction, crash et frontier,
  y compris l'exception source-less ;
- idempotence et clés canoniques Gmail/IMAP, séparation état/frontier, et
  absence de promotion MIME ;
- inventaire `OrphanValidated`, corruption, single-writer et régression
  lecture/recherche/export après reconstruction des dérivés.

## 7. Question produit restante

La seule question ouverte pour l'opérateur est la visibilité UI d'un RAW
source-less. Elle est explicitement non bloquante pour le modèle de
persistance : un RAW sans occurrence ne doit pas apparaître automatiquement
comme un message ordinaire sourcé. Une future politique de recovery ou une
vue distincte peut décider de l'afficher (ou de le réserver à
l'export/recovery).
