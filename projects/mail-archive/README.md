# mail-archive — implémentation actuelle de Memoria

Ce crate contient l’implémentation actuelle de Memoria : archive locale RAW,
catalogue SQLite mixte, index Tantivy dérivé, UI de recherche/consultation et
connecteur Gmail readonly. Il reste en développement et n’est pas encore une
release stabilisée.

L’import IMAP readonly multi-mailbox existe comme capacité CLI/expérimentale ;
il n’est pas intégré au parcours UI produit. L’export EML individuel et batch
est disponible et byte-exact ; la restauration complète ou la migration vers
un fournisseur ne le sont pas.

## Exécution

Depuis la racine du workspace :

```text
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- generate --messages 10000 --seed 42 --out /tmp/mail-archive
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- generate --profile personal --messages 10000 --seed 42 --out /tmp/mail-archive-personal
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- generate --profile heavy --messages 5000 --seed 42 --out /tmp/mail-archive-heavy
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- benchmark --messages 100000 --seed 42 --attachment-rate 0 --queries 2 --segment-bytes 67108864 --out /tmp/mail-archive-bench
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- cas-benchmark --profile personal --messages 10000 --seed 42 --out /tmp/mail-archive-cas
cargo test -p mail-archive-experiment
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- gmail-report --archive /chemin/archive
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- archive-inventory --archive /chemin/archive
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- recovery-plan --archive /chemin/archive
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- salvage-orphan --archive /chemin/archive --segment segment-000000.arc --offset N --output /chemin/salvage.eml
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- recover-gmail-raw --archive /chemin/archive --doc-id N --credentials /chemin/client_secret.json
cargo run -p mail-archive-experiment --bin mail-archive-app
cargo run -p mail-archive-experiment --bin mail-archive-app -- --archive /chemin/archive
cargo run -p mail-archive-experiment --bin mail-archive-app -- --archive /chemin/archive --benchmark
```

## Synchronisation Gmail en lecture seule

Une récupération expérimentale explicite est disponible pour un seul
`doc_id` : `recover-gmail-raw` re-fetch uniquement l'identité Gmail durable,
compare le BLAKE3 au digest historique et ne publie qu'en cas d'égalité
exacte. Avant cette validation complète, elle ne crée ni segment RAW ni frame
et ne modifie ni le catalogue, ni les métadonnées Gmail, ni la frontière
d'historique.
Le chemin Gmail exact re-fetch R2.1a est fermé pour ce périmètre.
Après append durable, un conflit du CAS catalogue peut volontairement laisser
la nouvelle frame `OrphanValidated`; l'ancienne ligne `messages` reste alors
non publiée.

R2.1b IMAP exact re-fetch est fermé pour son périmètre. Il reprend l'identité
`source_account + mailbox + UIDVALIDITY + UID`; les anciennes clés IMAP
`--source` libres ne sont pas automatiquement reliées à une session et restent
insuffisamment prouvées pour un refetch. Le format moderne est
`imap:{username}@{host}:{port}`. `UID FETCH ... BODY.PEEK[]` est utilisé avec
tagged completion `OK`, exactement une réponse et un payload full-message ;
toute ambiguïté est refusée. Les flags et le frontier restent inchangés.

R2.1 — re-fetch assisté par source est fermé pour Gmail + IMAP. Les deux
providers partagent `publish_exact_recovered_raw` : une destination RAW fraîche
et durable est publiée par A3.2/CAS ; un conflit peut laisser une frame
`OrphanValidated` sûre sans modifier l’ancienne ligne catalogue. R2.2a —
export explicite byte-exact d’un `OrphanValidated` — est fermé : la commande
`salvage-orphan` revalide sous single-writer une preuve physique complète,
exporte le payload RAW dans une destination externe `create_new` et produit
un manifest limité aux faits physiques. Elle ne fait aucune adoption,
mutation catalogue/RAW/frontier ou index; R2.2 global reste ouvert.

Le connecteur utilise exclusivement le scope
`https://www.googleapis.com/auth/gmail.readonly`. Il appelle `list`, `get` en
`format=RAW` et `history`; aucune opération Gmail d’écriture n’est présente
dans le connecteur. Les bytes RAW décodés de base64url sont la représentation
faisant autorité, tandis que les IDs Gmail, thread, labels, dates et history IDs
restent dans le catalogue SQLite.

```text
cargo run -p mail-archive-experiment --bin mail-archive-experiment -- gmail-sync \
  --archive /chemin/archive \
  --credentials /chemin/client_secret.json \
  --max-messages 100
```

