# Retour manuel — corrections de la première UI

## Périmètre

Cette passe reproduit les défauts signalés lors de la première utilisation
réelle, puis vérifie l'état initial, une recherche longue, la sélection
clavier et un message dont le corps dépasse le viewport. Les captures sont
restées dans `/tmp` ; aucun contenu réel n'est conservé dans le dépôt.

## Causes vérifiées et corrections

| observation | cause | correction |
|---|---|---|
| contenu des lignes absent | le délégué `ListView` n'avait pas de largeur explicite | largeur du délégué et de sa zone tactile liée à `parent.width` |
| traces de texte dans la ligne d'état | texte dans une ligne fixe sans alignement vertical explicite | `vertical-alignment: center` sur le compteur et l'état |
| fenêtre apparemment fixe | `width`/`height` fixaient la taille préférée sans contraintes explicites | `preferred-width/height` et `min-width/min-height` |
| corps long mal dimensionné | le texte enfant n'imposait pas sa largeur de reflow | largeur du texte liée à la zone du `ScrollView` |
| état initial ressemblant à une archive vide | compteur par défaut et panneau gauche neutre | message explicite demandant une recherche |
| résultat conservé après effacement | `clear-search` ne vidait que le modèle | effacement simultané du message sélectionné, des résultats et de la requête |

Le placement initial dans le coin supérieur droit n'est pas une décision de
l'application. Sous Wayland, le placement est contrôlé par le compositeur ;
aucune position n'est imposée par le code. Le titre et l'identité minimale ont
été renommés `Memoria — Archive` / `Memoria`. Slint 1.17.1 n'expose pas ici de
propriété simple d'icône de fenêtre portable ; aucune icône artificielle n'a
été ajoutée.

## Politique de lecture

Le chrome du lecteur (sujet, date, expéditeur, destinataires, pièces jointes)
reste hors du `ScrollView`. Seul le corps dérivé défile. Le `ScrollView` Slint
gère verticalement les messages longs et horizontalement les lignes qui ne
peuvent pas être refluées. Les parties HTML continuent d'être converties en
texte : balises, scripts et styles ne sont pas affichés. Cette décision évite
WebView et permet un reflow robuste, mais ne promet pas de préserver la mise
en page visuelle d'une newsletter ; un rendu HTML fidèle reste une décision
produit ouverte.

Aucun menu contextuel n'a été ajouté : la première UI ne propose encore
aucune action locale (export, copie structurée, marquage) dont le menu
améliorerait réellement l'usage.

## Vérifications reproductibles

```text
cargo fmt --all
cargo test --workspace
cargo check --workspace
cargo build -p mail-archive-experiment --bin mail-archive-app

# Xvfb, recherche clavier, capture hors dépôt
xvfb-run -a -s '-screen 0 1600x1000x24' sh -c '
  export WAYLAND_DISPLAY=; export WINIT_UNIX_BACKEND=x11
  target/debug/mail-archive-app --archive .local/gmail-real-20260820 & app=$!
  sleep 2
  win=$(xdotool search --onlyvisible --name "Memoria.*" | head -1)
  xdotool windowfocus --sync "$win"
  xdotool key --window "$win" ctrl+f
  xdotool type --window "$win" --delay 20 <requête de test>
  sleep 2
  import -window root /tmp/mail-ui-results.png
  xdotool key --window "$win" Down
  xdotool key --window "$win" Return
  sleep 2
  import -window root /tmp/mail-ui-message.png
  kill "$app"
'
```

Résultats observés :

- état initial lisible à 800×600 et à la taille desktop Wayland ;
- 50 lignes visibles avec correspondant, date, sujet et défilement à 1600×1000 ;
- sélection clavier et ouverture du message fonctionnelles ;
- corps long reflué dans le panneau, chrome fixe ;
- 800×600 reste utilisable, avec une densité plus élevée mais sans texte
  verticalement tronqué ;
- lancement Wayland direct vérifié après correction, avec fenêtre native et
  titre `Memoria — Archive`.

Le facteur HiDPI supérieur à 1 et l'arbre AT-SPI restent non vérifiés dans la
session disponible. Le test Xvfb vérifie le layout logique, pas le rendu d'un
écran physique.

## Décision

**Faits vérifiés :** les défauts signalés étaient des défauts de contraintes
de layout et d'état, non des problèmes de Tantivy ou de données ; ils sont
corrigés et les tests existants passent.

**Décision de projet :** l'état vide est search-first mais l'explique ;
effacer signifie maintenant effacer la recherche, les résultats et la lecture
courante. Le rendu texte reste le compromis actuel sans WebView.

**Ouvert :** rendu HTML visuel fidèle, icône portable, accessibilité système
et HiDPI >1.
