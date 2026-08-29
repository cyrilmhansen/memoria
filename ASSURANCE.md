# Memoria — Politique de criticité et d’assurance du code

Version 0.2 — 25 août 2026

## 1. Objet

Memoria distingue les données de référence irremplaçables des données calculées ou reconstruisibles.

Le niveau de contrôle du code doit dépendre des conséquences réelles d’un défaut. L’objectif n’est pas d’appliquer uniformément le niveau maximal d’assurance à tout le logiciel, mais de rendre particulièrement difficiles les pertes, substitutions ou corruptions silencieuses des données de référence.

Cette politique définit des responsabilités et des invariants. Elle ne prescrit ni le nombre de crates, ni le découpage physique des fichiers, ni une architecture d’implémentation particulière.

## 2. Niveaux de criticité

### Tier A — Conservation et fidélité

Relève du Tier A toute responsabilité pouvant déterminer ce qui constitue la référence conservée ou permettant de la créer, retrouver, modifier, supprimer ou exporter.

Cela comprend notamment le stockage RAW, les coordonnées des records, la provenance durable, l’acquisition Gmail/IMAP, les identités source, les opérations de recovery, l’intégrité du stockage et les exports fidèles.

Invariant principal :

Une erreur de Memoria ne doit pas provoquer silencieusement la perte, l’altération ou la substitution d’une donnée de référence.

Une opération Tier A doit notamment empêcher qu’une coordonnée valide mais incorrecte puisse substituer silencieusement un autre record.

### Tier B — Analyse de données non fiables

Relève du Tier B le code interprétant du contenu extérieur sans autorité sur la référence : MIME, HTML, MDN/DSN, pièces jointes, extraction de texte, ICS/vCard et futurs analyseurs.

Invariant principal :

Une analyse peut échouer ; elle ne doit ni altérer la référence ni empêcher l’exploitation des autres données conservées.

Une défaillance Tier B doit produire une analyse absente, partielle ou explicitement invalide plutôt qu’une défaillance Tier A.

### Tier C — Dérivé et présentation

Relève du Tier C ce qui est reconstruisible ou ne possède aucune autorité sur les données archivées : Tantivy, caches, ranking, vues calculées, UI et présentation.

Invariant principal :

La perte ou la corruption complète du Tier C ne doit pas entraîner la perte d’une donnée de référence.

Un échec Tier C ne doit pas être confondu avec l’échec d’une opération Tier A déjà réussie.

## 3. Autorité des métadonnées

Toutes les métadonnées associées aux sources ne possèdent pas la même autorité.

Memoria distingue au minimum :

**Identité source durable.** Données permettant d’identifier une occurrence ou un message dans sa source : identifiants Gmail, UID/UIDVALIDITY IMAP, identité du compte et de la mailbox lorsque nécessaire. Leur perte peut rendre impossible une synchronisation fidèle ou provoquer des doublons.

**Provenance durable.** Information établissant d’où provient une donnée de référence et permettant éventuellement une restauration ou une corrélation future.

**État source mutable observé.** Labels, flags et autres propriétés susceptibles de changer dans la source. Ils peuvent être importants sans constituer nécessairement une référence immuable.

**État de synchronisation.** Curseurs, history IDs, frontières UID et informations similaires. Ils permettent une synchronisation efficace mais ne doivent pas être confondus avec l’identité de la référence.

**Métadonnées dérivées ou de navigation.** Données reconstructibles ou destinées essentiellement à l’affichage, la recherche ou l’ergonomie.

Le schéma physique SQLite peut contenir plusieurs de ces catégories. Leur coexistence dans une même base ne leur donne pas la même autorité.

## 4. Unité de conservation

L’unité logique fondamentale de conservation est le record RAW individuel identifié, et non son segment physique.

Un segment est un conteneur.

La corruption ou disparition d’une unité physique ne doit donc pas rendre arbitrairement illisibles les records indépendants encore récupérables.

Le système doit tendre vers la propriété suivante :

Une archive Memoria doit rester exploitable à partir d’un sous-ensemble arbitraire de records de référence encore valides.

Les références manquantes doivent être représentées comme manquantes, et non transformer automatiquement le reste de l’archive en archive invalide.

## 5. Modèle de panne Tier A

### Politique R1 de recovery

