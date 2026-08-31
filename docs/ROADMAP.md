# Memoria — Roadmap

Version 0.3 — 31 août 2026

Cette roadmap est un document de priorisation. Elle ne remplace ni
[`ARCHITECTURE.md`](ARCHITECTURE.md), ni [`ASSURANCE.md`](ASSURANCE.md), ni
[`RECOVERY.md`](RECOVERY.md).

Principe :

> Les nouvelles fonctionnalités ne doivent pas élargir silencieusement
> l'autorité de Memoria. Les abstractions communes sont introduites seulement
> lorsqu'un besoin réel les justifie.

## 1. Horizons

```text
NOW    stabiliser le modèle documentaire et le modèle source/provenance
NEXT   terminer le recovery produit minimal et les imports locaux
THEN   automatiser la synchronisation et développer restauration/migration
LATER  optimiser stockage, enrichir extraction/recherche et élargir plateformes
```

## 2. Baseline actuelle

Socle fermé :

- RAW autoritatif et lecture validée ;
- exploitation non destructive d'archives partiellement endommagées ;
- publication identité/provenance ;
- RAW durable avant publication ;
- frontier Gmail cohérent ;
- inventaire physique/orphans ;
- single-writer ;
- R1 read-only ;
- R2.1 Gmail exact ;
- R2.1 IMAP exact ;
- R2.2a export byte-exact d'un orphan validé.

Produit actuel :

- application desktop locale ;
- Gmail read-only dans l'UI ;
- IMAP read-only expérimental en CLI ;
- recherche locale Tantivy ;
- EML byte-exact individuel/batch ;
- pièces jointes et previews dérivées ;
- rendu HTML sanitised via navigateur système ;
- Linux/KDE voie principale ;
- build Windows disponible mais validation native incomplète.

## 3. NOW — frontières d'autorité et provenance

### P0 — Refonte documentaire

**Objectif :** rendre explicites et séparées les autorités qui étaient
mélangées dans `ASSURANCE.md` et `ROADMAP.md`.

Livrables :

- `ARCHITECTURE.md` ;
- `SECURITY.md` ;
- `RECOVERY.md` ;
- `ASSURANCE.md` recentré ;
- `ROADMAP.md` compact ;
- `AGENTS.md` routant vers la bonne documentation.

**Exit criterion :** une modification de parsing, sécurité, recovery ou source
peut identifier son document d'autorité sans devoir lire toute l'histoire du
projet.

### P1 — Modèle conceptuel source/acquisition/provenance

**Objectif :** préparer Gmail, IMAP, EML, MBOX, Outlook, MailStore et futures
sources sans imposer une abstraction d'implémentation prématurée.

Le modèle doit distinguer :

```text
RAW physique
acquisition/module
instance de source
identité du record source
provenance directement attestée
provenance déclarée par une source intermédiaire
contenu MIME observé
dérivé
provenance inconnue/non prouvée
```

**Non-objectifs immédiats :**

- pas de nouveau trait Rust générique ;
- pas de schéma SQL définitif ;
- pas de blob JSON opaque non versionné ;
- pas de migration catalogue tant que les besoins ne sont pas stabilisés.

**Exit criterion :** M1 peut être spécifié sans traiter Gmail/IMAP comme les
formes fondamentales de toute provenance.

### P2 / M1 — Modèle persistant d'acquisition/provenance

**Objectif :** définir la représentation persistante nécessaire aux assertions
d'acquisition/provenance, y compris les RAW dont certaines dimensions de
provenance restent inconnues ou non démontrées.

`M1` est un chantier architectural, pas une action R2. La numérotation R2 reste
réservée aux actions de recovery bornées.

**Dépend de :** P1 + R2.2a + socle Tier A déjà fermé (publication A3,
inventaire/R1, single-writer).

**Contraintes :**

- provenance composée par assertion, jamais un unique niveau global du record ;
- l'inconnu sur une dimension ne doit pas dégrader une identité source déjà
  attestée ;
- état source mutable et frontiers restent des axes séparés ;
- persistance d'un RAW partiellement documenté ≠ publication source, adoption
  ou relink.

**Question principale :** évolution versionnée du catalogue, modèle auxiliaire
ou combinaison explicite des deux.

**Exit criterion :** format/migration, invariants, compatibilité et tests
d'interruption sont spécifiés avant implémentation.

## 4. NEXT — recovery produit minimal

### P3 — R2.2 extensions utiles

R2.2a est fermé.

Évaluer seulement les extensions qui apportent une vraie valeur :

- listing ;
- batch ;
- manifest agrégé ;
- intégration UI/CLI.

Elles ne bloquent pas P2 sauf nouvelle preuve nécessaire.

### P4 — R2.4 cleanup des tails

Action destructive explicite uniquement après preuve de terminalité, absence de
claim/chevauchement et absence de frame ultérieure valide.

