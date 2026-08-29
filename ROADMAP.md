# Memoria — Roadmap intégrée

Ce document maintient une vue unique des évolutions de Memoria selon trois axes qui ne doivent plus être suivis séparément :

1. **Produit / utilisateur** — ce que l’utilisateur pourra faire ;
2. **Assurance / intégrité** — les garanties nécessaires pour que ces fonctions restent sûres ;
3. **Dépendances / impact cumulatif** — quelles briques rendent les suivantes possibles et quelles garanties seraient affectées par une évolution.

La roadmap n’est pas une promesse de livraison. Les priorités peuvent évoluer selon les résultats des expériences, les contraintes plateforme et les besoins observés sur des corpus réels.

Les décisions d’assurance détaillées restent dans [`ASSURANCE.md`](ASSURANCE.md), les faits durables dans [`KNOWLEDGE.md`](KNOWLEDGE.md), et l’historique de travail dans [`WORKLOG.md`](WORKLOG.md).

## 1. Principes directeurs

Le modèle actuel reste volontairement conservateur :

- le RAW MIME original est l’autorité byte-exacte locale tant qu’une politique utilisateur différente n’a pas été explicitement choisie ;
- SQLite contient un mélange de coordonnées/identités Tier A, d’état source mutable et de métadonnées de navigation ;
- Tantivy, FTS, MIME analysé, HTML, thumbnails et vues UI sont dérivés ;
- Gmail est actuellement une source read-only ;
- une seule autorité d’écriture Tier A peut modifier une archive locale à la fois ;
- le recovery doit être fondé sur des preuves explicites et ne doit jamais convertir une ambiguïté en autorité.

Une évolution fonctionnelle qui modifie ces principes doit être traitée comme une évolution du contrat d’autorité, pas comme une simple optimisation interne.

## 2. Socle actuellement fermé

| Bloc | État | Garantie cumulative | Débloque |
|---|---|---|---|
| A1 — lecture autoritative RAW | Fermé | Les coordonnées, l’identité de frame, le framing, FNV et BLAKE3 sont validés avant de retourner un RAW comme autoritatif | Recovery, export fidèle, rebuild sûr |
| A2 — archive partiellement endommagée | Fermé pour l’exploitation non destructive | Une corruption locale n’invalide pas arbitrairement les autres records encore valides | Inventaire, salvage, rebuild partiel |
| A3.1 — identité/provenance atomique | Fermé | Les métadonnées source durables sont publiées avec le catalogue selon une transaction cohérente | Recovery assisté par source |
| A3.2 — RAW durable avant publication | Fermé | Un RAW doit franchir sa barrière durable avant que SQLite puisse le publier ; les APIs mutantes sont scellées | Crash-consistency logique, orphans détectables |
| A3.3 — frontier Gmail | Fermé | Un frontier n’avance pas au-delà de l’état réellement appliqué localement | Re-sync et recovery sans histoire source inventée |
| Détection RAW physique | Fermé | Frames cataloguées, orphans, contradictions, corruption, missing et tail sont distingués sans mutation | Recovery R1 |
| Single-writer | Fermé | Une seule autorité Tier A existe par archive locale ; reset/création sont couverts | Toute future réparation destructive |
| Recovery R1 | Fermé | `recovery-plan` classe les preuves en lecture seule sans réseau ni mutation | Recovery R2 action par action |
| R2.1a — Gmail exact re-fetch | Fermé | Un RAW Gmail physiquement manquant n’est re-fetché et republié que si l’identité source et le BLAKE3 historique sont exactement validés | R2.1b IMAP |
| R2.1b — IMAP exact re-fetch | Fermé | Un RAW IMAP manquant exige mailbox, UIDVALIDITY, UID, fetch exact et digest historique validés | R2.2 salvage/export |
| R2.1 — re-fetch assisté par source | Fermé pour Gmail + IMAP | Gmail et IMAP partagent une publication A3.2/CAS exacte, avec destination fraîche et orphan sûr en cas de conflit | R2.2 salvage/export |

Les checkpoints récents correspondants incluent notamment :

```text
8046fa2  fix(mail-archive): detect orphan raw frames safely
83d546c  fix(mail-archive): enforce single-writer archive authority
2693e40  feat(mail-archive): add read-only recovery planning
f8c61a1  feat(mail-archive): recover missing gmail raw exactly
429d167  docs(mail-archive): close gmail recovery R2.1a
```

## 3. Dépendances principales

