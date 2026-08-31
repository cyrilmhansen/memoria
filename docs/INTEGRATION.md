# Intégration de la refonte documentaire

Cette première passe ajoute trois documents et remplace trois documents
d'autorité. Deux petites mises à jour supplémentaires sont recommandées lors de
l'intégration dans le dépôt.

## README.md — section Project documentation

Remplacer la liste actuelle par une carte correspondant aux nouvelles
responsabilités :

```text
README.md                         current product state and user-visible limits
AGENTS.md                         working rules and documentation routing
ARCHITECTURE.md                   conceptual model and authority boundaries
ASSURANCE.md                      A/B/C criticality and conservation guarantees
SECURITY.md                       security threat model and capability policy
RECOVERY.md                       recovery evidence, states and bounded actions
ROADMAP.md                        priorities and dependencies
KNOWLEDGE.md                      durable verified technical facts
WORKLOG.md                        lightweight development history
experiments/                      measurements, probes and detailed reports
projects/mail-archive/            current Memoria implementation
```

Ajouter une phrase indiquant que les documents spécialisés sont autoritatifs
pour leur domaine et que `KNOWLEDGE.md` ne doit pas les dupliquer.

## KNOWLEDGE.md — en-tête

Remplacer :

```text
La référence pour la criticité et l’assurance du code est désormais
[`ASSURANCE.md`](ASSURANCE.md).
```

par :

```text
Les documents d'autorité sont séparés par responsabilité :
[`ARCHITECTURE.md`](ARCHITECTURE.md) pour le modèle conceptuel,
[`ASSURANCE.md`](ASSURANCE.md) pour la conservation et la criticité,
[`SECURITY.md`](SECURITY.md) pour le threat model,
[`RECOVERY.md`](RECOVERY.md) pour le recovery, et
[`ROADMAP.md`](ROADMAP.md) pour les priorités.

`KNOWLEDGE.md` reste une carte de faits techniques durables et ne doit pas
dupliquer ces politiques.
```

## Vérification après application

Pour cette tranche strictement documentaire :

```text
git diff --check
git status --short
git diff --name-only
```

Le diff ne devrait contenir aucun fichier Rust, SQL, Slint ou de test.

## Relecture recommandée

La patch review devrait vérifier séparément :

1. **Architecture**
   - le modèle commun est-il conceptuel plutôt qu'un framework prématuré ?
   - Gmail/IMAP restent-ils des modules particuliers et non le modèle universel ?
   - la distinction provenance attestée/déclarée/observée/dérivée/inconnue est-elle cohérente ?

2. **Assurance**
   - aucune garantie fermée A1/A2/A3/R1/R2.1/R2.2a n'a-t-elle été affaiblie ?
   - les détails déplacés dans `RECOVERY.md` restent-ils assez précis pour empêcher une régression d'autorité ?

3. **Sécurité**
   - le threat model est-il proportionné au produit local ?
   - Gmail/IMAP read-only, HTML, attachments, credentials et helpers externes sont-ils correctement séparés ?
   - les non-objectifs évitent-ils une dérive vers un sandbox de niveau hostile-host ?

4. **Roadmap**
   - M1 est-il correctement séparé de R2, réservé aux actions de recovery bornées ?
   - R2.2a est-il bien suffisant pour commencer la conception de M1 ?
