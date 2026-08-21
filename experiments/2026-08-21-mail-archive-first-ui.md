# Mail archive — première interface Slint de recherche

## Périmètre

Cette étape ajoute une première application produit hors ligne sous
`projects/mail-archive`. Elle ouvre une archive existante, utilise
`GmailSearchIndex`, affiche jusqu’à 50 résultats et lit le RAW sélectionné
pour produire une vue texte dérivée.

Elle ne synchronise pas Gmail et ne modifie ni les segments RAW, ni le
catalogue, ni l’index Tantivy.

Commandes reproductibles :

```text
cargo fmt --all
cargo test --workspace
cargo build -p mail-archive-experiment --bin mail-archive-app
cargo run -p mail-archive-experiment --bin mail-archive-app -- \
  --archive .local/gmail-real-20260820
cargo run -p mail-archive-experiment --bin mail-archive-app -- \
  --archive .local/gmail-real-20260820 --benchmark
```

Les essais graphiques Linux ont utilisé Xvfb avec une surface 1600×1000 et
`WAYLAND_DISPLAY` désactivé. Aucun contenu n’a été enregistré dans le dépôt.

## Architecture

```text
Slint MailWindow
    → contrôleur Rust dans mail-archive-app
    → GmailSearchIndex / read_archived_raw
    → Tantivy + catalogue + segments RAW
```

La couche Slint ne connaît ni Tantivy, ni SQLite, ni la segmentation. La
recherche reste synchrone car sa latence mesurée est de quelques millisecondes.
La lecture et le parsing du message sélectionné s’effectuent dans un thread
de fond et reviennent dans la boucle Slint avec `invoke_from_event_loop`.

## Mesures locales

Benchmark contrôleur sur l’archive réelle, sans accès Gmail :

| mesure | résultat |
|---|---:|
| ouverture index | 4 187 µs |
| recherche bornée à 50 | 2 172 µs |
| lecture + parsing du message sélectionné | 2 770 µs |
| résultat de recherche | 1 |
| parsing réussi | oui |

Sous Xvfb, le temps lancement → fenêtre détectée par le gestionnaire X était
`194 ms`. Ce chiffre inclut le lancement du binaire et l’initialisation du
backend ; ce n’est pas une mesure d’un démarrage release sur une machine
physique.

## Fonctionnalités vérifiées

- recherche réactive à chaque modification du champ ;
- validation par Entrée ;
- bouton d’effacement ;
- focus initial sur la recherche ;
- sélection souris ;
- flèches haut/bas puis Entrée pour sélectionner et ouvrir ;
- affichage date, correspondant, sujet, extrait et indicateur de pièce jointe ;
- affichage date, From, To, Subject et corps texte dérivé ;
- fallback d’erreur si lecture ou parsing échoue ;
- état neutre pour une recherche vide ;
- redimensionnement testé de 800×600 à 1500×900 sans crash ;
- fenêtre native Slint, sans WebView ;
- rôles accessibles et labels sur la fenêtre, la recherche et les lignes.

Le test visuel a montré qu’une recherche locale affiche rapidement une ligne,
et que la sélection clavier ouvre le message dans le panneau droit. Les
résultats et le message réels ne sont pas reproduits ici.

## Limites observées

- La petite largeur n’a pas encore de layout vertical dédié : les deux
  panneaux restent côte à côte et peuvent devenir denses sous 800 px.
- Le test HiDPI réalisé ici est limité au renderer logiciel/Xvfb ; aucun écran
  physique Windows ou Wayland n’a été utilisé.
- L’HTML est converti par la logique heuristique existante : balises,
  scripts/styles sont retirés, mais le rendu n’est pas celui d’un navigateur.
- Les citations et signatures ne sont pas supprimées.
- La navigation clavier est implémentée au niveau de la FocusScope, mais la
  sélection visuelle ne virtualise pas encore une liste importante ; la limite
  actuelle de 50 résultats rend ce choix suffisant.
- Aucun snippet sensible n’est écrit dans les logs ou rapports ; les données
  affichées dans la fenêtre restent locales à l’utilisateur.

## Tests et décision

`cargo test --workspace` passe avec les 14 tests du prototype mail et les 2
tests du workspace racine. Les fixtures MIME et l’API de recherche restent
indépendantes de Slint.

**Faits vérifiés :** l’application ouvre l’archive Gmail réelle hors ligne,
recherche, sélectionne et affiche un message ; la latence contrôleur reste
perceptuellement immédiate pour cette archive.

**Décision de projet :** ne pas optimiser Tantivy ni ajouter de moteur de
requêtes à ce stade. La prochaine étape produit la plus utile est une courte
itération UX sur écran réel Windows/Linux/Wayland, notamment la densité à
petite largeur et la vérification accessibilité/HiDPI.
