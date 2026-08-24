# Probe Gmail IMAP XOAUTH2

Date : 2026-08-24

## Statut

```text
AUTH_PATH_IMPLEMENTED       : oui, dans un probe expérimental isolé
AUTH_PROTOCOL_LOCALLY_VALIDATED : oui, avec un faux serveur TCP IMAP local
AUTH_PATH_TESTED_AGAINST_GMAIL : non, aucun compte/token autorisé fourni
GMAIL_COMPARISON_EXECUTED   : non
```

Cette passe ne modifie pas le connecteur Gmail de Memoria, son scope
`gmail.readonly`, son catalogue ou son archive. Le blocage de la validation
réelle est double : aucun compte Gmail de test autorisé n'est disponible dans
cet environnement et le jeton OAuth IMAP/XOAUTH2 nécessaire n'a pas été
fourni. Aucun token, refresh token, credential ou contenu Gmail n'a été lu.

## Probe

Le binaire isolé est `experiments/gmail-imap-xoauth2-probe/`. Il utilise
`async-imap` 0.11.3 avec Tokio, `rustls` 0.23 et les racines publiques
`webpki-roots`. La connexion est limitée à `imap.gmail.com:993`, avec SNI et
validation TLS normale. Aucun verifier dangereux n'est présent.

Le token d'accès est accepté, par ordre de préférence, depuis :

```text
--token-stdin
MEMORIA_GMAIL_IMAP_XOAUTH2_TOKEN
--token-file PATH
```

Il n'existe aucun argument contenant directement le bearer token. Le probe ne
demande ni ne stocke de refresh token. Le type secret a un `Debug` redacted,
les erreurs d'authentification sont génériques et les erreurs transport
remplacent le token s'il apparaissait dans un message externe.

Après authentification, le chemin normal ne contient que :

```text
TLS → greeting → AUTHENTICATE XOAUTH2 → CAPABILITY → LIST → LOGOUT
```

L'option explicite `--fetch` ajoute uniquement `EXAMINE INBOX` et un `UID
FETCH` readonly avec `UID FLAGS INTERNALDATE RFC822.SIZE X-GM-MSGID
X-GM-THRID X-GM-LABELS BODY.PEEK[]`. Aucun `SELECT`, `STORE`, `APPEND`,
`COPY`, `MOVE` ou `EXPUNGE` n'est utilisé.

## Tests sans compte Gmail

Les quatre tests unitaires passent :

- construction de la réponse SASL `user=...\x01auth=Bearer ...\x01\x01` ;
- absence du token dans `Debug` ;
- masquage du token dans les erreurs ;
- échec propre lorsqu'aucune source de token n'est fournie.

## Validation wire-level locale

Le probe contient désormais un serveur IMAP factice dans les tests. Il écoute
sur `127.0.0.1`, envoie un greeting, lit la ligne `AUTHENTICATE XOAUTH2`,
envoie un challenge vide, puis décode la réponse base64 effectivement écrite
par `async-imap` sur le socket. Le payload décodé observé est exactement :

```text
user=test@example.invalid\x01auth=Bearer TEST_TOKEN\x01\x01
```

Après une réponse `OK`, le faux serveur accepte et vérifie réellement
`CAPABILITY`, `LIST` et `LOGOUT`. Un second scénario renvoie
`NO [AUTHENTICATIONFAILED]`; le client retourne l'erreur générique
`XOAUTH2 authentication failed`, sans faire apparaître `TEST_TOKEN` dans
l'erreur ou les diagnostics du test. Le test wire-level ne contacte aucun
serveur externe.

Le faux serveur n'exerce pas encore le chemin `X-GM-*`/`UID FETCH`; ce point
reste secondaire. Le probe de production est préparé à demander ces
attributs avec `--fetch`, mais leur parsing n'est pas déclaré validé ici.

Un lancement sans token retourne une erreur de source manquante. Un lancement
avec un token factice n'a pas atteint Gmail dans cet environnement (résolution
réseau indisponible) et n'a pas réémis le token dans sa sortie. Cela ne vaut
pas comme test d'authentification Gmail.

Validations locales :

```text
cargo test --offline --manifest-path experiments/gmail-imap-xoauth2-probe/Cargo.toml : 6 passed
cargo check --offline --manifest-path experiments/gmail-imap-xoauth2-probe/Cargo.toml : success
release probe : 5 922 488 octets
```

## Ce qui reste à mesurer

Avec un compte et un access token autorisés, il faudra confirmer séparément
le handshake XOAUTH2 réel, les capacités Gmail, puis `LIST`, `EXAMINE` et les
champs `X-GM-*` du `--fetch`. Une fois seulement ces mesures obtenues, la
comparaison Gmail API ↔ Gmail IMAP pourra relever les IDs, labels, threads,
RAW et duplications. Les correspondances restent donc des hypothèses à
mesurer, et aucune équivalence API/IMAP n'est déduite de ce probe.

Le probe reste volontairement distinct du produit : il ne modifie ni le
scope Gmail API, ni le schéma SQLite, ni le framing RAW, ni le chemin OAuth
desktop de Memoria.