La synchronisation dérive `source_account` depuis l'adresse renvoyée par le
profil Gmail : c'est une identité locale opaque
`gmail:<BLAKE3(email canonique)>`. `--account`, lorsqu'il est fourni, est une
assertion sur l'adresse e-mail attendue ; il ne devient jamais l'identité
persistée et l'utilisateur n'a pas à connaître la clé opaque.

`--query` accepte une requête Gmail pour borner une expérience. Si `--account`
est fourni, il doit correspondre à l'adresse du profil Gmail authentifié
(trim et minuscules ASCII) ; la CLI dérive ensuite elle-même la clé opaque dans
le parcours normal.
La synchronisation incrémentale utilise le
`historyId`; si Gmail ne conserve plus cet historique, le connecteur repart en
full sync sans effacer l’archive. Une full sync complète marque les messages
absents comme supprimés côté source, mais ne supprime jamais leurs frames. Sa
frontière est capturée au début de l’énumération, depuis le `historyId` observé
sur le premier message de la première page ; elle n’est publiée que si cette
preuve existe. Les
records d’historique sont ordonnés par `historyId`, sans priorité inventée
entre les champs d’un même record. Une requête ou une limite restaure les
messages effectivement observés, sans déduire d’absence hors périmètre.

Configuration minimale : créer un projet Google Cloud, activer Gmail API,
configurer l’écran de consentement, créer un client OAuth **Desktop app**, puis
télécharger le JSON installé. Le premier lancement ouvre un navigateur et
utilise une redirection loopback locale. Credentials et tokens doivent rester
hors du répertoire d’archive et sont ignorés par Git. Aucun token, contenu ou
attribut MIME n’est écrit dans les logs.

Le benchmark produit des lignes `key=value` et crée quatre familles
d’artefacts séparées :

- `archive/segment-*.arc` : données brutes faisant autorité, append-only,
  avec frames length/checksum et segmentation configurable ;
- `metadata.sqlite` : catalogue structuré et positions d’archive, distinct de
  la recherche ;
- `sqlite-fts.db` : index FTS5 dérivé et reconstructible ;
- `tantivy/` : index Tantivy dérivé et reconstructible.

## Première interface de recherche

`mail-archive-app` peut démarrer sans argument. Il rouvre l’archive par défaut
ou récemment utilisée si elle est accessible ; sinon l’écran initial permet
`Ouvrir une archive…` ou `Créer une archive…`. Un dossier non vide est refusé
pour une nouvelle archive et un dossier invalide n’est pas créé implicitement.
La configuration légère est stockée dans `$XDG_CONFIG_HOME/memoria/config.json`
(ou le répertoire standard équivalent) ; elle ne contient ni messages ni
tokens. Les tokens OAuth restent dans un répertoire séparé.

`mail-archive-app` ouvre une archive existante hors ligne, réutilise l’index
Tantivy dérivé et affiche jusqu’à 50 résultats. Une recherche vide conserve un
état neutre ; la saisie est réactive. Cliquer une ligne, ou utiliser les
flèches puis Entrée, lit la frame RAW dans un thread de fond et affiche une
représentation texte dérivée du MIME. Le RAW n’est jamais remplacé par cette
représentation.

## Synchronisation depuis Memoria

La vue `Archive / Synchronisation` est accessible depuis le menu `Archive`.
Depuis cette vue, `Ajouter un compte Gmail…` demande explicitement un fichier
OAuth Desktop existant, puis réutilise le flux loopback du connecteur. Le scope
reste exclusivement `gmail.readonly`. La première synchronisation se lance
ensuite avec `Synchroniser maintenant` ; aucune autorisation ni synchronisation
n’est lancée au démarrage.

Le mode CLI reste disponible pour des exécutions reproductibles :

```text
cargo run -p mail-archive-experiment --bin mail-archive-app -- \
  --archive /chemin/archive \
  --credentials ~/.config/mail-archive/gmail-client.json \
  --token-dir ~/.config/mail-archive/tokens \
  # --account est facultatif ; il sert seulement de contrôle de compatibilité
```

Sans credentials, la consultation locale reste disponible et la vue Archive
indique que la source Gmail n’est pas configurée. L’action explicite
`Synchroniser maintenant` effectue la full sync, la réconciliation history ou
la reprise prévue par le connecteur existant, puis met à jour l’index Tantivy
dans un worker. Les RAW et le catalogue sont validés avant que l’interface
n’affiche l’index comme à jour. Une erreur d’indexation ne supprime pas les
RAW ; elle est affichée séparément.