Le recovery Tier A commence par un plan de preuves en lecture seule. Les
Les inventaires physique/catalogue fournissent l'état de conservation locale;
les identités source durables peuvent fournir une preuve additionnelle pour
classifier une possibilité de re-fetch. Une frame valide non publiée est
`OrphanValidated` et
reste un salvage, jamais une ligne `messages`. Une contradiction catalogue /
RAW est `CataloguedInconsistent` et ne peut pas être relinkée par `doc_id`,
MIME, proximité ou index dérivé. Les octets corrompus ne sont pas une source
de reconstruction.

Une absence physique est `RecoverableWithSource` seulement lorsqu'une
identité durable Gmail (`source_account + gmail_message_id`) ou IMAP
(`source_account + mailbox + UIDVALIDITY + UID`) est réellement présente.
Cette classe autorise un futur choix explicite de re-fetch ; elle n'autorise
pas le réseau ni l'avance d'un frontier dans le planner R1. Sans identité,
l'absence est `UnrecoverableLocally`. Une queue terminale incomplète est un
candidat de nettoyage futur, jamais une troncature R1. Cette disposition
signifie seulement qu'une identité suffisamment forte permet de tenter un
futur re-fetch; elle ne garantit ni la disponibilité actuelle de Gmail/IMAP,
ni l'identité des octets retournés avec le RAW historique perdu.

## R2.1a — re-fetch Gmail exact

R2.1a est fermé pour son périmètre : un seul `doc_id` peut être réparé si et
seulement si le record est `PhysicallyMissing`, sans contradiction
physique/catalogue, avec une identité Gmail durable unique et cohérente,
`source_state == present`, un compte OAuth authentifié correspondant à
`source_account`, un Gmail message ID retourné égal à l'ID demandé et un BLAKE3
du RAW re-fetché égal au `raw_blake3` historique.

Pour Gmail, `source_account` est la clé locale opaque
`gmail:<BLAKE3(email Gmail canonique)>`, dérivée du profile OAuth authentifié ;
l'adresse brute n'est jamais l'identité persistée. Aucune réparation ne peut
utiliser Message-ID MIME, sujet, date, expéditeur, thread, `doc_id` seul,
proximité physique ou index dérivé.

L'ordre garanti est :

```text
authority-only → réconciliation locale → identité canonique
→ profile OAuth → fetch Gmail exact → returned ID → décodage RAW
→ BLAKE3 → fresh segment non revendiqué → append
→ durable barrier → CAS catalogue
```

Avant validation complète du compte, de l'ID et du digest, aucune mutation Tier
A de l'archive n'a lieu. La destination est physiquement absente, non
revendiquée par le catalogue et créée par `create_new` sous single-writer ; la
localisation manquante n'est jamais recréée. Un échec CAS après append durable
retourne `RecoveryConflict`, conserve l'ancienne ligne catalogue et laisse la
nouvelle frame `OrphanValidated`.

R2.1a ne modifie ni `gmail_state`/frontier, ni `source_state`, labels ou thread
metadata : le recovery n'est pas une synchronisation. Un contenu Gmail dont le
BLAKE3 diffère produit `SourceContentChanged` et ne remplace jamais
silencieusement le RAW historique.

`source_account` Gmail n'est pas une adresse affichée : c'est l'identité locale
opaque `gmail:<BLAKE3(email canonique)>`, produite par le helper partagé entre
l'ingestion, la CLI et le recovery. Une valeur `--account` éventuelle n'est
qu'une assertion sur l'adresse du profil et n'est jamais persistée comme
identité. Avant validation complète de l'état local,
du compte authentifié, de l'identité Gmail distante et du digest historique,
R2.1a ne crée ni segment RAW ni publication catalogue. Après append durable,
un échec du CAS catalogue peut volontairement laisser une nouvelle frame
`OrphanValidated`; c'est le mode de panne sûr hérité d'A3.2.

Une zone `PhysicalCorruption` n'est pas un salvage : sans payload et digest
validés, elle est irrécupérable localement, ou unsafe si elle contredit une
revendication catalogue. Seules les frames indépendantes validées peuvent
être salvagées.

`IncompleteTail` ne donne pas une autorisation de destruction. Une future
troncature ne pourra être envisagée qu'après démonstration simultanée que la
zone est réellement terminale, qu'aucune revendication catalogue ne la
concerne ou ne la chevauche, qu'aucune frame valide ultérieure n'existe, que
l'autorité single-writer est détenue pendant l'opération et que la destruction
est explicitement demandée et autorisée par la politique de recovery. R1 ne
tronque jamais.

