# Règles de travail

Ce dépôt sert de workspace Rust pour plusieurs petites applications
multiplateformes utilisant Slint. Il doit rester petit : il n'est pas un
framework partagé.

## Avant de travailler

- Lire `KNOWLEDGE.md` avant tout travail.
- Consulter les notes pertinentes dans `experiments/` avant de rechercher à
  nouveau une solution.
- Privilégier l'expérience minimale qui permet de trancher une incertitude.

## Pendant et après une expérience

- Distinguer explicitement **fait vérifié**, **hypothèse** et **décision de
  projet**.
- Enregistrer les commandes reproductibles, les versions utiles et les
  références externes.
- Conserver les mesures, échecs et détails d'expérience dans `experiments/`.
- Mettre à jour `KNOWLEDGE.md` lorsqu'une expérience confirme, infirme ou
  précise une information réutilisable.
- `KNOWLEDGE.md` reste une carte concise : conclusions importantes et
  pointeurs, jamais un compte rendu complet d'expérience.

## Structure et architecture

- Ne jamais ajouter une abstraction commune simplement parce que deux projets
  pourraient hypothétiquement en avoir besoin.
- Les emplacements `projects/mail-archive/` et
  `projects/disk-explorer/` restent vides tant que leurs besoins ne sont pas
  connus. Une fois les besoins explicitement explorés, le projet concerné
  peut accueillir un prototype isolé ; son architecture interne est décidée
  avec ce projet et ne devient pas une abstraction commune par défaut.
- Ne pas développer d'application dans le cadre de la mise en place initiale.

## État du dépôt

- Après chaque étape significative, laisser le dépôt dans un état compilable.
- Documenter dans le journal ce qui a été appris, y compris lorsqu'il n'y a
  encore aucune conclusion technique.

Toute nouvelle chaîne visible par l’utilisateur passe par le mécanisme i18n.
Toute chaîne constituant un identifiant de format, protocole, schéma ou
configuration reste indépendante de la langue et est centralisée dans son
module propriétaire lorsqu’elle est structurellement partagée.
