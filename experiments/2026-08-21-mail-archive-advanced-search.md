# Mail archive — recherche structurée

## Périmètre et audit initial

Cette expérience ajoute une requête structurée au moteur Tantivy dérivé sans
modifier les frames RAW, le catalogue SQLite physique ou le connecteur Gmail.
Les documents de départ contenaient déjà `doc_id`, date, expéditeur,
destinataires, sujet, corps, compte, labels textuels, types et compteur de
pièces jointes. `doc_id`, date et compteur étaient stockés/indexés ; les
champs textuels étaient stockés et indexés avec le tokenizer Tantivy par
défaut. La date n'était pas un fast-field.

Le lecteur UI affichait au plus 50 documents. Le post-filtrage après ces 50
documents aurait pu perdre une réponse valide ; il n'est donc pas utilisé.

## Contrat retenu

```text
SearchRequest {
    text: String,
    from: Option<String>,
    to: Option<String>,
    date_from: Option<i64>,   # Unix ms, borne incluse
    date_to: Option<i64>,     # Unix ms, borne exclusive
    attachment: All | With | Without,
    attachment_mime: Option<String>,
    labels: Vec<String>,
    limit: usize,
}
```

Tous les champs présents se combinent par AND. Les labels ont une sémantique
« tous les labels sélectionnés ». `application/pdf` est un MIME exact ;
`image/*` utilise la famille MIME. Un message est considéré comme ayant une
pièce jointe selon la même définition que l'UI : disposition `attachment` ou
partie nommée téléchargeable, en excluant une ressource inline/CID décorative.

Une requête sans texte mais avec filtres est valide. Elle est triée par date
décroissante. Une requête vide sans filtre ne charge toujours pas toute
l'archive. Avec du texte, BM25 reste le classement ; les filtres ne modifient
pas le score.

## Répartition Tantivy / catalogue

Le schéma dérivé a reçu :

- `has_attachment`, champ u64 indexé, stocké et fast-field ;
- `attachment_mime`, tokenizer raw, valeurs MIME exactes ;
- `attachment_family`, tokenizer raw, par exemple `image` ;
- `label_exact`, tokenizer raw, une valeur par label.
- `sender_filter` et `recipient_filter`, tokenizer raw, avec les fragments
  normalisés et l'adresse complète lorsqu'elle est disponible.

La date est maintenant aussi fast-field pour le tri sans texte. Expéditeur et
destinataire restent des champs textuels Tantivy : chaque fragment fourni est
analysé par le tokenizer du champ et tous les fragments non vides sont
intersectés. Cela accepte une adresse ou un nom affiché sans exiger la forme
MIME complète. Les données de restitution continuent de venir des documents
Tantivy et du catalogue ; aucune colonne SQLite n'a été ajoutée.

L'ancien index est détecté par l'absence des nouveaux champs. Comme il est
dérivé, son répertoire et son checkpoint dérivé sont reconstruits depuis RAW
+ catalogue. Si l'application ouvre un ancien index non compatible, elle
déclenche cette reconstruction avant de présenter la recherche ; une erreur
de parsing est remontée au lieu d'effacer l'archive.

## UI

La barre texte reste dominante. Un bouton `Filtres` ouvre un panneau compact
avec expéditeur, destinataire, bornes calendaires, tri-état des pièces
jointes, MIME et les labels effectivement présents dans le catalogue. Chaque
modification relance le même chemin de recherche ; la remise à zéro vide tous
les filtres. Ctrl+F et le comportement de recherche texte restent inchangés.

## Tests déterministes

Une fixture couvre :

- texte + expéditeur + borne haute exclusive ;
- PDF exact ;
- présence et absence de pièce jointe ;
- plusieurs labels ;
- ressource inline/CID qui ne devient pas une pièce jointe utilisateur ;
- tri récent sans texte ;
- absence de faux négatif dû à une limite préalable.

Le smoke test conditionnel sur `.local/gmail-real-20260820` ouvre l'index et
exécute hors ligne un filtre MIME et un label sans imprimer de données. Tous
les tests du workspace passent.

## Mesures

Environnement Linux x86-64, Tantivy 0.26.1, build local. Une reconstruction
propre de l'archive Gmail réelle a traité 3 013 documents, sans échec MIME,
en 8 421 ms dans cette exécution ; l'index fait 11 225 345 octets. L'ancien
ordre de grandeur documenté était 11 148 600 octets : les champs structurés
ajoutent environ 76,7 KiB (0,7 %). Ces temps ne sont pas une comparaison
scientifique de cache avec l'ancienne campagne, mais vérifient le coût global.

Le benchmark synthétique réutilisé à 100 000 messages a donné :

| mesure | résultat |
|---|---:|
| archive | 5 606 662 572 octets |
| Tantivy | 36 426 915 octets |
| indexation Tantivy | 40 587 ms |
| recherche Tantivy, workload lexical p50 | 1 474 µs |
| recherche Tantivy, workload lexical p95 | 31 296 µs |

Cette campagne synthétique contient des messages mais pas un catalogue Gmail
avec une distribution représentative de labels et de MIME structurés ; elle
ne mesure donc pas la sélectivité réelle de ces filtres. Une tentative 1M a
été interrompue par la saturation du tmpfs après environ 13 Go de fichiers
temporaires. La mesure 1M précédente (76,2 s et environ 239 MiB RSS) reste
réutilisable pour la loi d'échelle générale, pas comme benchmark des filtres
Gmail structurés.

## Faits, hypothèses, décisions

- **Fait vérifié :** les contraintes structurées sont évaluées dans Tantivy
  avant la collecte des 50 résultats ; une requête sélective ne perd donc pas
  ses résultats derrière une limite de post-filtrage.
- **Fait vérifié :** les tests et la reconstruction locale réelle passent,
  sans lecture réseau ni modification RAW.
- **Décision de projet :** conserver Tantivy comme moteur de retrieval et
  limiter SQLite au catalogue/à la restitution, tant qu'une mesure à une
  échelle supérieure ne montre pas le contraire.
- **Hypothèse ouverte :** la stratégie de labels exacts et de MIME raw doit
  être mesurée sur un corpus de plusieurs millions contenant une distribution
  Gmail réaliste ; le benchmark synthétique actuel ne permet pas de le faire.

## Limites

Les labels sont actuellement des IDs Gmail présentés localement ; aucune
taxonomie conviviale n'est ajoutée. Les fragments `from`/`to` reposent sur le
tokenizer Tantivy et ne promettent pas une recherche d'adresse RFC complète.
Il n'y a ni syntaxe Gmail, ni OR général, ni stemming, ni recherche dans le
contenu des pièces jointes, ni comptage global au-delà de la limite affichée.

La prochaine expérience utile est un benchmark ciblé sur un corpus de
plusieurs millions où les labels, MIME et dates sont générés avec des
distributions explicites ; il doit mesurer la mémoire et la latence des
requêtes sélectives, sans relancer une campagne FTS5/Tantivy générale.