`recovery-plan` n'acquiert pas l'autorité single-writer, n'écrit ni SQLite ni
RAW, ne crée pas de sidecar et n'avance aucun état source. Les index, MIME
analysé, HTML et thumbnails sont Tier B/C et ne peuvent justifier une
réparation Tier A.

Les garanties Tier A doivent être définies par rapport à plusieurs classes de panne distinctes.

### Crash du processus

Memoria doit revenir dans un état cohérent ou détectablement incohérent. Une opération partiellement publiée ne doit pas être silencieusement considérée comme terminée.

### Coupure brutale / perte de buffers

La politique de durabilité doit préciser quelles opérations nécessitent effectivement une synchronisation du fichier, du catalogue et, lorsque nécessaire, du répertoire.

Il ne suffit pas qu’une opération soit logiquement terminée en mémoire.

### Corruption locale

Une frame, un segment ou une entrée de catalogue peut devenir incohérent.

La lecture Tier A doit vérifier que l’identité et les dimensions réellement lues correspondent à celles attendues avant de retourner une donnée comme autoritative.

### Catalogue perdu ou corrompu

La perte d’un index ou d’une information dérivée ne doit pas être confondue avec la perte des RAW.

Lorsque la reconstruction complète n’est pas possible, Memoria doit au minimum permettre l’exploitation des données de référence encore directement identifiables.

### Writers concurrents

Le comportement multi-writer doit être explicitement supporté ou explicitement interdit.

Un contrat « un seul writer par archive » est acceptable s’il est effectivement imposé plutôt que simplement supposé.

## 6. Publication Tier A

Une acquisition réussie implique plusieurs états potentiellement distincts :

RAW reçu
→ RAW durable
→ localisation catalogue durable
→ identité/provenance durable
→ frontière de synchronisation publiée

La publication doit être conçue de sorte qu’une interruption à n’importe quelle frontière produise soit :

un état récupérable ;
un état réconciliable ;
ou une incohérence explicitement détectable.

En particulier, Memoria ne doit pas pouvoir marquer durablement une identité source comme importée alors que son RAW autoritatif n’est pas durablement disponible.

Un RAW orphelin est généralement préférable à une provenance affirmant à tort qu’une donnée inexistante a été conservée.

## 7. Frontières entre tiers

Les responsabilités Tier B et C ne devraient normalement pas posséder de primitives générales de modification ou suppression des RAW.

Elles reçoivent des données en lecture seule et produisent des résultats dérivés.

Une fonctionnalité peut traverser plusieurs tiers. Dans ce cas, l’ordre des opérations doit préserver les frontières.

Ainsi :

- une analyse MIME Tier B ne devrait pas empêcher la conservation d’un RAW Tier A déjà reçu ;
- un échec Tantivy Tier C ne devrait pas transformer une synchronisation Tier A déjà durablement réussie en échec indistinct ;
- l’ouverture d’un index Tier C ne devrait pas modifier implicitement un catalogue Tier A sauf si cette responsabilité est explicitement assumée et auditée.

La classification porte sur les responsabilités, pas nécessairement sur les fichiers.

## 8. Assurance Tier A

Tier A reçoit le niveau de contrôle maximal raisonnablement applicable.

Le plancher commun Rust/Clippy peut être partagé avec le reste du produit, mais Tier A ajoute des contrôles spécifiques :

- validation systématique des identités, longueurs et coordonnées lors des lectures autoritatives ;
- opérations bornées avant allocation à partir de données stockées ;
- tests d’interruption aux frontières de publication ;
- tests de corruption centrale et de perte partielle ;
- tests de reconstruction et d’archive amputée ;
- vérification du contrat multi-writer ;
- fuzzing du framing et des mécanismes de recovery lorsque pertinent ;
- audit explicite des opérations destructrices et des usages unsafe.

Les panic, unwrap et hypothèses non vérifiées dans les chemins dépendant du contenu persistent doivent être particulièrement surveillés.

## 9. Assurance Tier B

Les entrées Tier B sont considérées comme potentiellement malformées ou hostiles.

Les limites doivent être imposées aussi tôt que possible, idéalement avant les allocations ou décodages coûteux.

Le contrôle des ressources peut comprendre :

