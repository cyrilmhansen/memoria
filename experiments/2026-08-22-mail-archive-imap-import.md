# Memoria — premier import IMAP readonly

Date : 2026-08-22

## Périmètre et décision

Cette passe ajoute un chemin de développement IMAPS readonly vers une archive
Memoria existante. L’acquisition IMAP reste distincte de Gmail ; aucune
abstraction `MailSource` ni UI de compte n’est introduite.

Le chemin validé est :

```text
IMAPS / EXAMINE / BODY.PEEK[]
        ↓
ArchiveWriter.append_raw()
        ↓
messages + imap_messages
        ↓
index_gmail_archive() comme pipeline dérivé existant
        ↓
Tantivy / recherche / lecture / export EML
```

Le RAW et le framing ne changent pas. Le RAW MIME est la donnée de référence.
Le parsing MIME, le catalogue de recherche et l'index Tantivy restent
reconstructibles. La table `imap_messages` conserve en revanche une
provenance observée côté serveur — source, mailbox, UIDVALIDITY, UID, flags,
INTERNALDATE, taille annoncée et état source — qui n'est pas reconstructible
à partir du RAW MIME seul.

## Interface de développement

Le binaire `imap-import` accepte explicitement :

```text
--archive PATH
--host HOST --port 993 --server-name NAME
--username USER --password PASSWORD
--ca-cert CA.pem
--mailbox INBOX
--source SOURCE_KEY
--limit N
--timeout-ms N
```

L’import accepte aussi plusieurs occurrences de `--mailbox`; `--all-mailboxes`
sélectionne toutes les entrées retournées par `LIST` qui ne portent pas
`\\Noselect`. Avant l’import, Memoria exécute `CAPABILITY` puis `LIST "" "*"`.
Le nom protocolaire, le séparateur, les attributs bruts, les attributs
SPECIAL-USE et le caractère sélectionnable sont conservés dans
`imap_mailboxes`. Les noms sont utilisés tels que fournis par async-imap,
sans supposer un séparateur `/`. Leur adéquation à un affichage Unicode n’est
pas déduite de cette API.

Chaque mailbox est ensuite importée indépendamment avec le même mécanisme
`EXAMINE` / `UIDVALIDITY` / `UIDNEXT` / `scanned_through_uid`. Une erreur sur
une mailbox est rapportée dans son résultat et ne modifie pas les frontières
déjà publiées pour les autres. La clé logique reste
`source_account + mailbox + UIDVALIDITY + UID`: deux occurrences du même RAW
dans deux mailboxes ne sont ni fusionnées ni dédupliquées.

Le mot de passe et la CA sont fournis hors archive et hors dépôt. Aucun secret
n’est écrit dans les logs. Le port TLS implicite est utilisé ; STARTTLS,
OAuth IMAP et la synchronisation Gmail via IMAP restent hors périmètre de
ce CLI de développement; la découverte de mailboxes est désormais supportée
et validée séparément avec GreenMail.

## Runtime et sécurité

`sync_imap` crée un runtime Tokio multi-thread local et attend sa tâche réseau
avant de revenir au code appelant. Cette frontière empêche Tokio d’entrer
dans le thread Slint ; une future UI pourra appeler cette primitive depuis son
worker existant.

Le client utilise `EXAMINE`, jamais `SELECT`, et demande
`UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[]`. Le certificat est vérifié
par rustls avec la CA fournie dans un `RootCertStore`. Aucun verifier
dangereux n’existe dans le chemin produit. Les connexions, greeting, login,
EXAMINE, FETCH et logout sont bornés par timeout.

## Catalogue et idempotence

L’identité observée pour cette passe est :

```text
source_account + mailbox + UIDVALIDITY + UID
```

`messages.message_id` reçoit une représentation technique stable de cette
clé, tandis que `imap_messages` conserve les colonnes séparées. Une seconde
importation avec la même UIDVALIDITY et les mêmes UID ne crée aucune frame ni
ligne logique supplémentaire. Si une UIDVALIDITY différente est observée
pour la source/mailbox déjà connue, l’import s’arrête avec une erreur
explicite ; aucune correspondance spéculative n’est tentée.

RFC 8474 OBJECTID, avec MAILBOXID/EMAILID/THREADID éventuels, reste une
extension future optionnelle.

## Mailboxes multiples

