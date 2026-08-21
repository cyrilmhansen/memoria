# Slint native workspace demo

Démonstration minimale d’une application Rust/Slint native, destinée à être
compilée depuis les mêmes sources sous Windows et Linux. Elle n’utilise pas
de WebView. Le code métier est volontairement limité à la démonstration :
liste synthétique, recherche, préférences et tâche annulable.

## Commandes

```text
cargo run
cargo test --all-targets
cargo build --release
cargo run --release -- --benchmark
```

`--benchmark` mesure la préparation des données sans ouvrir de fenêtre ; la
mesure d’une fenêtre affichée doit être faite dans une session graphique
Windows/Linux.

## Dépendances Slint

- `slint = 1.17.1` : runtime UI, backend Winit, renderer logiciel et
  accessibilité activée explicitement ; les features par défaut sont
  désactivées pour ne pas tirer le renderer Femtovg/Skia par défaut.
- `slint-build = 1.17.1` : compilation de `ui/app.slint` en code Rust pendant
  le build.
- `fontique = 0.10.0` : feature `fontconfig-dlopen` unifiée avec la
  dépendance transitive de Slint 1.17.1 ; elle est nécessaire pour éviter un
  chemin `pkg-config`/Homebrew figé sous Linux.

Les préférences utilisent uniquement `std::fs` et les variables de plateforme
(`APPDATA`, `XDG_CONFIG_HOME`, `HOME`).

## Vérifications manuelles à exécuter sur une machine graphique

- Tab/Shift-Tab dans recherche, navigation, checkbox, dialogue et tâche ;
  activation par Entrée/Espace et conservation d’un focus visible.
- Navigation dans la liste et sélection au clavier.
- Lecteur d’écran : labels de recherche, liste, items, checkbox, progression
  et dialogue ; Slint expose les rôles disponibles sur la plateforme.
- Redimensionnement continu, fenêtre à 125/150/200 % HiDPI et affichage
  Wayland puis X11 sous Linux.
- Démarrage/annulation de la tâche pendant une recherche et fermeture de la
  fenêtre.
- Relance après modification de chaque préférence.

## Limitations connues

- Le modèle contient 100 000 `SharedString` en mémoire ; le `ListView` ne
  rend que les délégués visibles, mais la préparation d’un filtre reconstruit
  le vecteur sur le thread UI.
- Le renderer logiciel évite le besoin d’un GPU, au prix de performances
  graphiques potentiellement moindres.
- L’accessibilité dépend du backend et du lecteur d’écran de la plateforme ;
  elle est activée côté Slint mais n’est pas certifiée ici par un testeur
  Windows/AT-SPI réel.
- Le conteneur de développement ne fournit pas de compositor utilisable ; la
  mesure GUI et la vérification interactive restent à faire sur Windows/Linux.
