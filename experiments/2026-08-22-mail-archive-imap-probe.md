# Memoria — probe IMAP de fidélité et d’interopérabilité

Date : 2026-08-22

## Verdict

**GO WITH CAVEATS.** `BODY.PEEK[]` fournit, avec GreenMail 2.1.12, un
message MIME directement réutilisable comme RAW : les 12 fixtures sont
identiques octet par octet après APPEND puis FETCH, en client Linux comme en
client Windows sur le LAN. Une CA de test dédiée valide maintenant aussi
IMAPS depuis Linux et Windows. Cela ne constitue pas encore une campagne de
compatibilité avec des serveurs IMAP réels variés.

## Versions et architecture

- Rust/Cargo : 1.96.0.
- `async-imap` : 0.11.3.
- Tokio : 1.53.1, avec le runtime multi-thread du probe.
- GreenMail standalone : 2.1.12, image Docker `greenmail/standalone:2.1.12`.
- Java dans l’image : OpenJDK 21.0.11.
- Client : CLI Rust indépendant, sans lien avec Memoria.

Le serveur a été lancé avec IMAP plain sur 3143 et IMAPS sur 3993, avec
authentification et un compte synthétique. Pour la campagne TLS, une CA locale
éphémère a signé un certificat serveur PKCS#12 GreenMail avec SAN `localhost`
(également `imap.test` et `127.0.0.1`). Le probe charge explicitement le PEM
de la CA dans le `RootCertStore` rustls et utilise le server name `localhost`;
aucun verifier dangereux n’est utilisé sur le chemin principal. Le probe
utilise `EXAMINE INBOX`,
`UID FETCH 1:* (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])`, des
timeouts par opération et ne demande jamais `BODY[]` ni `STORE +FLAGS`.

Commandes principales :

```text
docker run -d --name memoria-greenmail -p 3143:3143 -p 3993:3993 \
  -e GREENMAIL_OPTS='-Dgreenmail.setup.test.imap -Dgreenmail.setup.test.imaps -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=imap-probe:probe-pass@localhost' \
  greenmail/standalone:2.1.12

cargo run --manifest-path experiments/imap-probe/Cargo.toml -- \
  --host 127.0.0.1 --port 3143 --user imap-probe --password probe-pass \
  --append-fixtures experiments/imap-probe/fixtures \
  --output /var/tmp/imap-fetched
```

Le même binaire a été compilé nativement sur Windows x86-64 puis exécuté
contre l’adresse LAN du serveur Linux. Aucun fichier de travail Windows n’est
une fixture personnelle.

## Corpus

Le corpus contient 12 messages MIME synthétiques en CRLF : ASCII, français
UTF-8, japonais, multipart/alternative, pièce jointe base64, image inline
CID, headers répétés, header plié, adresses Unicode, `message/rfc822`,
`text/calendar` et un cas quoted-printable inhabituel mais valide. Les tailles
se situent entre 267 et 503 octets.

Les fixtures ont été injectées par `APPEND` IMAP, de façon à mesurer le chemin
serveur stocké → `BODY.PEEK[]` sans confondre le résultat avec une éventuelle
transformation SMTP.

## Fidélité locale

| fixture | octets | résultat fixture → fetch |
|---|---:|---|
| 01 ASCII | 267 | byte-exact |
| 02 français | 321 | byte-exact |
| 03 japonais | 294 | byte-exact |
| 04 alternative | 423 | byte-exact |
| 05 attachment | 494 | byte-exact |
| 06 inline CID | 503 | byte-exact |
| 07 repeated headers | 291 | byte-exact |
| 08 folded header | 288 | byte-exact |
| 09 Unicode address | 341 | byte-exact |
| 10 message/rfc822 | 350 | byte-exact |
| 11 calendar | 353 | byte-exact |
| 12 unusual | 289 | byte-exact |

Les SHA-256 des fichiers injectés et récupérés sont égaux pour les 12
fixtures ; aucune différence CRLF, header, boundary ou encodage n’a été
observée. C’est à la fois une mesure de fidélité au fichier injecté et, dans
ce scénario APPEND, de fidélité au message stocké par GreenMail. Elle ne
prouve pas que tous les serveurs préserveront de la même manière les messages
reçus par SMTP.

## Flux Linux → Windows

Le client Windows natif a utilisé le serveur Linux via le réseau local,
authentification incluse. Les 12 UID 1–12 ont été récupérés ; les longueurs et
SHA-256 correspondent exactement aux fixtures Linux. Les réponses ont montré
`UIDVALIDITY`, `UIDNEXT`, `RFC822.SIZE`, `INTERNALDATE` et un jeu de flags par
message. Tous les messages ont conservé `seen=false`, ce qui confirme que
`BODY.PEEK[]` n’a pas marqué les messages lus.

