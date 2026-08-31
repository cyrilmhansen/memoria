# Memoria — Architecture et modèle d'autorité

Version 0.1 — 31 août 2026

Ce document définit le modèle conceptuel durable de Memoria. Il décrit les
responsabilités et frontières d'autorité indépendamment d'un découpage
particulier en crates, traits Rust ou tables SQLite.

La règle centrale est :

> Memoria conserve des octets de référence, atteste séparément comment ils ont
> été acquis, puis construit des interprétations et représentations dérivées
> qui ne deviennent jamais l'autorité sur ces octets par simple commodité.

## 1. Vue d'ensemble

Le modèle conceptuel est :

```text
source ou corpus externe
        │
        ▼
module d'acquisition
        │
        ├── identité de l'instance source
        ├── identité du record dans cette source
        ├── faits directement attestés
        └── métadonnées spécifiques au module
        │
        ▼
publication Memoria
        │
        ├── record RAW physique byte-exact
        ├── identité/localisation catalogue
        └── provenance durable disponible
        │
        ├───────────────┐
        ▼               ▼
parsing Tier B      état source/sync
        │
        ▼
index, vues, previews, UI Tier C
```

Les couches peuvent partager une base physique ou un crate. Leur coexistence ne
leur donne pas la même autorité.

## 2. Autorité byte-exacte

Dans le mode de conservation actuel, l'autorité locale fondamentale est le RAW
MIME original conservé byte pour byte.

L'unité logique de conservation est le record RAW individuel identifié. Un
segment n'est qu'un conteneur physique.

SQLite contient actuellement un mélange de données d'autorité différentes :

- coordonnées physiques Tier A ;
- identités et provenance Tier A ;
- état source mutable ;
- état de synchronisation ;
- métadonnées de navigation ou dérivées.

Tantivy, parsing MIME, HTML rendu, thumbnails, texte extrait, ranking et vues UI
sont dérivés.

Le fait qu'une donnée soit persistée ne la rend pas autoritative.

## 3. Le concept de source

Une **source** est un module d'acquisition capable de fournir des octets à
Memoria et d'attester certains faits sur cette acquisition.

Le cœur Memoria ne doit pas considérer Gmail ou IMAP comme les deux formes
fondamentales de provenance. Ce sont deux modules parmi d'autres possibles.

Exemples légitimes :

- Gmail ;
- IMAP ;
- import EML ;
- import MBOX ;
- Outlook/PST ou API Outlook ;
- MailStore Home ;
- Apple Mail, Thunderbird ou autres corpus futurs.

Le modèle commun doit rester minimal. Il prépare ces sources sans imposer
aujourd'hui un trait Rust universel, une hiérarchie de classes ou un schéma SQL
générique définitif.

## 4. Responsabilités du cœur Memoria

Le cœur est responsable de :

- l'identité physique et la fidélité du RAW local ;
- la publication Tier A ;
- les barrières de durabilité ;
- la cohérence entre RAW, catalogue et provenance publiée ;
- la gestion single-writer ;
- les invariants communs de lecture et de recovery ;
- la distinction entre faits attestés, déclarations externes et contenu observé ;
- la possibilité de conserver un RAW même lorsqu'une provenance complète
  n'existe pas ou n'est plus démontrable ;
- la séparation entre données autoritatives et dérivées.

Le cœur ne doit pas prétendre connaître la sémantique complète de chaque
provider.

## 5. Responsabilités d'un module d'acquisition

Un module source est responsable de :

- définir ce qui identifie une instance de sa source lorsque cette notion existe ;
- définir ce qui identifie un record dans son domaine ;
- acquérir les octets selon son protocole ou format ;
- attester uniquement les propriétés réellement observées ou vérifiées ;
- conserver les métadonnées provider-specific nécessaires à la synchronisation,
  au recovery ou à l'expérience utilisateur ;
- distinguer ses propres preuves des déclarations contenues dans les données
  importées ;
- ne pas promouvoir une heuristique en identité Tier A.

Un module n'a pas à exposer les mêmes métadonnées qu'un autre.

## 6. Concepts communs minimaux

Sans présumer du schéma final, tout modèle persistant futur devrait pouvoir
exprimer conceptuellement :

### 6.1 Type/module d'acquisition

Quel code ou protocole a réalisé l'acquisition :

```text
gmail
imap
eml-import
mbox-import
mailstore-import
outlook
...
```

Cette valeur identifie une sémantique de module, pas une preuve suffisante à
elle seule.

### 6.2 Instance de source

L'entité externe ou locale à laquelle se rattache l'acquisition lorsque le
module peut l'attester.

Exemples :

