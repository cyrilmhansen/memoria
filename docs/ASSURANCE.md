# Memoria — Politique de criticité et d'assurance

Version 0.3 — 31 août 2026

Ce document définit la criticité des responsabilités Memoria et les invariants
de conservation/fidélité.

Il ne définit pas le threat model de sécurité, qui appartient à
[`SECURITY.md`](SECURITY.md), ni le détail des actions de recovery, qui
appartient à [`RECOVERY.md`](RECOVERY.md).

## 1. Objet

Memoria distingue les données de référence irremplaçables des données calculées
ou reconstruisibles.

Le niveau de contrôle doit dépendre des conséquences réelles d'un défaut.
L'objectif n'est pas d'appliquer uniformément le niveau maximal d'assurance,
mais de rendre particulièrement difficiles les pertes, substitutions ou
corruptions silencieuses de données de référence.

Cette politique définit des responsabilités et des invariants. Elle ne prescrit
ni le nombre de crates, ni le découpage physique des fichiers, ni une
architecture d'implémentation particulière.

## 2. Niveaux de criticité

### Tier A — Conservation et fidélité

Relève du Tier A toute responsabilité qui détermine ce qui constitue la
référence conservée ou permet de la créer, retrouver, modifier, supprimer,
réparer ou exporter fidèlement.

Cela comprend notamment :

- stockage RAW ;
- framing et coordonnées physiques ;
- identités et provenance durables ;
- acquisition qui publie une référence locale ;
- publication catalogue Tier A ;
- single-writer ;
- recovery ;
- intégrité et validation autoritative ;
- exports byte-exacts.

Invariant principal :

> Une erreur de Memoria ne doit pas provoquer silencieusement la perte,
> l'altération ou la substitution d'une donnée de référence.

Une coordonnée valide mais incorrecte ne doit pas pouvoir substituer
silencieusement un autre record.

### Tier B — Interprétation de données non fiables

Relève du Tier B le code qui interprète du contenu extérieur sans autorité sur
la référence :

- MIME ;
- HTML ;
- MDN/DSN ;
- pièces jointes ;
- extraction de texte ;
- ICS/vCard ;
- futurs parseurs et analyseurs.

Invariant principal :

> Une analyse peut échouer ; elle ne doit ni altérer la référence ni empêcher
> l'exploitation des autres données conservées.

Une défaillance Tier B doit produire une analyse absente, partielle ou
explicitement invalide plutôt qu'une défaillance Tier A.

### Tier C — Dérivé et présentation

Relève du Tier C ce qui est reconstructible ou ne possède aucune autorité sur
les données archivées :

- Tantivy ;
- caches ;
- ranking ;
- vues calculées ;
- previews ;
- UI et présentation.

Invariant principal :

> La perte ou corruption complète du Tier C ne doit pas entraîner la perte
> d'une donnée de référence.

Un échec Tier C ne doit pas être confondu avec l'échec d'une opération Tier A
déjà durablement réussie.

La classification porte sur les responsabilités, pas nécessairement sur les
fichiers ou crates.

## 3. Modèle d'autorité

Le modèle conceptuel complet est défini dans
[`ARCHITECTURE.md`](ARCHITECTURE.md).

Les catégories d'autorité à préserver comprennent au minimum :

- identité physique RAW ;
- identité source durable lorsqu'elle existe ;
- provenance durable ;
- état source mutable observé ;
- état/frontier de synchronisation ;
- contenu MIME observé ;
- métadonnées dérivées.

Leur coexistence dans SQLite ne leur donne pas la même autorité.

Une identité ou provenance ne doit pas être reconstruite à partir d'un champ de
plus faible autorité sans contrat explicite.

## 4. Unité de conservation

L'unité logique fondamentale est le record RAW individuel identifié, non son
segment physique.

Un segment est un conteneur.

La corruption ou disparition d'une unité physique ne doit pas rendre
arbitrairement illisibles les autres records indépendants encore récupérables.

Cible durable :

> Une archive doit rester exploitable à partir de tout sous-ensemble de records
> de référence encore valides.

Les références manquantes doivent être représentées comme manquantes ou
contradictoires, pas transformer automatiquement le reste de l'archive en
archive invalide.

## 5. Modèle de panne Tier A

