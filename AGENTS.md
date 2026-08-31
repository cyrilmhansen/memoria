# Règles de travail

Ce dépôt contient Memoria et des expériences associées. Les documents
d'autorité sont séparés par responsabilité afin d'éviter qu'un agent doive
charger toute l'histoire du projet avant chaque tâche.

## 1. Cold-start

Pour un travail normal, lire dans cet ordre :

1. `AGENTS.md`
2. `README.md`
3. `docs/ARCHITECTURE.md`

Puis charger uniquement les autorités nécessaires au travail :

- **Tier A, stockage, acquisition, intégrité, publication :**
  `docs/ASSURANCE.md`
- **recovery, corruption, salvage, relink :**
  `docs/ASSURANCE.md` puis `docs/RECOVERY.md`
- **HTML, pièces jointes, réseau, credentials, helpers externes, privacy :**
  `docs/SECURITY.md`
- **sélection/cadrage du prochain chantier :**
  `docs/ROADMAP.md`
- **faits techniques déjà établis :**
  `docs/KNOWLEDGE.md`
- **chronologie ou ancienne décision précise :**
  `WORKLOG.md`
- **mesures, probes et échecs expérimentaux :**
  document pertinent dans `experiments/`

`WORKLOG.md` ne doit pas être lu intégralement par défaut.

`docs/KNOWLEDGE.md` est une carte de faits, pas l'autorité de politique lorsqu'un
document spécialisé existe.

## 2. Hiérarchie documentaire

Les responsabilités sont :

```text
README.md                   état produit réellement disponible
docs/ARCHITECTURE.md        modèle conceptuel et frontières d'autorité
docs/ASSURANCE.md           criticité A/B/C et garanties de conservation
docs/SECURITY.md            threat model et capacités de sécurité
docs/RECOVERY.md            preuves, états et actions de recovery
docs/ROADMAP.md             priorités et dépendances
docs/KNOWLEDGE.md           faits techniques durables
WORKLOG.md                  historique
experiments/                preuves détaillées, mesures et probes
```

Lorsqu'une information est dupliquée, préférer l'autorité spécialisée et
réduire progressivement les duplications.

## 3. Discipline de raisonnement

Distinguer explicitement :

- **fait vérifié** ;
- **hypothèse** ;
- **décision de projet** ;
- **question ouverte**.

Privilégier l'expérience minimale qui permet de trancher une incertitude.

Ne pas transformer une observation expérimentale en invariant sans preuve
appropriée.

Ne pas transformer une simplification d'implémentation en règle
architecturale.

## 4. Architecture

Ne pas ajouter une abstraction commune simplement parce que plusieurs sources
ou projets pourraient hypothétiquement en avoir besoin.

Le modèle source/acquisition/provenance de `docs/ARCHITECTURE.md` est conceptuel. Il
ne constitue pas une instruction pour créer immédiatement :

- un trait Rust universel ;
- une couche plugin ;
- un registre de providers ;
- un blob de métadonnées générique ;
- une migration SQLite.

Les abstractions d'implémentation doivent être justifiées par des cas réels.

Toute évolution qui modifie :

- la définition du RAW autoritatif ;
- l'identité/provenance Tier A ;
- l'ordre de publication ;
- le single-writer ;
- les conditions de recovery ;
- les capacités réseau/credentials ou d'écriture externe ;

doit être traitée comme une évolution de contrat d'autorité.

## 5. Sécurité et assurance

Ne pas confondre :

- sécurité ;
- assurance de conservation ;
- reproductibilité ;
- méthodologie de développement.

Une mesure de sécurité doit correspondre au threat model de `docs/SECURITY.md`.

Une mesure Tier A doit correspondre aux conséquences de perte, corruption ou
substitution décrites dans `docs/ASSURANCE.md`.

Ne pas ajouter un mécanisme coûteux uniquement parce qu'il est techniquement
possible.

## 6. Recovery

R1 reste read-only.

R2 est composé d'actions bornées ; ne pas introduire un `recover --force`
général.

Ne jamais justifier une réparation Tier A uniquement par :

- `doc_id` ;
- contenu MIME ;
- sujet/date/expéditeur ;
- proximité ;
- index dérivé.

Consulter `docs/RECOVERY.md` avant toute modification d'un chemin de recovery.

## 7. Expériences

Enregistrer dans `experiments/` :

- commandes reproductibles ;
- versions utiles ;
- corpus ;
- mesures ;
- échecs ;
- hypothèses testées ;
- références externes.

Promouvoir dans `docs/KNOWLEDGE.md` seulement les conclusions qui ont une conséquence
réutilisable.

## 8. Validation

Après chaque étape significative, laisser le dépôt dans un état compilable
lorsque du code a été modifié.

Socle normal :

```text
cargo fmt --all -- --check
cargo test --workspace
cargo check --workspace
git diff --check
```

Adapter les tests au périmètre de la tâche plutôt que lancer des contrôles
coûteux sans raison.

Pour un changement purement documentaire :

```text
git diff --check
git status --short
```

et vérifier qu'aucun fichier de code n'a été modifié involontairement.

## 9. Portée et modifications non liées

Préserver les modifications et fichiers non liés à la tâche.

Ne pas supprimer, normaliser ou intégrer automatiquement des fichiers
préexistants non trackés.

Ne pas faire de commit, tag, push ou réécriture d'historique sauf autorité
explicitement déléguée par le workflow ou l'utilisateur.

## 10. Produit et i18n

Toute nouvelle chaîne visible par l'utilisateur passe par le mécanisme i18n.

Toute chaîne constituant un identifiant de format, protocole, schéma ou
configuration reste indépendante de la langue et est centralisée dans son
module propriétaire lorsqu'elle est structurellement partagée.