- taille d’entrée ;
- taille cumulée décodée ;
- nombre de parties ;
- profondeur ;
- nombre d’objets DOM ou équivalent ;
- taille des résultats ;
- nombre de processus ou providers externes simultanés.

Une « limite de temps » n’est pas exigée universellement d’un parseur in-process lorsqu’il n’existe aucun mécanisme sûr d’interruption.

Les traitements coopératifs peuvent avoir des budgets internes. Les processus externes supervisables peuvent avoir des timeouts et être tués proprement.

Une API d’analyse pure ne doit pas provoquer implicitement une interaction externe significative si son contrat ne l’annonce pas.

## 10. Assurance Tier C

Tier C reste soumis au socle normal de qualité du projet :

- compilation propre ;
- Clippy standard ;
- tests fonctionnels pertinents ;
- capacité de reconstruction vérifiée lorsque cela constitue une propriété importante.

Les groupes pedantic et nursery peuvent être utilisés comme outils d’audit, mais ne constituent pas une dette automatiquement exigible.

Une modification purement destinée à satisfaire un lint ne doit pas introduire une abstraction ou une complexité supérieure au problème qu’elle corrige.

## 11. Politique de lints

Les tiers ne suivent pas nécessairement les frontières des crates.

Tant que plusieurs tiers coexistent dans une même cible Rust, la cible reçoit un socle commun :

rustc sans warnings
Clippy standard sans warnings

Les exigences spécifiques Tier A et B sont ensuite principalement assurées par des tests, audits, fuzzing, règles architecturales et contrôles ciblés.

Une future séparation physique peut permettre des politiques de lint différentes, mais elle ne doit pas être effectuée uniquement pour permettre cette différenciation.

## 12. Export fidèle

Un export EML fidèle signifie au minimum :

Lorsque Memoria annonce qu’un message a été exporté avec succès, les octets du fichier exporté correspondent exactement au RAW autoritatif correspondant.

L’atomicité de la création du fichier destination, l’export d’une archive complète et l’export de métadonnées/provenance sont des propriétés distinctes qui doivent être spécifiées séparément si elles deviennent des fonctionnalités produit.

## 13. Intégrité

Le modèle actuel vise d’abord la détection de corruptions accidentelles.

Un checksum non cryptographique ne constitue pas une protection contre un adversaire local capable de modifier intentionnellement archive et catalogue.

Si Memoria doit ultérieurement fournir une garantie d’intégrité adversariale, elle devra être spécifiée séparément plutôt que déduite des mécanismes actuels de checksum.

## 14. Expérience et produit

Le statut « expérimental » appartient à une responsabilité ou un composant identifiable, pas simplement au nom historique d’un crate ou d’un binaire.

Une expérience démontre des faits ou la faisabilité d’une approche. Son implémentation n’est pas automatiquement appropriée pour le produit.

Lors du passage experiment → product, doivent notamment être réexaminés :

- cycle de vie des ressources ;
- allocations volontairement non libérées ;
- panic et assertions ;
- erreurs ignorées ;
- simplifications de protocole ;
- opérations destructrices ;
- interactions externes ;
- limites justifiées uniquement par la petite taille du corpus expérimental.

Les conclusions validées d’une expérience sont plus autoritatives que ses raccourcis d’implémentation.

## 15. Connaissance pérenne

Les agents ne produisent pas automatiquement une connaissance durable du système en écrivant du code.

Doivent être promus dans la connaissance projet lorsqu’ils sont durables :

- invariants ;
- frontières d’autorité ;
- garanties de panne ;
- décisions architecturales ;
- propriétés réellement démontrées ;
- limitations ayant une conséquence future.

Les détails locaux et décisions facilement redérivables ne doivent pas recevoir le même statut.

## 16. État actuel

Cette politique décrit une cible d’assurance, pas une affirmation selon laquelle le code actuel la respecte déjà.

L’audit du 25 août 2026 a notamment identifié des écarts Tier A concernant :

- validation de l’identité du record lors de la lecture ;
- bornage des coordonnées et longueurs ;
- crash-consistency entre RAW, catalogue et provenance ;
- recovery après corruption centrale ;
- concurrence des writers ;
- exploitation d’une archive partiellement amputée.

Ces éléments doivent être traités comme des défauts ou dettes Tier A distincts et priorisés séparément, sans refactoring global automatique.