### P5 — R2.5 relink catalogue

Traiter uniquement les contradictions pour lesquelles une preuve non ambiguë
peut être spécifiée.

### P6 — R2.6 catalogue perdu

Première cible :

> salvage fidèle des RAW prouvables, pas reconstruction fictive de provenance.

### P7 — Recovery UI

Présenter :

- état observé ;
- preuves ;
- action disponible ;
- destructivité ;
- résultat.

L'UI ne doit pas inventer une action que le backend de recovery n'autorise pas.

## 5. NEXT — sources locales et produit

### P8 — EML import

Définir d'abord son contrat d'acquisition :

- octets importés ;
- contexte d'import ;
- identité locale éventuellement conservée ;
- aucune promotion automatique de `Message-ID` en provenance.

### P9 — MBOX import

Traiter le MBOX comme un module d'acquisition avec framing et identité
provider-specific.

Le choix précis de format/record est à valider sur corpus réels.

### P10 — IMAP produit

Faire passer le support IMAP read-only expérimental vers une expérience produit
auditable :

- configuration ;
- multi-mailbox ;
- progression ;
- erreurs ;
- resync ;
- UI.

Le contrat Tier A existant ne doit pas être affaibli pour simplifier l'UX.

### P11 — Multi-comptes Gmail

Étendre l'UI et le scheduling en conservant l'identité de compte Tier A et
l'isolation des credentials.

## 6. THEN — synchronisation et restauration

### P12 — Synchronisation automatique/background

Seulement après stabilisation du workflow manuel.

Contraintes :

- single-writer ;
- frontiers ;
- cancellation ;
- reprise ;
- observabilité séparée Tier A / index Tier C.

### P13 — Sauvegarde et intégrité périodique

Réutiliser l'inventaire physique/R1 pour :

- vérification périodique ;
- sauvegarde incrémentale des segments append-only ;
- restauration depuis sauvegarde ;
- rapport d'état de santé.

Ne pas créer un second moteur de validation.

### P14 — Restauration complète

Reconstituer une archive exploitable à partir des données et preuves
réellement disponibles.

### P15 — Migration vers une source externe

Gmail → Gmail ou autre write-back exige un connecteur d'écriture distinct du
connecteur read-only.

Aucune extension de scopes ou de capacité d'écriture ne doit être implicite.

## 7. THEN — plateforme

### P16 — Windows natif

Valider sur machine réelle :

- UX ;
- HiDPI ;
- UI Automation/accessibilité ;
- dialogs ;
- OAuth ;
- IFilter ;
- packaging.

### P17 — Packaging

Installer/signature/distribution uniquement après stabilisation fonctionnelle et
sécurité correspondante.

## 8. LATER — recherche et stockage

### P18 — Extraction de pièces jointes

Étendre seulement à des formats justifiés par des corpus réels.

OCR, Office supplémentaires et nouveaux providers restent Tier B/C avec budgets
de ressources.

### P19 — Recherche enrichie

Filtres, navigation, ranking ou recherche sémantique restent dérivés et
reconstructibles.

### P20 — Stockage exact optimisé

Étudier :

- compression réversible ;
- déduplication exacte ;
- CAS reconstructible byte-exact ;
- réorganisation physique prouvée réversible.

### P21 — Mode dérivé / économie d'espace

Un futur choix de ne plus conserver le bitstream original constitue un
**changement explicite de garantie produit**.

Il doit être persisté et ne jamais être confondu avec une corruption
accidentelle.

## 9. Règles de priorité

Une tâche monte en priorité si elle :

- ferme une ambiguïté d'autorité ;
- empêche une perte/substitution silencieuse ;
- débloque plusieurs fonctionnalités produit ;
- répond à un problème observé sur corpus réel ;
- simplifie durablement le système sans affaiblir une garantie.

Une tâche baisse en priorité si elle :

- généralise une abstraction sans second cas réel ;
- optimise une représentation dérivée avant d'avoir mesuré le besoin ;
- ajoute de la sécurité contre une menace explicitement hors périmètre ;
- duplique une logique déjà portée par inventaire/R1 ;
- ajoute une infrastructure qui ne débloque pas le produit.

## 10. Critère de sortie de la phase recovery

Le chantier recovery cesse d'être la priorité principale lorsque :

- les RAW valides restent exploitables sous corruption partielle ;
- les manques/contradictions sont explicitement classés ;
- les re-fetch exacts disponibles sont sûrs ;
- les orphans peuvent être exportés et représentés honnêtement ;
- les actions destructrices nécessaires ont des contrats explicites ;
- une archive sans catalogue peut être salvagée de façon fidèle ;
- l'utilisateur peut comprendre l'état et les actions proposées.

À cette frontière, l'effort principal doit revenir aux usages produit plutôt
qu'à une sophistication illimitée du recovery.