```text
A1 lecture autoritative
        │
        ▼
A2 exploitation partielle
        │
        ├───────────────┐
        ▼               ▼
A3 publication      scan physique RAW
        │               │
        └───────┬───────┘
                ▼
          single-writer
                │
                ▼
          Recovery R1
                │
       ┌────────┼────────┐
       ▼        ▼        ▼
     R2.1      R2.2     R2.4
    re-fetch  salvage   cleanup
       │        │        │
       │        ▼        │
       │      R2.3       │
       │  modèle salvage │
       └────────┼────────┘
                ▼
       recovery complexe
       catalogue perdu /
       incohérences fortes
```

La durabilité namespace/power-loss et le fuzzing renforcent cette chaîne mais ne bloquent pas les premières étapes de R2.

## 4. Priorité actuelle — Recovery R2

R2 ne doit pas devenir une commande générale `recover --force`. Chaque action possède ses propres preuves, son niveau de destructivité et son audit.

### R2.1 — Re-fetch assisté par source

#### R2.1a Gmail — Fermé

Le re-fetch Gmail exact est fermé pour le périmètre d’un `doc_id` explicite.
L’identité source, le compte OAuth, l’ID Gmail et le BLAKE3 historique doivent
être validés avant publication.

#### R2.1b IMAP — Fermé

L’extension du mécanisme exact à IMAP est fermée pour son périmètre. Les
anciennes identités `source_account` libres restent volontairement non
éligibles tant qu’aucune correspondance durable avec une configuration IMAP
n’est prouvée. La prochaine étape est R2.2, le salvage/export des orphelins.

**Produit / utilisateur**

Restaurer un RAW physiquement manquant lorsque Memoria possède encore une identité source durable suffisamment forte.

**Assurance**

- Gmail : `source_account + gmail_message_id` ;
- IMAP : `source_account + mailbox + UIDVALIDITY + UID` ;
- aucune recherche heuristique par sujet/date/expéditeur ;
- le re-fetch ne doit pas modifier les frontiers par simple effet de bord ;
- le RAW récupéré doit être validé avant toute nouvelle publication.

**Question structurante**

Si un BLAKE3 historique existe et que le RAW récupéré aujourd’hui diffère, Memoria doit signaler une nouvelle contradiction ; le nouveau contenu ne remplace pas silencieusement l’ancien attendu.

**Dépend de** : A1, A3.1, A3.3, single-writer, R1.

### R2.2 — Salvage/export des RAW orphelins

**Produit / utilisateur**

Permettre d’exporter ou de décrire les `OrphanValidated` byte-exacts sans les adopter dans le catalogue autoritatif.

**Assurance**

- aucune adoption par `doc_id` ;
- aucune provenance inventée ;
- manifest explicite des preuves réellement disponibles ;
- same-doc-id ne change pas l’autorité du record déjà publié.

**Dépend de** : détection physique, A1, R1.

### R2.3 — Modèle persistant de salvage

**Produit / utilisateur**

Conserver durablement la connaissance de RAW récupérés dont la provenance complète a été perdue, sans les faire passer pour des messages source-liés normaux.

**Assurance**

Le schéma catalogue v1 ne représente pas honnêtement ce cas. Deux directions restent possibles :

- modèle de salvage séparé ;
- évolution explicite et versionnée du catalogue.

La préférence actuelle est de ne pas contaminer v1 tant que le modèle de recovery n’exige pas réellement une migration.

**Dépend de** : R2.2 et les conclusions R1.

### R2.4 — Cleanup des tails incomplètes

**Produit / utilisateur**

Permettre un nettoyage explicite de queues terminales inutilisables afin de rendre l’état physique plus propre.

**Assurance**

Une future troncature exige simultanément :

- terminalité démontrée ;
- absence de claim/chevauchement catalogue ;
- absence de frame valide ultérieure ;
- autorité single-writer détenue ;
- action destructive explicitement demandée et autorisée.

**Dépend de** : détection physique, single-writer, R1.

### R2.5 — Repair/relink catalogue explicite

**Produit / utilisateur**

Traiter certains `CataloguedInconsistent` lorsqu’une preuve réellement non ambiguë permet de proposer une réparation.

**Assurance**

- aucune correspondance par simple `doc_id` ;
- aucune heuristique MIME/proximité ;
- une éventuelle correspondance BLAKE3 doit être auditée comme preuve et non supposée suffisante ;
- l’utilisateur doit voir la contradiction et l’action proposée.

**Dépend de** : R2.3, single-writer, R1.

### R2.6 — Catalogue perdu / reconstruction partielle