Les garanties Tier A sont définies par rapport à plusieurs classes de panne.

### 5.1 Crash du processus

Une opération partiellement publiée doit conduire à un état :

- cohérent ;
- récupérable ;
- réconciliable ;
- ou explicitement incohérent.

Elle ne doit pas être silencieusement considérée comme terminée.

### 5.2 Coupure brutale / perte de buffers

Une opération logiquement terminée en mémoire n'est pas nécessairement durable.

Les chemins Tier A doivent définir quand sont nécessaires :

- synchronisation du fichier ;
- synchronisation SQLite ;
- synchronisation du répertoire ;
- barrière de publication équivalente.

### 5.3 Corruption locale

Une frame, un segment ou une entrée catalogue peut devenir incohérent.

Une lecture autoritative doit vérifier que l'identité, les dimensions et les
preuves réellement lues correspondent à celles attendues avant de retourner un
RAW comme autoritatif.

### 5.4 Catalogue perdu ou corrompu

La perte de données dérivées ne doit pas être confondue avec la perte des RAW.

Lorsque la reconstruction historique complète n'est pas possible, Memoria doit
au minimum permettre l'exploitation fidèle des données de référence encore
directement prouvables.

### 5.5 Writers concurrents

Le contrat actuel est :

> **single-writer enforced; multiwriter deliberately unsupported**

Une seule autorité Tier A peut modifier une archive à la fois. Cette propriété
doit être imposée par le runtime et testée, pas seulement supposée.

## 6. Publication Tier A

Une acquisition réussie implique conceptuellement :

```text
RAW reçu
→ RAW durable
→ localisation catalogue durable
→ identité/provenance durable
→ frontier/état de synchronisation publié
```

Une interruption à toute frontière doit produire un état récupérable,
réconciliable ou explicitement incohérent.

Memoria ne doit pas marquer durablement une occurrence source comme conservée
si son RAW autoritatif n'est pas durablement disponible.

Principe conservateur :

> un RAW orphelin est préférable à une provenance affirmant à tort qu'une
> donnée inexistante a été conservée.

Les détails provider-specific sont décrits dans `RECOVERY.md` lorsqu'ils
concernent une réparation.

## 7. Frontières entre tiers

Tier B et Tier C ne devraient normalement pas posséder de primitives générales
de modification ou suppression du RAW.

Ils reçoivent des données en lecture et produisent des résultats dérivés.

Conséquences :

- une analyse MIME Tier B ne doit pas empêcher la conservation d'un RAW Tier A
  déjà reçu ;
- un échec Tantivy Tier C ne doit pas transformer une synchronisation Tier A
  déjà durablement réussie en échec indistinct ;
- un index ou rendu dérivé ne peut pas justifier une réparation Tier A ;
- l'ouverture d'une représentation dérivée ne doit pas muter implicitement
  l'autorité sauf responsabilité explicitement assumée et auditée.

## 8. Assurance Tier A

Tier A reçoit le niveau maximal raisonnablement applicable au risque.

Contrôles attendus lorsque pertinents :

- validation systématique des identités, longueurs, coordonnées et digests ;
- opérations bornées avant allocation à partir de données persistées ;
- publication ordonnée et testable ;
- tests d'interruption aux frontières de publication ;
- tests de corruption centrale et de perte partielle ;
- tests d'archive amputée et de reconstruction ;
- tests du contrat single-writer ;
- fuzzing du framing et du recovery ;
- audit explicite des opérations destructrices ;
- audit des usages `unsafe` dans les chemins d'autorité ;
- vigilance particulière sur `panic`, `unwrap` et hypothèses non vérifiées
  dépendant de contenu persistant.

Une garantie n'est considérée fermée que lorsque son périmètre et ses tests
sont identifiables.

## 9. Assurance Tier B

Les entrées Tier B sont potentiellement malformées ou hostiles.

Les limites doivent être imposées aussi tôt que possible avant les allocations
ou décodages coûteux.

Le contrôle de ressources peut comprendre :

- taille d'entrée ;
- taille cumulée décodée ;
- nombre de parties ;
- profondeur ;
- nombre d'objets ;
- taille des résultats ;
- nombre de processus/providers externes simultanés.

Les traitements coopératifs peuvent utiliser des budgets internes. Les
processus externes supervisables peuvent utiliser des timeouts.

