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

Le mot de passe et la CA sont fournis hors archive et hors dépôt. Aucun secret
n’est écrit dans les logs. Le port TLS implicite est utilisé ; STARTTLS,
OAuth IMAP et la découverte de mailboxes restent hors périmètre.

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

Le chemin Windows natif n'a pas pu être rejoué dans cette passe : l'hôte
N16PRO était momentanément injoignable en SSH (`No route to host`). Le check
Windows précédent du chemin IMAP complet reste valide, mais la nouvelle
sélection incrémentale devra être rejouée sur ce runner.

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