Le modèle de provenance a été étendu sans changement de framing RAW ni du
schéma de recherche. `imap_mailboxes` est une table de métadonnées observées
par mailbox; `imap_scan_state` reste la frontière de parcours indépendante.
Les rows de message de deux mailboxes restent deux occurrences, même lorsque
leurs octets RAW sont identiques. Une future étude pourra mesurer une
déduplication physique, mais elle ne doit pas modifier l’identité logique
observée ici.

La découverte et le filtrage sont exposés par le CLI, par exemple:

```text
imap-import ... --mailbox INBOX --mailbox Sent
imap-import ... --all-mailboxes
```

Les résultats affichent les capacités, le nom serveur, le delimiter, les
attributs et SPECIAL-USE. Le backend garde `EXAMINE` et `BODY.PEEK[]`; aucune
commande distante d’écriture n’a été ajoutée.

## Campagne multi-mailbox GreenMail

Une campagne Linux a utilisé GreenMail standalone 2.1.12 avec une CA locale,
un certificat serveur signé par cette CA et SAN `localhost`, `imap.test` et
`127.0.0.1`. Le client Memoria a chargé la CA dans `RootCertStore` rustls;
aucun verifier dangereux n’a été utilisé.

Le serveur a reçu les mailboxes `INBOX`, `Sent`, `Archive`, `Projects`,
`Projects.Alpha`, `Projects.Beta`, `Empty` et `Caf&AOk-`. GreenMail a annoncé
un delimiter `.` et aucun attribut SPECIAL-USE. Ses capacités observées
étaient `IMAP4rev1`, `UIDPLUS`, `IDLE`, `MOVE`, `SORT`, `QUOTA`, `LITERAL+`,
`SASL-IR` et `XOAUTH2`; ni `IMAP4rev2`, ni `UTF8=ACCEPT`, ni `SPECIAL-USE`
n’ont été observés.

`Caf&AOk-` est le nom modified UTF-7 envoyé par le client de préparation et
retourné par `LIST`. Memoria a réutilisé exactement cette valeur pour
`EXAMINE`; l’ouverture de la mailbox vide a réussi. Cette expérience valide
la conservation du nom protocolaire, pas encore sa présentation Unicode.
Nom protocolaire, futur nom d’affichage, delimiter et SPECIAL-USE restent des
concepts distincts.

La campagne a d’abord créé `Projects`, puis a construit les enfants avec le
delimiter effectivement retourné (`Projects.Alpha` et `Projects.Beta`).
`LIST "" "*"` a retourné les trois noms et `LIST "" "Projects.%"` a retourné
les deux enfants. `EXAMINE "Projects.Alpha"` et `EXAMINE "Projects.Beta"`
ont réussi. GreenMail n’a pas ajouté `\\HasChildren` ou `\\HasNoChildren` dans
ses attributs pour cette configuration. Un nom séparé `Projects/with-slash`
était sélectionnable et confirme que `/` est ici un caractère ordinaire.

Résultats de l’import produit pour cette campagne hiérarchique :

```text
premier passage : INBOX=1, Projects.Alpha=1,
                  autres mailboxes sélectionnables vides
second passage  : raw_fetched=0 et new_messages=0 pour chaque mailbox
ajout Alpha     : Projects.Alpha raw_fetched=1, autres raw_fetched=0
occurrences     : 3 (shared dans INBOX puis Alpha, alpha dans Alpha)
```

Les occurrences `shared` restent des lignes de provenance distinctes et des
frames séparées; aucune fusion par hash, Message-ID ou sujet n’est effectuée.
La campagne précédente de duplication avait mesuré 5 occurrences, 3 RAW
distincts et 2 occurrences dupliquées; cette mesure concernait le cas de
provenance, pas une hiérarchie `/`.
La commande garde les frames RAW et l’export EML existant reste fondé sur ces
octets de référence; l’égalité byte-for-byte de l’export est couverte par la
primitive EML et ses tests dédiés, tandis que l’expérience multi-mailbox ne
copie aucun fichier personnel dans le dépôt.

GreenMail n’a pas annoncé SPECIAL-USE; l’interopérabilité de cette extension
reste donc non mesurée au niveau serveur. Les tests unitaires conservent la
distinction des attributs SPECIAL-USE.