**Produit / utilisateur**

Exploiter une archive RAW lorsque `metadata.sqlite` est perdu ou irrécupérable.

**Assurance**

Le RAW seul permet de retrouver les octets, leur position, le `doc_id` de frame, checksum et BLAKE3 ; il ne recrée pas automatiquement compte source, Gmail ID, labels/thread/history ou identité IMAP.

La cible est donc d’abord un **salvage fidèle**, pas une prétendue reconstruction historique complète.

**Dépend de** : R2.2, R2.3 et éventuellement R2.1.

## 5. Roadmap fonctionnelle produit

### 5.1 Synchronisation et sources

| Fonction | Valeur utilisateur | Dépendances / contraintes |
|---|---|---|
| Synchronisation automatique / background | Archive maintenue à jour sans action manuelle | Workflow manuel d’abord stable ; respecter single-writer et frontiers |
| Multi-comptes Gmail dans l’UI | Centraliser plusieurs comptes | Provenance/account déjà Tier A ; UI et scheduling à étendre |
| IMAP intégré au produit | Archiver d’autres fournisseurs | Le CLI readonly multi-mailbox existe ; validation UX et sync nécessaire |
| Import MBOX | Importer des archives locales/offline | Définir identité/provenance locale avant ingestion produit |
| Autres sources | Étendre Memoria au-delà de Gmail/IMAP | À décider source par source ; pas de modèle générique prématuré |

### 5.2 Restauration et migration

| Fonction | Valeur utilisateur | Dépendances / contraintes |
|---|---|---|
| Export EML individuel/batch | Déjà disponible, byte-exact | A1 |
| Restauration complète | Reconstituer une archive exploitable après panne/migration | R2, modèle de salvage, politique de provenance |
| Migration Gmail → Gmail | Réinjecter les données vers un autre compte | Nécessite un connecteur d’écriture distinct et explicitement autorisé ; ne doit pas affaiblir le connecteur Gmail readonly actuel |
| Recovery UI | Présenter preuves, choix et actions possibles | R1 fermé ; actions R2 à stabiliser avant UI générale |

### 5.3 Recherche, pièces jointes et contenu

| Fonction | Valeur utilisateur | Dépendances / contraintes |
|---|---|---|
| Extension de l’indexation des pièces jointes | Recherche dans davantage de documents | Justifier formats/providers sur corpus réels ; traitements restent Tier B/C |
| PDF/Office multiplateforme | Recherche documentaire cohérente entre OS | Providers natifs/sandboxing selon plateforme |
| OCR / images | Recherche dans scans et images utiles | Coût/ressources à borner ; jamais source Tier A |
| Recherche enrichie | Filtres et navigation supplémentaires | Tantivy/FTS restent dérivés et reconstructibles |
| Previews système | Meilleure lecture des pièces jointes | Optionnel ; échec sans conséquence sur archive |

### 5.4 HTML, confidentialité et rendu

L’objectif reste un rendu utile sans transformer l’email archivé en contenu actif de confiance.

Directions fonctionnelles :

- améliorer le rendu HTML local/sanitisé ;
- conserver le support CID local ;
- continuer à bloquer les ressources distantes automatiques et le tracking ;
- laisser l’ouverture externe explicite à l’utilisateur ;
- ne pas faire du moteur de rendu une dépendance de l’autorité ou du recovery.

### 5.5 Stockage, compression et représentations dérivées

Cette branche reste fonctionnelle et volontairement séparée du recovery actuel.

#### Mode exact

Le RAW original reste reconstructible byte pour byte. Sont compatibles avec ce contrat :

- compression générique réversible ;
- déduplication exacte ;
- CAS permettant une reconstruction byte-exacte ;
- réorganisation physique démontrée réversible.

#### Mode dérivé / économie d’espace

À terme, l’utilisateur pourra éventuellement choisir de **ne conserver que des représentations dérivées** pour économiser de l’espace.

Cela constitue un changement explicite de garantie :

- l’export EML byte-exact peut devenir impossible ;
- la politique doit mémoriser que l’original a été volontairement abandonné ;
- les transformations doivent être traçables ;
- Memoria ne doit jamais confondre cette décision utilisateur avec une corruption ou une perte accidentelle.

Exemples futurs à étudier :

- déduplication de pièces jointes ;
- recompression sans perte lorsque la reconstruction du contenu attendu reste définie ;
- optimisation PNG/JPEG ou autres formats comme représentations dérivées lorsqu’elles ne préservent pas le bitstream original.