- compte Gmail authentifié ;
- configuration IMAP authentifiée ;
- fichier MBOX précis ;
- opération d'import locale ;
- base MailStore précise.

Certaines sources peuvent n'avoir aucune notion stable d'instance externe.

### 6.3 Identité du record source

L'identité d'un record selon le module lorsqu'elle existe.

Exemples :

- Gmail message ID ;
- mailbox + UIDVALIDITY + UID ;
- position/record d'un conteneur local ;
- identifiant attesté par MailStore.

Cette identité n'est jamais remplacée par `Message-ID` MIME sauf si un futur
module définit explicitement et prouve un tel contrat, ce qui n'est pas le cas
aujourd'hui.

### 6.4 Métadonnées spécifiques au module

Les modules peuvent persister des données différentes :

- Gmail labels/thread/history ;
- IMAP flags/mailbox/frontiers ;
- chemin ou identité d'un corpus d'import ;
- identifiants et attributs propres à MailStore ;
- autres données versionnées nécessaires au module.

Il n'est pas nécessaire de normaliser prématurément toutes ces données dans des
colonnes communes.

## 7. Classes de provenance et de preuve

Memoria distingue au minimum les catégories suivantes.

Ces catégories qualifient **des assertions individuelles**, pas un record dans
son ensemble.

Un même RAW peut donc simultanément avoir :

- une identité Gmail directement attestée ;
- une origine historique déclarée par une source intermédiaire ;
- des en-têtes MIME seulement observés ;
- des interprétations dérivées ;
- certaines dimensions de son histoire inconnues ou non démontrées.

Il ne doit pas exister de règle implicite du type :

```text
record.provenance_level = weakest_or_strongest_level
```

qui écraserait cette composition.

Une provenance partielle ou inconnue sur une assertion ne retire pas
l'autorité d'une autre assertion déjà prouvée. Inversement, une assertion forte
ne transforme pas les autres dimensions du record en faits attestés.

L'état source mutable et l'état/frontier de synchronisation constituent encore
d'autres axes : ils peuvent être associés à une acquisition, mais ne sont pas
des « niveaux de provenance ».

### 7.1 Provenance directement attestée par Memoria

Fait observé ou vérifié directement par le module d'acquisition dans son
contexte d'autorité.

Exemples :

- compte Gmail authentifié + Gmail message ID retourné ;
- session IMAP configurée + mailbox + UIDVALIDITY + UID ;
- octets exacts lus d'un fichier explicitement importé avec son contexte
  d'import.

Cette catégorie peut contribuer à une identité/provenance Tier A.

### 7.2 Provenance déclarée par une source intermédiaire

Une source peut déclarer qu'un objet provient d'un système antérieur sans que
Memoria ait contacté directement ce système.

Exemple :

- MailStore expose un identifiant interne et indique qu'un message provenait
  historiquement d'un compte ou dossier donné.

Memoria peut conserver cette déclaration, mais doit la distinguer d'une preuve
directement attestée auprès du système originel.

### 7.3 Contenu observé

Informations contenues dans les octets eux-mêmes :

- `Message-ID` ;
- `From`, `To`, `Date`, `Subject` ;
- `Received` ;
- corps du message ;
- contenu des pièces jointes.

Ces données décrivent ce que Memoria observe dans le RAW. Elles ne constituent
pas automatiquement une preuve d'acquisition ou d'origine.

### 7.4 Interprétation dérivée

Résultats produits par parsing, indexation, normalisation ou analyse :

- texte extrait ;
- adresse normalisée ;
- threading dérivé ;
- détection de langue ;
- OCR ;
- classification ou résumé futur.

Cette couche n'est jamais une preuve Tier A par défaut.

### 7.5 Provenance inconnue ou non démontrée

Une assertion peut être inconnue ou non démontrée sans que le reste des faits
concernant le RAW perde son statut.

Memoria peut aussi posséder un RAW valide sans disposer d'une liaison durable
suffisamment prouvée avec une acquisition/source.

Ces cas doivent être représentables explicitement. Ils ne doivent pas être
forcés dans un modèle de message source-lié normal et ne doivent pas recevoir
une provenance inventée.

Exemple : un RAW peut conserver une identité Gmail directement attestée tout en
ayant une origine antérieure à Gmail inconnue. L'inconnu sur cette origine ne
dégrade pas l'identité Gmail déjà prouvée.

## 8. Exemples de modules

### Gmail

Peut attester :

- identité canonique du compte authentifié selon le contrat Memoria ;
- Gmail message ID ;
- octets obtenus par le chemin Gmail RAW ;
- métadonnées Gmail observées.

Les en-têtes MIME ne remplacent pas cette identité.

### IMAP

