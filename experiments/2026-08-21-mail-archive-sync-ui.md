# Memoria — boucle synchronisation Gmail et recherche

Date : 2026-08-21  
Périmètre : réunir le connecteur Gmail readonly, l’archive RAW, l’index
Tantivy et la première UI Slint. Aucun nouveau format de stockage n’a été
introduit.

## Implémentation

- `sync_account` reste l’API utilisée par le CLI.
- `sync_account_with_progress` ajoute un callback optionnel de snapshots sans
  dupliquer l’algorithme. Les snapshots contiennent seulement des compteurs et
  volumes agrégés.
- Memoria lance `HttpGmail::authenticate`, la synchronisation puis
  `index_gmail_archive` dans un worker. La recherche reste dans le thread UI
  et le reader Tantivy est rechargé après le commit dérivé.
- Un `AtomicBool` empêche deux synchronisations simultanées.
- La vue `Archive / Synchronisation` présente messages, taille archive,
  segments, catalogue, état de l’index, état Gmail et résumé de la dernière
  action. Les détails `historyId`, frames et checkpoints restent absents de
  l’interface normale.
- L’annulation n’est pas ajoutée : le connecteur actuel ne possède pas encore
  de points d’annulation coopérative entre toutes les opérations réseau et
  écritures cohérentes. Une synchronisation concurrente reste interdite.

## États UI

`Source Gmail non configurée` → `Synchronisation Gmail en cours…` →
`Mise à jour de l’index…` (étape interne après import) → `Archive à jour ·
index de recherche à jour`.

Une erreur de synchronisation est présentée comme erreur de configuration,
Gmail/réseau ou archive selon sa famille. Une erreur d’index est distincte et
indique que les RAW restent disponibles. Une archive sans credentials reste
consultable hors ligne.

## Tests automatisés

Les tests ne contactent pas Gmail :

- 17 tests mail passent, dont progression fixture et index Tantivy
  incrémental après ajout d’un second message ;
- le transport fixture couvre déjà pagination, idempotence, metadata pour les
  messages connus et RAW pour les nouveaux, suppression et historique expiré ;
- l’index incrémental saute les documents inchangés (`indexed=0`, `skipped=3013`
  lors du contrôle local final).

Commandes :

```text
cargo fmt --all
cargo test --workspace -q
cargo check --workspace -q
cargo build -p mail-archive-experiment --bin mail-archive-app -q
```

## Validation réelle contrôlée

L’interface a été lancée avec les credentials et le token locaux déjà
présents hors de l’archive. Depuis le menu `Archive`, l’action
`Synchroniser maintenant` a été déclenchée dans Memoria.

Résultats agrégés observés :

- avant l’action : 3 012 messages ;
- après le premier passage UI : 3 013 messages, 4 segments, archive physique
  202,1 MiB ; un nouveau message a donc été archivé ;
- index Tantivy visible comme à jour après l’action, sans redémarrage de
  l’application ;
- second passage UI : `Aucun nouveau message`, archive toujours à 3 013
  messages ;
- contrôle offline : archive 211 891 029 octets, catalogue environ 1,6 MiB,
  index Tantivy 11 161 681 octets, 3 013 checksums vérifiés ;
- `gmail-index` après la campagne : 3 013 examinés, 0 indexé, 3 013 sautés,
  0 échec de parsing, durée totale 12 ms.

Les captures temporaires ont confirmé la vue Archive, le bouton de
synchronisation, le résumé `Archive à jour · index de recherche à jour` et le
retour visuel à Recherche. Les requêtes et contenus réels n’ont pas été
conservés. L’injection clavier automatisée n’a pas produit de texte fiable
dans cette session Xvfb ; cela ne remet pas en cause les tests clavier
existants ni le rechargement de l’index, mais laisse une validation humaine de
la recherche post-sync à refaire sur un bureau réel.

## Classification

- **Fait vérifié :** la boucle UI peut synchroniser une archive Gmail réelle,
  mettre à jour Tantivy et afficher l’état final sans redémarrer.
- **Fait vérifié :** une seconde synchronisation réelle sans changement ne
  crée pas de nouveau message et laisse l’index stable.
- **Fait vérifié :** une erreur d’index simulée par fixture n’est pas nécessaire
  pour invalider les RAW ; le chemin d’index est dérivé et reconstructible.
- **Décision de projet :** conserver deux espaces seulement : Recherche /
  Consultation et Archive / Synchronisation.
- **Ouvert :** annulation coopérative pendant une full sync longue et
  validation de la saisie post-sync par interaction humaine Wayland.
