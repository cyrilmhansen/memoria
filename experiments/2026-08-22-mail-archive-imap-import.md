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
Il n’y a pas encore de synchronisation incrémentale IMAP, de suppression,
MOVE/COPY, CONDSTORE/QRESYNC, IDLE, OBJECTID, STARTTLS ou onboarding UI.
