# Validation UX Wayland — première UI mail

## Périmètre

Validation graphique de `mail-archive-app` sur l'archive locale existante,
sans accès Gmail et sans modification de l'archive, de l'index ou du
connecteur. Aucun contenu réel n'est copié dans ce rapport.

Commandes reproductibles :

```text
wayland-info
cargo build -p mail-archive-experiment --bin mail-archive-app
target/debug/mail-archive-app --archive .local/gmail-real-20260820
kscreen-doctor -o
spectacle -b -a -n -o /tmp/mail-archive-active.png
```

## Environnement réellement observé

- session `wayland` avec compositeur KDE/Wayland accessible après autorisation
  d'accès au socket de session ;
- backend choisi naturellement par Slint/Winit : Wayland, sans Xvfb et sans
  forcer X11 ;
- deux sorties 2560×1440 à environ 75 Hz, facteur KDE observé à 1 ; une sortie
  est pivotée ;
- la fenêtre native s'est ouverte et est restée stable ; fermeture par arrêt
  normal du processus sans erreur rapportée.

La capture temporaire utilisée pour l'inspection est restée dans `/tmp` et
n'est pas versionnée.

## Observations

À la taille desktop disponible, les polices sont nettes, le champ de recherche
est visiblement focalisé au démarrage, les deux panneaux sont lisibles et le
panneau de lecture occupe la plus grande largeur. Aucun artefact MIME ou
clipping n'a été observé dans l'état initial.

Les tailles 800×600, 1024×768 et 1500×900 ont déjà été exercées sous Xvfb :
la fenêtre reste stable, mais la disposition côte à côte devient dense sous
800 px. Cette limite était déjà connue ; aucun défaut suffisamment manifeste
n'a justifié un breakpoint ajouté pendant cette validation.

La sélection souris et la navigation flèches/Entrée ont été vérifiées sous
Xvfb avec fixtures d'archive réelle, ainsi que Ctrl+F, Esc et le
redimensionnement. Le lancement Wayland direct confirme le rendu natif, mais
la session ne fournit ni `wtype` ni `ydotool` pour injecter proprement des
touches dans une application Wayland depuis la ligne de commande.

Le facteur HiDPI distinct n'a pas pu être vérifié : les sorties disponibles
sont configurées à 1. Le test ne permet donc pas de conclure sur un facteur
Wayland supérieur à 1. L'arbre d'accessibilité n'a pas pu être inspecté par
un outil AT-SPI disponible ; les rôles et labels Slint restent déclarés et la
navigation clavier validée sous Xvfb.

## Défauts et décision

Aucune correction de code n'a été nécessaire : aucun défaut manifeste n'a
été reproduit dans le chemin réellement testé. Les limites ouvertes sont la
difficulté de vérifier l'accessibilité système et une disposition dense à
petite largeur ; elles ne bloquent pas la recherche/lecture quotidienne sur
un écran desktop.

Les mesures contrôleur déjà établies restent : ouverture de l'index environ
4,2 ms, recherche environ 2,2 ms et lecture/parsing environ 2,8 ms. Elles ne
constituent pas un nouveau microbenchmark Wayland ; elles expliquent
pourquoi aucune latence perceptible n'a été recherchée artificiellement.

**Faits vérifiés :** l'application démarre directement dans une session KDE
Wayland réelle, rend correctement sa fenêtre initiale et conserve le RAW hors
de la couche UI.

**Limites :** interaction clavier/souris automatisée et arbre AT-SPI non
vérifiés sur Wayland dans cet environnement ; HiDPI > 1 non disponible.

**Décision de projet :** considérer la première UI suffisamment utilisable
pour rechercher et lire des messages sur un bureau Linux desktop. La
prochaine fonctionnalité doit être choisie à partir de l'usage, pas d'un
polissage UI supplémentaire.
