# Mail archive — Tantivy sur l’archive Gmail réelle

## Objectif et périmètre

Cette expérience construit un index Tantivy dérivé des 3 012 RAW Gmail déjà
archivés. Aucun accès réseau n’est effectué. Les segments RAW et le catalogue
existant restent inchangés ; l’index et son checkpoint sont écrits sous
`<archive>/derived/`.

Le chemin testé est :

```text
segments RAW → catalogue SQLite → lecture ciblée → mailparse → Tantivy
→ doc_id → catalogue → frame RAW
```

Commandes reproductibles :

```text
cargo fmt --all
cargo test --workspace
cargo run -p mail-archive-experiment -- gmail-index \
  --archive .local/gmail-real-20260820
cargo run -p mail-archive-experiment -- search \
  --archive .local/gmail-real-20260820 'invoice'
```

La commande `search` est destinée à l’utilisation locale interactive. Aucun
résultat, sujet, adresse ou contenu n’est copié dans ce rapport.

## Indexation initiale

Environnement : Linux x86_64, build de développement local, Tantivy 0.26.1,
mailparse 0.16.1, archive de 3 012 messages.

| mesure | résultat |
|---|---:|
| messages examinés | 3 012 |
| documents indexés | 3 012 |
| échecs de parsing | 0 |
| lecture archive cumulée | 405 768 µs |
| parsing cumulé | 4 945 313 µs |
| indexation/commit | 15 475 379 µs |
| durée totale | 15 523 ms |
| RSS maximale sur reconstruction propre | 94 984 KiB |
| index Tantivy | 11 148 600 octets |

La mesure RSS inclut le processus Cargo ; elle donne un plafond opérationnel,
pas une RSS isolée du binaire release.

## Champs et parsing

Chaque document contient un `doc_id` stocké et indexé, la date, l’expéditeur,
les destinataires, le sujet, le corps texte, le compte source, les labels
Gmail, les types de pièces jointes détectés et leur nombre.

`mailparse 0.16.1` est utilisé pour parser les headers, décoder les feuilles
texte et parcourir les multiparts. Pour `text/html`, le prototype retire les
balises ainsi que les blocs `script`/`style`, conserve un texte aplati et
décode seulement quelques entités HTML courantes. Il ne se comporte pas comme
un navigateur, ne traite pas le CSS visuellement et ne retire pas les
signatures ou citations.

Une fixture MIME vérifie : HTML visible recherché, script exclu, headers
expéditeur/destinataire lus et pièce jointe PDF comptée. Les RAW réels ne sont
jamais réécrits par ce parsing.

Le tokenizer Tantivy par défaut a accepté les tests de texte Unicode exécutés
hors ligne, notamment caractères accentués, apostrophes, traits d’union et
chiffres. Aucun stemming, détection linguistique ou normalisation métier n’est
introduit à ce stade.

## Recherche locale

Les requêtes de workload sont des classes synthétiques et ne sont pas des
valeurs extraites du compte. Les mesures suivantes sont prises avec un index
ouvert et une connexion catalogue réutilisés ; les résultats sont limités à
20 documents.

| classe | résultats | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| terme rare | 1 | 349 µs | 664 µs | 823 µs |
| terme fréquent | 20 | 1 825 µs | 1 921 µs | 3 770 µs |
| plusieurs termes | 20 | 1 338 µs | 1 425 µs | 3 376 µs |
| expression exacte | 20 | 1 277 µs | 1 353 µs | 3 008 µs |
| expéditeur | 20 | 884 µs | 934 µs | 2 580 µs |
| destinataire | 0 | 59 µs | 69 µs | 85 µs |
| date seule | 20 | 11 058 µs | 13 513 µs | 14 720 µs |
| texte + date | 1 | 5 054 µs | 5 752 µs | 6 030 µs |
| texte + expéditeur | 20 | 484 µs | 522 µs | 604 µs |
| label | 20 | 460 µs | 505 µs | 1 517 µs |
| sans résultat | 0 | 262 µs | 272 µs | 388 µs |

Ouverture de l’index : 2 411 µs. Première requête : 1 607 µs. Les temps
lexicaux sont compatibles avec une interaction locale immédiate à cette
échelle. Les plages de dates sont nettement plus lentes dans cette
implémentation et devront être reconsidérées si elles deviennent centrales.

La syntaxe minimale accepte les champs Tantivy usuels (`sender:`, `to:` ou
`recipients:`, `subject:`, `label:`), les expressions entre guillemets et les
bornes `after:YYYY-MM-DD` / `before:YYYY-MM-DD`. Elle ne constitue pas encore
un langage de requête produit.

## Index incrémental

Le checkpoint dérivé `derived/tantivy-state.sqlite` conserve, pour chaque
`doc_id`, la localisation de frame, la taille de frame, les labels et l’état
source. Une frame et ses métadonnées inchangées sont sautées ; une frame ou
des labels modifiés entraînent une mise à jour Tantivy ; un message devenu
non-présent est retiré de l’index dérivé.

Relance immédiate sur l’archive réelle :

```text
examined=3012
indexed=0
skipped=3012
removed=0
parse_failures=0
archive_read_us=0
parse_us=0
index_us=2502
wall_ms=32
```

Le checkpoint n’est pas couplé à `historyId` Gmail : il reflète uniquement
l’état local de l’archive et de ses métadonnées.

## Reconstruction

Sur une copie expérimentale de l’archive, suppression de `derived/`, puis
reconstruction complète :

```text
examined=3012
indexed=3012
skipped=0
removed=0
parse_failures=0
index_bytes≈11.1 MB
```

La recherche fonctionne après reconstruction. L’archive reste lisible sans
index : les frames, checksums et le catalogue sont les seules sources
nécessaires à la lecture RAW.

## Faits, limites et décisions

- **Fait vérifié :** le pipeline réel archive→catalogue→RAW→parsing→Tantivy
  fonctionne sur 3 012 messages sans échec MIME.
- **Fait vérifié :** la relance ne réindexe aucun document inchangé.
- **Fait vérifié :** l’index est reconstructible et n’est pas requis pour
  accéder aux RAW.
- **Limite vérifiée :** les plages de dates sont plus lentes que les requêtes
  lexicales et leur implémentation reste une baseline.
- **Limite vérifiée :** l’extraction HTML est volontairement heuristique ; le
  texte exact rendu par un client mail n’est pas garanti.
- **Décision de projet :** conserver la séparation récupération de candidats /
  classement BM25 ; ne pas ajouter d’embeddings, de reranker ou d’index
  vectoriel.
- **Décision de projet :** exposer à la future UI `GmailSearchIndex::open`,
  `search` et `read_archived_raw`, sans dépendance à Slint ni au CLI.

## Références

- [Tantivy 0.26.1](https://docs.rs/tantivy/0.26.1/tantivy/)
- [mailparse 0.16.1](https://docs.rs/mailparse/0.16.1/mailparse/)

## Conclusion

Sur 3 012 messages, Tantivy fournit déjà une recherche lexicale locale
interactive et un chemin de reconstruction propre. La prochaine étape
raisonnable est une première UI Slint de recherche, avec chargement différé du
RAW sélectionné. L’indexation multilingue avancée et l’optimisation des dates
restent ouvertes mais ne bloquent pas cette UI.
