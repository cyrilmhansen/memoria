# Memoria — Recovery, preuves et réparation

Version 0.1 — 31 août 2026

Ce document décrit le sous-système de recovery de Memoria. Les invariants
généraux de conservation restent définis dans [`ASSURANCE.md`](ASSURANCE.md) et
le modèle de source/provenance dans [`ARCHITECTURE.md`](ARCHITECTURE.md).

Principe directeur :

> Le recovery transforme des preuves explicites en actions explicitement
> autorisées. Une ambiguïté n'est jamais convertie en autorité par heuristique.

## 1. Frontière R1 / R2

### R1 — diagnostic et plan de preuves

R1 est strictement read-only.

`recovery-plan` :

- inventorie l'état physique et catalogue ;
- classe les contradictions et possibilités de recovery ;
- n'acquiert pas l'autorité single-writer ;
- n'écrit ni RAW, ni SQLite, ni sidecar ;
- ne contacte pas une source distante ;
- n'avance aucun frontier ;
- ne tronque rien.

R1 décrit ce qui est prouvé et quelles actions R2 pourraient éventuellement
être admissibles.

### R2 — actions

R2 est composé d'actions séparées. Il ne doit pas devenir une commande générale
`recover --force`.

Chaque action possède :

- un état d'entrée admissible ;
- des preuves obligatoires ;
- une destructivité explicite ;
- une autorité d'écriture définie ;
- une politique de panne ;
- des tests discriminants.

## 2. États physiques/catalogue principaux

### `CataloguedValidated`

Le catalogue revendique un record et la frame physique correspondante est
validée avec les preuves attendues.

Aucune action de salvage n'est justifiée.

### `OrphanValidated`

Une frame physique est valide et prouvée, mais n'est pas revendiquée par le
catalogue comme record publié correspondant.

Cette frame peut être exportée/salvagée. Elle ne devient pas une ligne
`messages` par simple `doc_id`, proximité ou contenu MIME.

### `CataloguedInconsistent`

Le catalogue et la réalité physique se contredisent.

L'état est unsafe pour un relink automatique. Une correspondance heuristique ne
suffit pas.

### `PhysicallyMissing`

Une publication catalogue existe mais le RAW physique attendu n'est plus
disponible à sa localisation autoritative.

La possibilité de re-fetch dépend d'une identité source durable et du digest
historique disponible.

### `PhysicalCorruption`

Les octets physiques ne permettent pas d'établir une frame valide correspondant
à la preuve attendue.

Une zone corrompue n'est pas un salvage.

### `IncompleteTail`

Une queue de segment est incomplète.

Cet état ne constitue pas une autorisation de troncature. Une action destructive
future exige une preuve spécifique de terminalité et d'absence de record valide
ultérieur.

## 3. Classification de recoverability

### `RecoverableWithSource`

Un RAW physiquement manquant peut être candidat à un re-fetch lorsque Memoria
possède une identité source suffisamment forte pour le module concerné.

Cette classification signifie seulement :

> une tentative de re-fetch peut être autorisable.

Elle ne garantit ni disponibilité de la source, ni égalité des octets actuels
avec le RAW historique.

### `UnrecoverableLocally`

Les preuves locales ne permettent pas de reconstruire ou re-fetcher le RAW
historique avec le niveau d'autorité requis.

Cet état doit rester explicite plutôt que provoquer une reconstruction
heuristique.

## 4. Règles générales de preuve

Ne constituent jamais à eux seuls une preuve suffisante pour une réparation
Tier A :

- `doc_id` ;
- `Message-ID` MIME ;
- sujet ;
- date ;
- expéditeur/destinataires ;
- proximité physique ;
- ordre de messages ;
- index Tantivy ;
- texte extrait ;
- HTML rendu ;
- thumbnail ;
- ressemblance statistique.

Les digests et identités source peuvent contribuer à une preuve lorsque leur
contrat est explicitement défini.

## 5. R2.1 — Re-fetch exact par source

R2.1 est fermé pour les deux modules actuellement supportés : Gmail et IMAP.

Le contrat commun est :

1. revalider l'état local sous l'autorité appropriée ;
2. exiger une identité source durable non ambiguë ;
3. effectuer un fetch exact du record demandé ;
4. vérifier l'identité retournée lorsqu'elle existe dans le protocole ;
5. obtenir les octets RAW ;
6. exiger l'égalité avec le BLAKE3 historique ;
7. publier dans une destination fraîche selon la frontière Tier A ;
8. ne pas faire du recovery une synchronisation implicite.

Un contenu distant différent ne remplace jamais silencieusement le RAW
historique.

### 5.1 R2.1a — Gmail exact — fermé

Éligibilité minimale :

- état local `PhysicallyMissing` ;
- absence de contradiction physique/catalogue non résolue ;
- identité Gmail durable unique ;
- source encore marquée présente selon le contrat actuel ;
- profil OAuth authentifié correspondant au `source_account` canonique ;
- Gmail message ID retourné égal à l'identité demandée ;
- BLAKE3 des octets re-fetchés égal au `raw_blake3` historique.

