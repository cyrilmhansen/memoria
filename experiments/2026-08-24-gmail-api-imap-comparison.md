# Comparaison expérimentale Gmail API / Gmail IMAP

Date : 2026-08-24

## Statut

Cette passe est une étude de correspondance, sans modification de Memoria.
L’expérience sur un compte Gmail réel n’a pas pu être exécutée dans cet
environnement : aucun compte/test corpus Gmail autorisé n’est configuré et le
CLI IMAP actuel ne possède pas de mécanisme XOAUTH2/OAuth IMAP. Il utilise
`LOGIN` avec un mot de passe fourni explicitement. Aucun secret n’a été lu,
copié ou journalisé.

Conclusion de cette passe : **UNRESOLVED — aucune équivalence Gmail API/IMAP
ne doit être déduite avant une campagne autorisée**.

## Audit du connecteur Gmail API

Le connecteur API utilise le scope strict `gmail.readonly`, décode le champ
API `raw` base64url et conserve les métadonnées suivantes dans les structures
de synchronisation :

```text
gmail_message_id
thread_id
label_ids
history_id / message_history_id
internal_date_ms
RAW MIME décodé
```

Dans SQLite, `gmail_messages` possède une clé primaire
`source_account + gmail_message_id`, un `thread_id`, `label_ids`,
`internal_date_ms`, un historique de message et un état de source. Le RAW
reste dans les frames de l’archive; la table est une provenance/catalogue
dérivé côté navigation.

Les listes API `messages.list` exposent l’ID et le thread, puis les réponses
metadata/raw fournissent les labels, l’historique, la date interne et le RAW.
Le connecteur ne conserve pas actuellement de `X-GM-MSGID`, `X-GM-THRID` ou
de notion d’occurrence mailbox IMAP.

## Audit du chemin IMAP actuel

Le CLI utilise :

```text
CAPABILITY → LIST → EXAMINE → UID FETCH
  (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])
```

La provenance SQLite est séparée par
`source_account + mailbox + UIDVALIDITY + UID`, avec `FLAGS`,
`INTERNALDATE` et taille RFC822. Les mailboxes et leur delimiter/attributs
restent dans `imap_mailboxes`. Le RAW IMAP est archivé tel que reçu.

Le FETCH actuel ne demande pas encore `X-GM-MSGID`, `X-GM-THRID` ni
`X-GM-LABELS`, et le client ne possède pas de chemin OAuth/XOAUTH2. Gmail
IMAP ne peut donc pas être utilisé par ce CLI sur un compte moderne sans
ajouter explicitement cette capacité, ce qui est hors périmètre de cette
expérience.

## Mesure réelle

```text
Gmail API messages comparés       : non mesuré dans cette passe
Gmail IMAP occurrences comparées  : non mesuré dans cette passe
RAW byte-for-byte                 : non mesuré
X-GM-MSGID/THRID/LABELS           : non mesuré
duplications inter-mailboxes      : non mesuré pour Gmail
```

Les campagnes GreenMail précédentes valident seulement le protocole IMAP
readonly et la conservation de plusieurs occurrences. Elles ne constituent
pas un résultat Gmail : GreenMail ne fournit ni modèle Gmail labels, ni
extensions `X-GM-*` observées ici.

## Ce qu’il faudra mesurer avec un compte de test

Le protocole de la prochaine campagne devra produire, sans contenu personnel
dans le dépôt, une ligne par correspondance :

```text
API message id | threadId | labelIds | historyId | internalDate | API SHA-256
mailbox | UIDVALIDITY | UID | FLAGS | INTERNALDATE |
X-GM-MSGID | X-GM-THRID | X-GM-LABELS | IMAP SHA-256
```

Il faudra inclure INBOX, All Mail, Sent, Archive, labels multiples, un thread
de réponses et une pièce jointe. Les comptes Gmail peuvent exposer un même
message dans plusieurs vues/mailboxes; cette hypothèse reste précisément à
mesurer, pas à fusionner.

## Modèle minimal à envisager après mesure

Ne pas modifier le schéma maintenant. Si l’expérience confirme que Gmail API
et IMAP décrivent le même message avec plusieurs occurrences, le changement
minimal probable serait de séparer :

```text
logical message / RAW authority
    └── source occurrence
          ├── Gmail id + thread + labels + history
          └── IMAP mailbox + UIDVALIDITY + UID + FLAGS + INTERNALDATE
```

Une table d’occurrence référençant un message logique permettrait de ne pas
confondre identité Gmail, provenance IMAP et stockage physique RAW. Les
extensions `X-GM-*` seraient des métadonnées de correspondance observées,
non une justification immédiate de déduplication.

## Limites et suite

- Ajouter XOAUTH2 au probe, ou fournir un mécanisme de jeton explicitement
  hors archive, devra être une passe séparée.
- Il faudra vérifier si API RAW et IMAP `BODY.PEEK[]` sont byte-for-byte
  identiques; aucune normalisation ne doit être supposée.
- Les rôles SPECIAL-USE et labels système devront être comparés sur Gmail
  réel, pas inférés de GreenMail.
- Les flags IMAP et labels API peuvent représenter des dimensions différentes.