Le replay Windows de cette logique a ensuite été exécuté sur N16PRO avec le
même GreenMail TLS et la même CA de test. Le premier import multi-mailbox a
retourné 20 occurrences : INBOX=15, Projects.Alpha=1,
Projects.Beta=1, Caf&AOk-=1 et Projects/with-slash=1; Projects et Empty
étaient vides. Le second import a retourné `raw_fetched=0` pour chaque
mailbox. Après l'ajout d'un message dans Projects.Alpha, seul ce mailbox a
retourné `raw_fetched=1` et `new_messages=1`.

Le même replay Windows a observé `CAPABILITY`, `LIST "" "*"`, le delimiter
`.`, `LIST "" "Projects.%"`, puis `EXAMINE` des deux enfants. Le nom
modified UTF-7 `Caf&AOk-` a été réutilisé tel quel et `/` est resté un
caractère ordinaire dans `Projects/with-slash`. Les FLAGS contrôlées sont
restées vides (`\\Seen` absent). GreenMail n'annonce toujours pas
SPECIAL-USE.

Dans l'archive produite par Windows, 20 occurrences contiennent 12 RAW
distincts et 8 occurrences supplémentaires byte-identiques. Les occurrences
partagées restent donc des provenances séparées. Les frames contrôlées ont
été comparées aux fixtures par SHA-256; l'export EML de la frame Windows
correspondante a également produit le SHA-256 du RAW, sans reconstruction MIME.

La clé est aussi une contrainte `PRIMARY KEY` SQLite sur
`imap_messages(source_account, mailbox, uid_validity, uid)` ; le contrôle
préalable dans le code n'est donc pas l'unique protection contre un doublon.

L'ordre d'ingestion est `append_raw` puis insertion du catalogue IMAP, puis
`ArchiveWriter::sync`. Une chute entre ces étapes peut laisser un RAW appendé
mais non référencé, ce qui reste récupérable par la reconstruction ; aucune
ligne de catalogue n'est créée avant que `append_raw` ait fourni une
localisation de frame. La séquence ne constitue toutefois pas une transaction
atomique RAW+SQLite lors d'une panne matérielle : cette garantie plus forte
reste une dette distincte.

## Synchronisation incrémentale minimale

La table `imap_scan_state` conserve, pour chaque
`source_account + mailbox + UIDVALIDITY`, une frontière
`scanned_through_uid` et le dernier `UIDNEXT` observé. Cette frontière est une
métadonnée de provenance/synchronisation : elle n'appartient pas au message
MIME et n'est pas reconstructible depuis le RAW.

Au début d'une campagne, `EXAMINE` relève `UIDVALIDITY` et `UIDNEXT`. Après une
frontière publiée, le client demande seulement `scanned_through_uid + 1`. La
borne supérieure est `UIDNEXT - 1` sans limite, ou la fin de la tranche
`--limit`. Le snapshot est borné au début de la campagne ; les messages
arrivés ensuite sont laissés à la campagne suivante. La frontière n'est
écrite qu'après la fin propre du FETCH et la synchronisation de l'archive.

`--limit N` borne le nombre d'UID de la campagne courante à partir de la
frontière et avance celle-ci jusqu'à la borne effectivement parcourue. Une
interruption réseau suit la règle inverse : les messages déjà écrits restent
protégés par l'identité SQLite, mais aucune nouvelle borne n'est publiée.
Cette frontière est la borne UID parcourue, jamais le plus grand UID retourné
ou présent dans SQLite ; des UID absents dans une tranche n'empêchent donc pas
sa progression.

Résultats GreenMail synthétiques (12 fixtures, puis 3 nouvelles, campagne
limitée à 5) :

```text
limit 1       examined=5  raw_fetched=5  new=5  frontier 0  -> 5
limit 2       examined=5  raw_fetched=5  new=5  frontier 5  -> 10
limit 3       examined=5  raw_fetched=5  new=5  frontier 10 -> 15
limit repeat  examined=0  raw_fetched=0  new=0  frontier 15 -> 15
```

Le parcours traite chaque réponse FETCH au fil de l'eau : le vecteur global
des messages récupérés a été supprimé ; la mémoire de cette étape est donc
bornée par le message courant et les structures d'archive/catalogue. Le
serveur a été redémarré avec une nouvelle UIDVALIDITY : l'import a refusé la
campagne avec `UidValidityChanged` avant toute utilisation de l'ancienne
frontière.

