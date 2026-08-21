# Expérience — connecteur Gmail strictement read-only

Date : 2026-08-20  
Branche : `mail-archive`  
Statut : prototype de transport et synchronisation, sans compte réel importé
dans cet environnement.

## But et périmètre

Cette étape vérifie le chemin Gmail réel sans modifier le compte : OAuth
desktop loopback, `users.messages.list`, récupération `format=RAW`, catalogue
des métadonnées, réutilisation de l’archive append-only et synchronisation par
`users.history.list`. Les données de message ne sont jamais imprimées.

Le seul scope demandé par le code est :

```text
https://www.googleapis.com/auth/gmail.readonly
```

Les opérations d’écriture Gmail ne font pas partie du prototype. Les
références utilisées sont la documentation officielle de
[RAW et messages Gmail](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages),
de [l’énumération des messages](https://developers.google.com/workspace/gmail/api/guides/list-messages),
de [l’historique](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.history/list)
et du [flux OAuth desktop loopback](https://developers.google.com/identity/protocols/oauth2/native-app).

## Implémentation

Le transport HTTP est isolé derrière `GmailTransport`, avec seulement trois
opérations de lecture : `list`, `get_raw` et `history`. Les réponses RAW sont
décodées en base64url, puis les bytes RFC/MIME sont transmis directement à
`ArchiveWriter::append_raw`. Le générateur synthétique n’est pas visible dans
ce chemin.

Le catalogue conserve séparément :

- `source_account` + Gmail message ID comme identité source idempotente ;
- thread ID, label IDs, internal date et history ID ;
- l’état source `present` ou `deleted` ;
- la position de la frame RAW dans les segments.

Une suppression ou disparition Gmail ne supprime donc pas la frame. Une full
sync complète peut seulement passer l’état source à `deleted`. Une full sync
bornée par `--max-messages` ou `--query` n’est pas utilisée pour conclure à une
absence.

OAuth lit un client installé, stocke le refresh token dans un répertoire de
configuration distinct de l’archive et vérifie le scope lors de l’autorisation
et du refresh. Les chemins credentials/token sont refusés s’ils sont sous le
répertoire d’archive. Le dépôt ignore les fichiers credentials et tokens.

## Résultats reproductibles sans compte réel

Commande :

```text
cargo fmt --all
cargo test -p mail-archive-experiment
```

Résultat : 9 tests passent. La fixture Gmail contient deux messages répartis
sur deux pages.

| passage | messages examinés | nouveaux | frames ajoutées | remarque |
|---|---:|---:|---:|---|
| initial | 2 | 2 | 2 | pagination et RAW binaire |
| immédiat | 0 | 0 | 0 | history vide, idempotent |
| history expiré | 2 | 0 | 0 | repli full sync, archive conservée |

La fixture vérifie aussi que la table `gmail_messages` reste à deux entrées.
Le scénario réel doit être exécuté localement par le développeur avec les
étapes 100, 1 000 puis éventuellement 10 000 messages ; aucune statistique
de compte réel n’est inventée ici.

Commande de campagne :

```text
cargo run -p mail-archive-experiment -- gmail-sync \
  --archive /chemin/archive \
  --credentials /chemin/client_secret.json \
  --account compte-principal --max-messages 100
cargo run -p mail-archive-experiment -- gmail-sync \
  --archive /chemin/archive \
  --credentials /chemin/client_secret.json \
  --account compte-principal --max-messages 1000
```

La sortie est limitée à des compteurs, volumes, durée et agrégats MIME. Elle
ne doit pas être copiée avec des logs OAuth ou des données personnelles.

## MIME et statistiques CAS

`mailparse 0.16.1` est utilisé uniquement comme analyse dérivée. Il sait
identifier les feuilles avec disposition attachment ou paramètres filename/name
et fournir payload encodé et payload décodé pour les statistiques. Les headers
et les corps vus par son API ne constituent pas la source d’autorité : certaines
représentations peuvent être normalisées et les offsets MIME ne sont pas encore
persistés.

Pour chaque message importé, le prototype compte sans contenu : échec de
parsing, nombre de pièces jointes, octets encodés, octets décodés, hashes
BLAKE3 uniques dans le lot et octets des payloads encodés d’au moins 64 KiB.
Cela permet de calculer après campagne :

- CAS exact théorique : `encoded_total - encoded_unique` ;
- CAS décodé théorique : `decoded_total - decoded_unique` ;
- variante seuil 64 KiB : économie limitée aux payloads encodés au-dessus du
  seuil ;
- duplication par objets et par octets.

Ces mesures ne modifient pas les frames et ne créent aucun CAS réel.
Les messages dont le parsing échoue ne sont pas inclus dans une statistique
MIME présentée comme certaine.

## Contrat de conservation vérifié

Le chemin RAW rend possible le contrat byte-exact :

```text
bytes téléchargés après décodage base64url
== bytes lus depuis la frame archive
```

Le test binaire couvre déjà des bytes non ASCII. Les cas MIME difficiles
(multipart imbriqué, quoted-printable, base64, headers repliés, Content-ID et
noms encodés) doivent être ajoutés à des fixtures dédiées avant de tirer une
conclusion sur la couverture du parseur. Ils ne doivent jamais être remplacés
par une reconstruction MIME pour l’autorité.

## Limites et risques

- Aucune distribution de tailles, duplication ou anomalie MIME réelle n’est
  rapportée : aucun compte n’a été autorisé/importé dans cette session.
- La réconciliation complète relit les RAW déjà connus ; elle coûte donc du
  réseau même si elle n’ajoute aucun byte d’archive. Le chemin incrémental est
  celui qui doit rester pratiquement vide lorsque l’historique est disponible.
- Une interruption après écriture de la frame mais avant insertion catalogue
  peut laisser une frame orpheline. Elle ne rend pas illisible un message déjà
  validé, mais une reprise transactionnelle plus fine reste une expérience
  ouverte avant produit.
- Une expiration d’historique déclenche une full sync sans effacer les données,
  conformément au modèle Gmail ; la fenêtre d’énumération et son coût doivent
  être mesurés sur un compte contrôlé.
- Tantivy n’est pas relancé dans cette étape. L’indexation des RAW réels reste
  une étape séparée, sans rapport sensible.

## Conclusion provisoire

**Fait vérifié :** l’API RAW et le format base64url permettent de conserver une
copie byte-exacte indépendante du parsing MIME.  
**Fait vérifié :** l’identité source `source_account + gmail_message_id`
empêche les doublons lors d’une seconde synchronisation normale.  
**Fait vérifié :** une suppression source peut être représentée sans supprimer
la sauvegarde locale.  
**Hypothèse :** le modèle history + réconciliation complète sera suffisant pour
un compte de taille courante ; le coût doit être mesuré avec 100/1 000/10 000
messages réels.  
**Décision de projet :** rester RAW-inline pour le premier import Gmail ; les
statistiques CAS sont seulement calculées en lecture seule.

## Questions ouvertes et prochaine étape

La prochaine étape unique doit être une campagne contrôlée sur un compte de
test avec 100 puis 1 000 messages, en conservant uniquement les sorties
agrégées. Elle doit enregistrer les p50/p90/p99 de taille RAW, les octets
réseau/archive, les résultats MIME/CAS théoriques et les trois passages
initial → sans changement → history incrémental. Tant que cette campagne n’est
pas faite, les profils synthétiques ne peuvent pas être comparés aux tailles
réelles et aucune décision CAS ne doit être promue.