Aucun format définitif n’est adopté aujourd’hui.

### 5.6 Sauvegarde, rétention et offline

Directions produit :

- stratégie de sauvegarde incrémentale adaptée aux segments append-only ;
- vérification périodique d’intégrité ;
- restauration depuis sauvegarde ;
- fonctionnement durable hors ligne ;
- meilleure observabilité de l’état de santé de l’archive.

Ces fonctions doivent réutiliser inventaire/R1 plutôt que créer une seconde logique de validation.

### 5.7 Plateformes et distribution

#### Windows

- validation native complète UX/OAuth/dialogues/accessibilité ;
- packaging ;
- signature/distribution ;
- validation des providers natifs utilisés pour pièces jointes.

#### macOS

Le support produit reste à définir/valider séparément avant d’être annoncé. Les intégrations natives comme Quick Look/Spotlight doivent rester optionnelles et hors du processus d’autorité principal autant que possible.

#### Linux

Poursuivre la validation KDE/Wayland et les intégrations système sans rendre un desktop particulier nécessaire à la lecture de l’archive.

## 6. Assurance et hardening restant

### 6.1 Fuzzing framing / scanner

À lancer lorsque les interfaces sont suffisamment stables :

- magic/header ;
- longueurs/overflow ;
- allocations ;
- checksum ;
- corruption centrale ;
- tails ;
- combinaison de frames valides et invalides.

### 6.2 Fuzzing du planner de recovery

Objectif : aucune combinaison incohérente de preuves ne doit provoquer panic ou classification plus optimiste que les observations disponibles.

### 6.3 Crash/fault injection

Cibles :

- append RAW ;
- durable barrier ;
- publication catalogue ;
- publication frontier ;
- futures actions R2.

### 6.4 Durabilité namespace / power-loss multiplateforme

Sujet de hardening important mais non prioritaire à court terme.

À auditer :

- création d’un nouveau segment ;
- création initiale de l’archive ;
- rename/unlink futurs ;
- garanties Linux/macOS/Windows ;
- différence entre crash processus et perte brutale d’alimentation.

Le but est d’aligner exactement le sens de « durable » dans A3.2 avec les primitives du filesystem, sans bloquer le travail fonctionnel R2.

### 6.5 Portabilité du format d’archive

Le format binaire doit rester indépendant des représentations mémoire natives :

- endianness explicitement figée ;
- tailles entières fixes ;
- aucun `usize`/layout Rust natif sur disque ;
- framing/version explicites ;
- checksums calculés sur les octets sérialisés.

Pour de futures structures plus riches ou des environnements exotiques, un format de sérialisation versionné tel que Cap’n Proto reste un candidat pour les métadonnées structurées. Les gros payloads MIME doivent pouvoir rester sous contrôle direct de Memoria afin de préserver les possibilités d’optimisation physique.

## 7. Ordre de priorité proposé

| Priorité | Chantier | Impact cumulatif |
|---:|---|---|
| 1 | R2.1 re-fetch source | Première vraie récupération d’un manque à partir d’une preuve externe forte |
| 2 | R2.2 salvage/export orphan | Fonction utile à faible risque, sans modification de l’autorité |
| 3 | R2.3 modèle persistant de salvage | Base des récupérations partielles et catalogue perdu |
| 4 | R2.4 cleanup tail | Première mutation destructive strictement bornée |
| 5 | R2.5/R2.6 recovery catalogue complexe | Bénéficie de tout le modèle construit auparavant |
| 6 | Fuzzing / fault injection Tier A | Verrouille des interfaces désormais relativement stables |
| 7 | Durabilité namespace multiplateforme | Renforce le power-loss sans bloquer R2 |
| parallèle | Sync auto, multi-compte, IMAP UI, indexation PJ, Windows | Chantiers produit indépendants lorsque leurs prérequis immédiats sont stables |
| plus tard | CAS / représentations dérivées | Peut modifier le contrat d’autorité et doit rester un choix utilisateur explicite |

## 8. Comment maintenir cette roadmap

Une entrée doit préciser autant que possible :

- le bénéfice utilisateur ;
- la responsabilité Tier A/B/C concernée ;
- les prérequis déjà fermés ;
- les nouvelles garanties nécessaires ;
- les garanties existantes qu’elle pourrait affaiblir ;
- si elle est **implémentée**, **en assurance**, **candidate**, ou simplement **future**.

Les tickets d’implémentation détaillés n’ont pas vocation à vivre ici. Cette roadmap sert à préserver la cohérence d’ensemble entre produit et assurance.