Le replay Windows de la sélection incrémentale a été exécuté sur N16PRO avec
la même CA TLS. Résultats :

```text
initial       examined=12 raw_fetched=12 new=12 frontier 0  -> 12
unchanged     examined=0  raw_fetched=0  new=0  frontier 12 -> 12
after +3      examined=3  raw_fetched=3  new=3  frontier 12 -> 15
repeat        examined=0  raw_fetched=0  new=0  frontier 15 -> 15

--limit 5     raw_fetched=5 new=5 frontier 0  -> 5
--limit 5     raw_fetched=5 new=5 frontier 5  -> 10
--limit 5     raw_fetched=5 new=5 frontier 10 -> 15
repeat        raw_fetched=0 new=0 frontier 15 -> 15
```

`UIDVALIDITY` est restée stable et `UIDNEXT` a progressé de 13 à 16 après
l'ajout des trois messages. Les FLAGS contrôlées sont restées sans `\\Seen`.

## Validation GreenMail Linux

Serveur : GreenMail standalone 2.1.12, Linux, IMAPS 3993, certificat serveur
PKCS#12 signé par une CA locale de test avec SAN `localhost`. Les 12 fixtures
du probe IMAP ont été préchargées.

Résultat du premier import :

```text
examined=12
new_messages=12
network_bytes=4214
archive_bytes_added=4598
uidvalidity=1787416753
indexed=12
```

Le second import identique a produit `new_messages=0`,
`archive_bytes_added=0` et `indexed=0`. Les recherches anonymes sur du texte
français/japonais et sur le message avec pièce jointe ont retrouvé les
documents importés.

Les erreurs suivantes ont été vérifiées : mauvais mot de passe, mailbox
inexistante et port fermé. Elles retournent une erreur contrôlée ; la
connexion et les opérations réseau restent bornées.

## Fidélité RAW et export

Les 12 frames importées totalisent 4 214 octets RAW. Le SHA-256 de chaque RAW
correspond aux fixtures du probe IMAP. Le message attachment de `doc_id=4`
fait 494 octets et a été exporté par la primitive EML existante avec :

```text
RAW : cc6078662a70cdb5e9ca4106c516b84e175e068d22b81575d8a653a2294e7b44
EML : cc6078662a70cdb5e9ca4106c516b84e175e068d22b81575d8a653a2294e7b44
```

L’export réutilise `read_archived_raw` et ne reconstruit pas le MIME.

## Validation Windows native

Le même code a été compilé en profil CI sur Windows x86-64 et exécuté contre
GreenMail Linux via le réseau local. Résultat :

```text
examined=12 new_messages=12 network_bytes=4214 archive_bytes_added=4598 indexed=12
examined=12 new_messages=0 network_bytes=0 archive_bytes_added=0 indexed=0
```

Les 12 frames Windows ont été relues hors ligne : 4 214 octets et 12 SHA-256
identiques aux fixtures. Les recherches Unicode et attachment ont retrouvé
les mêmes documents. Les flags récupérés restent `seen=false`.

Le clone Windows de cette validation était exactement au commit
`0f348371515d1e57b241894e2a767019541750e0`. La validation native a également
passé `cargo check --workspace` et `cargo test --workspace` sans échec :
45 tests de bibliothèque, 7 tests de l'application, 1 test thumbnail et
2 tests workspace ont réussi.

## Coût et limites

Nouvelles dépendances directes : `async-imap`, `futures`, `tokio`, `rustls`,
`rustls-pki-types` et `tokio-rustls`. Le backend TLS réutilise la pile
`aws-lc-rs` déjà tirée par rustls dans le graphe.

Mesures profil CI après intégration :

```text
Linux imap-import       12 375 680 octets
Linux mail-archive-app  36 797 600 octets
Windows imap-import     10 036 224 octets
Windows mail-archive-app 30 222 848 octets
```

Le chemin importe actuellement les RAW récupérés de la campagne dans une
collection de travail avant l’écriture. C’est acceptable pour cette première
campagne, mais devra être réexaminé avant de viser de très grandes mailboxes.
Il n’y a pas encore de suppression, MOVE/COPY, CONDSTORE/QRESYNC, IDLE,
OBJECTID, STARTTLS ou onboarding UI. Le refetch complet et la matérialisation
du message courant restent les prochains sujets de scalabilité si les
mailboxes deviennent très grandes.