Le projet ne fournit pas de client OAuth Google distribué : le développeur doit
créer un client **Desktop app** dans Google Cloud et sélectionner son fichier
JSON dans Memoria. Credentials et tokens restent hors de l’archive et hors Git.

Le chemin de l’archive vient de `--archive` ou de `MAIL_ARCHIVE_PATH`. La
commande `--benchmark` mesure seulement le contrôleur local (ouverture,
recherche, lecture/parsing) et n’ouvre pas de fenêtre.

Après la génération initiale, les deux indexeurs suivent le chemin
`archive → catalogue → lecture ciblée → parsing → index`. Le générateur n'est
pas utilisé pour reconstruire les messages pendant l'indexation.

Le seed, le nombre de messages et la politique de pièces jointes rendent le
corpus reproductible. Le générateur couvre plusieurs langues, comptes,
dossiers, conversations, distributions de correspondants, termes fréquents
et rares, formats texte/HTML et pièces jointes à tailles variables.

Trois profils sont disponibles : `light` (majoritairement textuel),
`personal` (hétérogène, newsletters, reçus et pièces jointes) et `heavy`
(beaucoup de pièces jointes et messages volumineux). `--duplicate-rate`
contrôle explicitement la probabilité de contenu partagé ; `--compression`
active les mesures gzip/zstd, coûteuses sur les gros profils.

## Dépendances expérimentales

- `rusqlite 0.40.2` avec `bundled` : SQLite est compilé localement et FTS5 est
  disponible sans serveur ni bibliothèque SQLite système ; la table utilise
  le tokenizer Unicode61 et des champs FTS séparés.
- `tantivy 0.26.1` : index inversé Rust segmenté, BM25 via le query parser,
  champ numérique de date et index reader séparé.
- `flate2 1.1` : baseline gzip des bytes bruts ; ce n’est pas une décision de
  compression de l’archive.
- `zstd 0.13` : mesure expérimentale au niveau message, également sans
  décision de format.
- `reqwest 0.12` avec `rustls-tls` : transport HTTPS minimal vers l’API Gmail.
- `mailparse 0.16.1` : parsing MIME dérivé; il ne remplace jamais la copie RAW.
- `base64 0.22`, `blake3 1.8` : décodage base64url et statistiques de hash en
  lecture seule, sans CAS de production.
- `dirs 6`, `url 2.5`, `webbrowser 1` : token dir configurable, callback OAuth
  et ouverture du navigateur desktop.

## Limites actuelles

- L’IMAP readonly multi-mailbox est disponible via le CLI expérimental, pas
  dans le parcours UI produit.
- Une queue de segment incomplète peut être inventoriée en lecture seule par
  A2.1. Aucune opération de troncature physique n’est exposée ; la réparation
  destructive est différée jusqu’au chantier crash-consistency/publication.
- `archive-inventory` réconcilie les frames physiques valides avec les lignes
  `messages`. La revendication minimale est `segment + offset`, puis la
  validation exige longueur, `doc_id` et BLAKE3. Une frame durable non publiée
  est orphan, même avec un `doc_id` réutilisé par une retry ; une frame
  revendiquée mais mal décrite est incohérente, y compris si `doc_id` ou
  `frame_bytes` est négatif dès lors que `segment + offset` est exploitable.
  Les records sans bytes sont
  comptés comme physiquement manquants. Magic, longueur incohérente et
  checksum invalide arrêtent le segment sans resynchronisation heuristique ;
  seule une queue terminale trop courte pour un header est incomplète. Le scan
  et `archive_summary` sont strictement read-only. Le rebuild Tantivy suit le
  catalogue autoritatif et ignore les orphans.
- `recovery-plan` produit un plan Tier A déterministe et explicable, sans
  réseau, lock writer, écriture SQLite, sidecar, adoption d'orphelin, relink
  ou troncature. Un plan ne vaut jamais autorisation d'exécuter l'action.

## Politique de recovery Tier A (R1)

Le RAW validé permet de conserver et d'exporter des octets byte-exacts, ainsi
que son `doc_id` encodé et son BLAKE3 calculé. Il ne permet pas, à lui seul,
d'inventer un compte, un Gmail ID, un thread, des labels ou une identité IMAP.

| État | Automatique | Source | Choix utilisateur | Salvage / local |
|---|---:|---:|---:|---|
| AvailableValidated | non nécessaire | non | non | aucune action |
| OrphanValidated | non | non | oui | oui, en place et byte-exact |
| PhysicallyMissing + identité durable | non | oui | oui | re-fetch exact seulement |
| PhysicallyMissing sans identité | non | non | oui | non, irrécupérable localement |
| CataloguedInconsistent | non | candidat seulement | oui | parfois, sans relink |
| PhysicalCorruption | non | non | oui | irrécupérable localement ; frames indépendantes séparées seulement |
| IncompleteTail | jamais en R1 | non | oui | cleanup candidate conditionnel seulement |
| CatalogueLost + RAW valides | non | à étudier | oui | oui, catalogue de salvage uniquement |