La liste de capacités observée incluait notamment IMAP4rev1, UIDPLUS, IDLE,
MOVE, QUOTA, SORT, SASL-IR, LITERAL+ et XOAUTH2. `LOGIN` ne renvoyait pas de
code de capacités dans la réponse GreenMail ; un `CAPABILITY` explicite après
connexion permettait de les relever.

## IMAPS

Le chemin implicite IMAPS rustls est maintenant validé avec la CA réelle de
test :

| client | serveur | résultat |
|---|---|---|
| Linux | GreenMail 2.1.12, 3993 | TCP, handshake, greeting, login, EXAMINE et FETCH réussis |
| Windows x86-64 | GreenMail 2.1.12 Linux, 3993 via LAN | TCP, handshake, greeting, login, EXAMINE et FETCH réussis |

Les 12 fixtures ont été récupérées sous les deux clients avec les mêmes
SHA-256, sizes et flags `seen=false`. La session observée par OpenSSL utilise
TLS 1.3 et `TLS_AES_256_GCM_SHA384`; le certificat `CN=localhost` est vérifié
par la CA dédiée.

La première campagne utilisait le certificat GreenMail par défaut et un
verifier dangereux ; Windows terminait alors la connexion avant le greeting.
Après remplacement par un certificat signé par la CA explicitement chargée,
le même flux Windows réussit. Le diagnostic retenu est donc
**GREENMAIL_TEST_CERTIFICATE**, et non `RUSTLS_WINDOWS` ou `NETWORK/OS`.
Cette stratégie de CA de test ne doit pas être transposée telle quelle au
produit : Memoria devra utiliser une chaîne de confiance appropriée ou une
configuration de certificat explicitement sûre.

STARTTLS n’a pas été ajouté : le probe actuel ne le supporte pas et son ajout
n’était pas nécessaire pour trancher l’interopérabilité IMAPS implicite.

## Erreurs et bornes

Les erreurs minimales ont été contrôlées :

- mauvais mot de passe → erreur de login, exit non nul ;
- port fermé → `connection failed` contrôlé ;
- hôte non joignable → `connection timeout` avec délai borné.

Chaque opération réseau importante est bornée par `--timeout-ms` (10 s par
défaut). Le probe ne lance pas de shell et ne contient pas de secret réel.
L’arrêt du serveur avant connexion est couvert par le même chemin d’erreur de
connexion ; un test de coupure après connexion reste à élargir sur un serveur
IMAP dédié si nécessaire.

## Métadonnées et modèle Memoria

`UID` seul n’est pas une identité durable : il doit être associé au nom de
mailbox et à `UIDVALIDITY`. Le minimum utile autour du RAW est donc la source,
mailbox, UIDVALIDITY, UID, flags, INTERNALDATE et les informations de taille
observées. Une nouvelle UIDVALIDITY peut rendre les anciens UID invalides.

Par rapport au chemin Gmail actuel, IMAP ajoute une hiérarchie de mailboxes,
des UID locaux à chaque mailbox, `UIDVALIDITY`, flags et INTERNALDATE ; il ne
fournit pas directement l’équivalent Gmail `historyId`. La conservation du
RAW reste compatible, mais la stratégie d’idempotence et de suppression doit
être définie avec ces métadonnées IMAP.

Le probe utilise Tokio. `async-imap` est asynchrone et supporte des runtimes
optionnels, notamment Tokio et async-std ; le choix du runtime produit reste
ouvert. Pour Memoria, le client devrait rester derrière un worker contrôlé
plutôt que faire entrer une boucle async dans le thread Slint. Cette
expérience ne crée pas encore l’abstraction `MailSource`.

## Coût du probe

Le graphe direct est limité à `async-imap`, Tokio, futures, mailparse, sha2 et
les composants rustls (`rustls`, `rustls-pki-types`, `tokio-rustls`). Le binaire
Linux release mesuré est de 6 151 368 octets. Le binaire Windows x86-64 release
mesuré est de 1 385 472 octets. Ces tailles incluent le chemin IMAPS du probe,
pas Memoria.

## Questions ouvertes

- fidélité sur des serveurs IMAP autres que GreenMail et sur une livraison SMTP ;
- politique de mapping mailbox/UIDVALIDITY et des suppressions ;
- prise en compte éventuelle de RFC 8474 `OBJECTID` : `MAILBOXID`, `EMAILID`
  et éventuellement `THREADID`. Cette extension resterait optionnelle et ne
  serait jamais exigée pour le fonctionnement IMAP de base ;
- stratégie de confiance/certificats IMAPS avec des autorités réelles, au-delà
  de la CA locale de test ;
- comportement de serveurs avec extensions ou réponses FETCH partielles ;
- coût et intégration d’un worker async avant toute abstraction multi-source.

Les fichiers volumineux et sorties de test ont été conservés hors dépôt sous
`/var/tmp` pendant l’expérience puis supprimés après validation.