L'identité persistée du compte Gmail est une clé opaque dérivée du compte
authentifié selon le helper canonique du projet ; l'adresse affichée n'est pas
la clé Tier A.

Le recovery Gmail ne modifie pas, par simple effet de bord :

- frontier/history ;
- labels ;
- thread metadata ;
- état de synchronisation général.

### 5.2 R2.1b — IMAP exact — fermé

L'identité durable actuelle est :

```text
source_account + mailbox + UIDVALIDITY + UID
```

Les anciennes identités libres qui ne peuvent pas être reliées de façon sûre à
une configuration IMAP restent non éligibles sans migration explicite.

Le contrat de fetch actuel exige notamment :

- `EXAMINE` ;
- UIDVALIDITY exacte et positive ;
- UID positif ;
- `UID FETCH ... BODY.PEEK[]` ;
- aucune mutation de flags ;
- réponse non ambiguë ;
- corps full-message ;
- tagged completion `OK` ;
- BLAKE3 historique identique.

Les détails d'API Rust particuliers peuvent évoluer tant que ces propriétés
restent démontrées.

### 5.3 Publication commune

Gmail et IMAP partagent le principe :

```text
destination fraîche non revendiquée
→ append RAW
→ barrière durable
→ publication catalogue conditionnelle
```

Un conflit après append durable peut laisser une nouvelle frame
`OrphanValidated`. C'est un mode de panne sûr : le RAW existe sans fausse
publication.

Avec M1, un re-fetch réussi ne réutilise pas la claim physique historique.
Même lorsque les octets ont le digest historique attendu, l'append crée un
nouveau `raw_record_id`. La publication est un compare-and-swap sur le lien
courant de l'occurrence (la valeur attendue est l'ancien `raw_record_id`),
suivi dans la même transaction single-writer par la relation de remplacement
typée du provider. Cette transaction met aussi à jour
`messages.raw_record_id` et chaque projection descriptive/current-RAW requise
vers le nouveau `raw_record_id`, avant son commit. Il n'existe donc aucun état
commis où le lien provider courant désigne le nouveau RAW tandis que les
chemins normaux du catalogue désignent encore l'ancien. L'ancien claim
physique reste représenté comme historique et le nouveau devient le lien
courant. Si le compare-and-swap échoue, la transaction est annulée et seule la
nouvelle frame durable subsiste comme `OrphanValidated`; aucune adoption ou
substitution implicite n'est permise. Les valeurs current-RAW des tables de
présentation sont des projections dérivées : elles ne peuvent ni remplacer
ni reconstruire l'autorité provider. Une incohérence impose un échec fermé ou
une reconstruction explicitement contractée depuis l'occurrence provider
typée, jamais une inférence par similarité. Le `acquisition_id` immuable de
l'occurrence n'est pas écrasé : l'acquisition de re-fetch est celle du nouveau
RAW et de la transition de remplacement.

## 6. R2.2 — Salvage/export des orphelins

### R2.2a — Export byte-exact d'un `OrphanValidated` — fermé

L'export exige une référence physique suffisamment discriminante :

```text
segment
offset
frame_bytes
doc_id observé
raw_blake3 observé
```

L'opération :

- acquiert l'autorité single-writer pendant revalidation et lecture ;
- rescane l'inventaire ;
- exige que la frame soit toujours `OrphanValidated` ;
- revalide la même identité physique et le digest ;
- relit cette même frame ;
- écrit le payload RAW byte-exact avec création exclusive ;
- impose une destination hors de toute la racine archive canonique, y compris
  via alias/symlink ;
- produit seulement un manifest de faits physiques observés.

Elle ne :

- crée pas de provenance ;
- adopte pas la frame dans `messages` ;
- ne modifie pas SQLite ;
- ne modifie pas RAW ;
- n'avance pas de frontier ;
- ne modifie pas Tantivy.

Un `doc_id` identique à celui d'un message déjà catalogué ne change pas
l'autorité du record publié.

### R2.2 global — encore ouvert

R2.2a fournit le socle d'assurance nécessaire au salvage explicite.

Les extensions éventuelles de listing, batch ou UX ne doivent pas devenir une
dépendance artificielle de M1 si elles n'ajoutent pas de nouvelle preuve.

## 7. R2.3 — Identifiant historique réservé ; modèle déplacé vers M1

`R2` désigne exclusivement les **actions de recovery bornées**.

Le chantier autrefois appelé « R2.3 — modèle persistant de salvage » a révélé
un périmètre plus large : le modèle d'acquisition/provenance est utilisé aussi
par l'acquisition normale et les futurs imports. Ce n'est donc plus une action
R2.

Le chantier architectural correspondant est désormais :