Une API d'analyse pure ne doit pas provoquer implicitement une interaction
externe significative si son contrat ne l'annonce pas.

Les propriétés de sécurité supplémentaires sont définies dans `SECURITY.md`.

## 10. Assurance Tier C

Tier C reste soumis au socle qualité normal :

- compilation propre ;
- Clippy standard ;
- tests fonctionnels pertinents ;
- reconstruction vérifiée lorsque cette propriété est importante.

Les groupes `pedantic` et `nursery` peuvent être des outils d'audit mais ne
créent pas automatiquement une dette.

Une modification purement destinée à satisfaire un lint ne doit pas introduire
plus de complexité que le problème corrigé.

## 11. Politique de lints

Tant que plusieurs tiers coexistent dans une même cible Rust, le socle commun
est :

```text
rustc sans warnings
Clippy standard sans warnings
```

Les exigences Tier A/B supplémentaires sont principalement assurées par les
tests, audits, fuzzing, règles architecturales et contrôles ciblés.

Une séparation en crates ne doit pas être créée uniquement pour différencier
les lints.

## 12. Export fidèle

Un export EML fidèle signifie :

> lorsque Memoria annonce le succès, les octets exportés correspondent
> exactement au RAW autoritatif sélectionné.

Sont des propriétés distinctes qui doivent être spécifiées séparément :

- atomicité du fichier destination ;
- export d'une archive complète ;
- export de provenance ;
- manifest de salvage ;
- migration vers une source externe.

## 13. Intégrité accidentelle et sécurité

Le modèle actuel vise d'abord la détection de corruption accidentelle et la
fidélité de la conservation.

Checksums et digests ne constituent pas une garantie adversariale complète
contre un attaquant local capable de modifier archive, catalogue et runtime.

Le threat model correspondant est défini dans `SECURITY.md`.

## 14. Expérience et produit

Le statut expérimental appartient à une responsabilité ou un composant
identifiable, pas à un nom historique de crate.

Une expérience démontre des faits ou une faisabilité. Son implémentation n'est
pas automatiquement appropriée au produit.

Lors du passage experiment → product, réexaminer notamment :

- cycle de vie des ressources ;
- allocations volontairement non libérées ;
- panic/assertions ;
- erreurs ignorées ;
- simplifications de protocole ;
- opérations destructrices ;
- interactions externes ;
- limites justifiées uniquement par un petit corpus.

Les conclusions validées d'une expérience sont plus autoritatives que ses
raccourcis d'implémentation.

## 15. Connaissance pérenne

Doivent être promus dans la documentation durable lorsqu'ils ont une conséquence
future :

- invariants ;
- frontières d'autorité ;
- garanties de panne ;
- décisions architecturales ;
- propriétés réellement démontrées ;
- limitations structurantes.

Les détails locaux facilement redérivables restent dans le code, les tests,
`experiments/` ou `WORKLOG.md`.

## 16. Socle fermé actuel

Le détail des actions R1/R2 et leurs contrats se trouve dans `RECOVERY.md`.

Les garanties actuellement fermées comprennent notamment :

- A1 — lecture autoritative RAW ;
- A2 — exploitation non destructive d'une archive partiellement endommagée ;
- A3.1 — publication cohérente identité/provenance catalogue ;
- A3.2 — RAW durable avant publication ;
- A3.3 — frontier Gmail n'avançant pas au-delà de l'état appliqué ;
- inventaire physique et classification des orphans/contradictions ;
- single-writer ;
- R1 read-only ;
- R2.1a Gmail exact ;
- R2.1b IMAP exact ;
- R2.2a export byte-exact d'un `OrphanValidated`.

Cette liste décrit des périmètres fermés, pas une affirmation que tout le Tier A
est complet.

## 17. Questions encore ouvertes

Restent notamment ouverts :

- durabilité namespace/power-loss au-delà des frontières déjà prouvées ;
- fuzzing étendu ;
- recovery catalogue perdu ;
- cleanup destructif des tails ;
- relink des contradictions ;
- modèle persistant d'acquisition/provenance pour les sources futures et le
  salvage ;
- politique de sauvegarde/restauration complète.

Ces questions sont priorisées dans `ROADMAP.md`, pas spécifiées ici comme si
leur solution était déjà choisie.