Une identité Gmail complète est `source_account + gmail_message_id` ; une
identité IMAP complète est `source_account + mailbox + UIDVALIDITY + UID`.
Un `Message-ID` MIME, un sujet, une date ou un expéditeur ne suffit pas pour
un re-fetch automatique. Les frontiers Gmail history et IMAP ne bougent
jamais pendant ce planner ni du seul fait d'un RAW retrouvé.

Une `CataloguedInconsistent` reste une contradiction entre sources : même
`doc_id` ailleurs, MIME similaire ou proximité physique ne sont pas des
preuves. Tantivy/FTS, MIME analysé, HTML et thumbnails sont des dérivés ; ils
peuvent aider à identifier un salvage mais ne peuvent ni réparer les octets
ni fournir une provenance Tier A.

Une `IncompleteTail` est seulement un candidat de nettoyage futur, jamais une
autorisation de tronquer. Une troncature future devra vérifier que la zone est
réellement terminale, qu'aucune revendication catalogue ne la concerne ou ne
la chevauche, qu'aucune frame valide ultérieure n'existe, que l'autorité
single-writer est détenue et que la destruction est explicitement demandée et
autorisée par la politique de recovery. R1 ne contient aucun code destructif.

Le schéma catalogue v1 impose des champs de provenance et d'identité `NOT
NULL`. Il ne peut donc pas représenter honnêtement un RAW récupéré dont la
source est perdue. Une future exécution devra choisir un modèle de salvage
séparé ou une évolution explicite du schéma ; elle ne doit pas remplir ces
champs avec `unknown`, une chaîne vide ou une heuristique.
- L’export EML individuel et batch est disponible et copie les octets RAW ; la
  restauration complète et la migration vers un fournisseur ne sont pas
  implémentées.
- Le benchmark compare les mêmes champs recherchables et stocke aussi les
  champs textuels côté Tantivy ; SQLite conserve en plus sa table de contenu
  FTS5 et ses attributs structurés. Les tailles ne sont donc comparables
  qu'avec cette ventilation explicite.
- Les résultats de taille ne sont pas une preuve à 300 Go : le corpus reste
  synthétique et le benchmark ne simule pas la fragmentation, les disques
  lents ou plusieurs millions de lignes.
- La déduplication n’est pas encore appliquée aux bytes de l’archive. Le
  corpus mesure seulement les hashes d’attachements répétés pour préparer une
  expérience coût/bénéfice séparée.
- Le mode « index chaud » indexe les 270 derniers messages, soit environ 90
  jours selon l’horloge synthétique. Il est reconstruit indépendamment et ne
  déplace aucun objet archivé.
- La campagne Gmail réelle dépend de credentials locaux et n’est pas exécutée
  automatiquement. Les fixtures vérifient pagination, RAW binaire, idempotence
  et repli après expiration de l’historique; les statistiques d’un compte réel
  doivent rester agrégées et anonymisées.
- `mailparse` fournit des vues MIME dérivées; les offsets MIME ne sont pas
  encore persistés. La fidélité byte-exacte repose sur les bytes RAW.

Le rapport détaillé de changement d'échelle est dans
[`experiments/2026-08-20-mail-archive-scale.md`](../../experiments/2026-08-20-mail-archive-scale.md).
Le rapport CAS et contrat de fidélité est dans
[`experiments/2026-08-20-mail-archive-cas.md`](../../experiments/2026-08-20-mail-archive-cas.md).
Le rapport du connecteur Gmail est dans
[`experiments/2026-08-20-mail-archive-gmail-readonly.md`](../../experiments/2026-08-20-mail-archive-gmail-readonly.md).
Le rapport de la campagne réelle 100 → 1 000 est dans
[`experiments/2026-08-20-mail-archive-gmail-real-1000.md`](../../experiments/2026-08-20-mail-archive-gmail-real-1000.md).
L’expérience MIME ciblée `has:attachment` est dans
[`experiments/2026-08-20-mail-archive-gmail-attachments.md`](../../experiments/2026-08-20-mail-archive-gmail-attachments.md).
La full sync réelle complète est documentée dans
[`experiments/2026-08-20-mail-archive-gmail-full-sync.md`](../../experiments/2026-08-20-mail-archive-gmail-full-sync.md).
