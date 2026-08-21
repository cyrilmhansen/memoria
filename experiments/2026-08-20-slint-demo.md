# Démonstration Slint — 2026-08-20

## Faits vérifiés

- `slint` et `slint-build` sont verrouillés en `1.17.1`.
- `cargo test --all-targets` passe avec 2 tests du dataset.
- `cargo build --release` passe avec la configuration fontconfig dynamique.
- Mesure locale Linux, release, sans ouverture de fenêtre :

  - binaire : `26 976 736` octets (`stat -c '%s' target/release/slint-apps-workspace`) ;
  - `startup_to_dataset_ms=13` ;
  - `memory_without_items_kb=5604` ;
  - `populate_100000_ms=13` ;
  - `memory_with_100000_items_kb=12896`.

## Commandes reproductibles

```text
cargo fmt --all
cargo test --all-targets
cargo build --release
stat -c '%n %s bytes' target/release/slint-apps-workspace
./target/release/slint-apps-workspace --benchmark
```

## Échecs et limites

- Sans `RUST_FONTCONFIG_DLOPEN=1`, le build Linux cherche `fontconfig.pc` via
  pkg-config et dépend du chemin d’installation, notamment Homebrew.
- Le mode graphique lancé par `xvfb-run` n’est pas mesurable dans ce
  conteneur : aucun serveur X n’a pu être ouvert et aucun compositor Wayland
  n’est disponible. Les chiffres ci-dessus ne sont donc pas une mesure RSS de
  la fenêtre affichée.
- Le target Windows n’est pas installé dans cet environnement (`rustup
  target list --installed` ne le montre pas). La compilation Windows et les
  vérifications clavier/HiDPI/lecteur d’écran doivent être exécutées sur une
  machine Windows.

## Hypothèses restantes

- La virtualisation de `ListView` et les rôles AccessKit se comporteront comme
  attendu avec les lecteurs d’écran Windows et AT-SPI réels ; à confirmer par
  la checklist de [README.md](../README.md).