```text
M1 — modèle persistant d'acquisition/provenance
```

défini dans [`ARCHITECTURE.md`](ARCHITECTURE.md) et priorisé dans
[`ROADMAP.md`](ROADMAP.md).

Pour préserver la traçabilité historique, les identifiants existants R2.4,
R2.5 et R2.6 ne sont pas renumérotés. `R2.3` reste réservé comme ancien
identifiant et ne doit pas être réutilisé pour une autre action.

### Relation avec R2.2

R2.2a fournit déjà le socle de preuve physique nécessaire pour concevoir M1.

Un éventuel R2.2b consacré au batch/listing/UX peut rester non bloquant tant
qu'il n'introduit pas une preuve nouvelle nécessaire au modèle persistant.

### Frontière d'autorité à préserver

Persister un RAW avec une provenance partielle ou inconnue ne constitue jamais
implicitement :

- une publication source forte ;
- une adoption d'un `OrphanValidated` dans `messages` ;
- un relink catalogue ;
- une reconstruction de provenance.

M1 doit conserver séparément les faits directement attestés par un contrat
provider, les attributs observés, les projections dérivées et les dimensions
inconnues. Une absence ou un champ nullable ne doit ni effacer une identité
forte déjà attestée, ni être promu en preuve par agrégation. Les provenances
déclarées ou intermédiaires ne seront persistées dans une structure typée que
lorsqu'un producteur concret en démontrera le contrat.

La persistance concrète de ces qualifications et sa migration sont spécifiées dans
[`M1-PERSISTENCE.md`](M1-PERSISTENCE.md). Elle ne crée aucune nouvelle action
R2 et ne modifie pas la frontière read-only de R1.

## 8. R2.4 — Cleanup des tails incomplètes

R2.4 est ouvert et destructif.

Une troncature ne pourra être envisagée qu'après démonstration simultanée :

- que la zone est réellement terminale ;
- qu'aucun claim catalogue ne la concerne ou ne la chevauche ;
- qu'aucune frame valide ultérieure n'existe ;
- que l'autorité single-writer est détenue ;
- que la destruction est explicitement demandée.

R1 ne tronque jamais.

## 9. R2.5 — Repair/relink catalogue

R2.5 est ouvert.

Une future réparation de `CataloguedInconsistent` doit reposer sur une preuve
définie et auditée.

Sont explicitement insuffisants :

- `doc_id` seul ;
- similarité MIME ;
- proximité physique ;
- index dérivé.

Un BLAKE3 correspondant peut être un élément de preuve mais ne doit pas être
déclaré suffisant avant analyse du cas et du risque de substitution.

L'utilisateur ou la couche appelante doit pouvoir distinguer clairement la
contradiction observée de l'action proposée.

## 10. R2.6 — Catalogue perdu / reconstruction partielle

R2.6 est ouvert.

Le RAW seul peut permettre de retrouver :

- les octets ;
- la position physique ;
- l'identité portée par le framing ;
- checksums/digests stockés ou recalculables.

Il ne recrée pas automatiquement :

- compte source ;
- Gmail message ID ;
- labels/thread/history ;
- identité IMAP ;
- contexte d'import ;
- provenance externe historique.

La première cible est donc un salvage fidèle et exploitable, pas une
reconstruction fictive de l'histoire.

## 11. Matrice simplifiée

| État | Lecture/export | Re-fetch exact | Adoption/relink | Destruction |
|---|---|---|---|---|
| `CataloguedValidated` | normal | non applicable | non | non |
| `OrphanValidated` | export R2.2a | non par défaut | futur, preuve explicite | non |
| `PhysicallyMissing` + identité forte | absent localement | R2.1 possible | publication contrôlée | non |
| `CataloguedInconsistent` | diagnostic | selon cas futur | R2.5 futur | non |
| `PhysicalCorruption` | diagnostic | selon identité source, pas depuis les octets corrompus | non | non par défaut |
| `IncompleteTail` | diagnostic | non | non | R2.4 futur seulement |

Cette matrice résume les classes ; les préconditions détaillées restent
autoritatives.

## 12. Failure policy

Lorsqu'une preuve Tier A est ambiguë, l'action mutante doit échouer de façon
explicite.

Une perte de télémétrie ou une erreur d'UI ne doit pas être confondue avec une
preuve d'intégrité manquante.

Le recovery doit privilégier :

```text
orphan valide
>
fausse provenance
>
substitution silencieuse
```

et conserver les contradictions plutôt que les masquer.

## 13. Évolution

Une nouvelle source ajoute son propre contrat de re-fetch uniquement si son
protocole peut établir une identité suffisamment forte et si le produit a une
raison de le faire.

Le fait qu'un module sache acquérir un message ne signifie pas qu'il sait le
re-fetcher exactement plus tard.

Les contrats provider-specific restent donc explicites, tandis que le modèle
conceptuel commun est défini dans `ARCHITECTURE.md`.