Peut attester, selon le contrat actuel :

- configuration/identité de source persistée ;
- mailbox ;
- UIDVALIDITY ;
- UID ;
- octets retournés par le fetch exact.

Les anciennes clés libres qui ne permettent pas de relier durablement une
configuration ne deviennent pas automatiquement une provenance moderne.

### EML

Un import EML peut attester :

- qu'une opération locale a importé tel ensemble exact d'octets ;
- éventuellement le fichier sélectionné et son identité locale au moment de
  l'import si cette information est choisie comme durable.

Il ne peut pas attester directement que `From`, `Date` ou `Message-ID`
représentent une origine externe réelle.

### MBOX

Un module MBOX peut attester :

- le conteneur importé ;
- la position ou l'identité logique utilisée pour extraire un record ;
- les octets résultants ;
- le contexte de l'opération d'import.

Les détails de framing MBOX restent spécifiques au module.

### MailStore

Un import MailStore peut attester :

- l'identité du record dans MailStore ;
- les métadonnées effectivement fournies par MailStore ;
- éventuellement une origine déclarée par MailStore.

Cette origine déclarée reste distincte d'une acquisition directe Memoria auprès
de Gmail, IMAP ou d'un autre système originel.

## 9. Publication Tier A

Une acquisition réussie traverse conceptuellement :

```text
octets reçus
→ RAW durable
→ localisation catalogue durable
→ identité/provenance durable disponible
→ état/frontier source publié
```

L'ordre exact dépend du module, mais la frontière fondamentale reste :

> Memoria ne doit pas publier durablement qu'une occurrence source est
> conservée si son RAW autoritatif n'est pas lui-même durablement disponible.

Un RAW orphelin est préférable à une fausse publication source.

## 10. Recovery dans ce modèle

Le recovery ne constitue pas un second modèle de données. Il raisonne sur les
mêmes couches d'autorité.

Un `OrphanValidated` signifie qu'un RAW physique est prouvé mais qu'il n'est
pas actuellement publié comme record catalogue correspondant.

Il ne signifie pas nécessairement :

> ancien message Gmail ou IMAP dont on connaît encore l'origine.

Il peut aussi représenter un RAW dont la liaison durable avec une acquisition
n'est plus prouvable.

Le recovery doit donc pouvoir conserver la distinction :

```text
RAW physique prouvé
+
provenance prouvée, déclarée, observée ou inconnue
```

Voir [`RECOVERY.md`](RECOVERY.md).

## 11. M1 — Modèle persistant d'acquisition/provenance

Le chantier de modèle persistant est désormais identifié **M1** et sort de la
numérotation R2.

Raison : `R2` désigne les actions de recovery bornées. Un modèle architectural
persistant commun à l'acquisition normale, aux imports et au salvage n'est pas
une action de recovery.

La question M1 est :

> comment Memoria représente-t-il durablement des assertions d'acquisition et
> de provenance portant des niveaux de preuve différents, tout en permettant
> qu'un RAW valide existe avec une provenance partielle ou inconnue ?

Le salvage sans provenance attestée devient un cas de ce modèle, pas son
fondement unique.

Les catégories de preuve restent composables par assertion. M1 ne doit donc pas
introduire un unique niveau global de provenance attaché au message.

Cette décision est conceptuelle. Elle n'adopte aujourd'hui :

- aucun nouveau schéma SQLite ;
- aucune migration catalogue ;
- aucun blob JSON opaque non versionné ;
- aucun trait Rust universel ;
- aucune interface provider imposée aux sources futures.

## 12. Évolution du schéma

Une future évolution du catalogue doit :

- être versionnée explicitement ;
- préserver ou migrer les garanties Tier A ;
- distinguer les champs communs réellement stables des métadonnées propres aux
  modules ;
- éviter de transformer un conteneur opaque non versionné en contrat permanent ;
- permettre de représenter l'absence ou l'incertitude de provenance ;
- ne pas déduire une provenance forte à partir d'anciens champs insuffisants.

Une migration est un changement de contrat d'autorité et doit être auditée
comme tel.

## 13. Frontières avec les autres documents

- [`ASSURANCE.md`](ASSURANCE.md) définit la criticité et les invariants.
- [`SECURITY.md`](SECURITY.md) définit le threat model et les capacités de
  sécurité.
- [`RECOVERY.md`](RECOVERY.md) définit les états et actions de recovery.
- [`ROADMAP.md`](ROADMAP.md) décide quand une évolution de ce modèle devient un
  chantier.
- [`KNOWLEDGE.md`](KNOWLEDGE.md) conserve les faits techniques validés qui
  justifient les décisions sans dupliquer cette architecture.
