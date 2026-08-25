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
