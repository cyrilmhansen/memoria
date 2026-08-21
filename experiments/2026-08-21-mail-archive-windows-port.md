# Memoria — port Windows x86-64 MSVC

Date : 2026-08-21  
Statut : build cross-compilé validé ; validation native Windows encore ouverte

## Build

Outils utilisés : `cargo-xwin 0.23.1`, cible Rust
`x86_64-pc-windows-msvc`, Slint 1.17.1, Tantivy 0.26.1, rusqlite 0.40.2 et
rfd 0.17.2.

Commande :

```text
cargo xwin build --release --target x86_64-pc-windows-msvc \
  -p mail-archive-experiment --bin mail-archive-app
```

Résultat :

```text
target/x86_64-pc-windows-msvc/release/mail-archive-app.exe
PE32+ executable for MS Windows, x86-64, GUI
26455552 bytes
```

Le binaire release est désormais `GUI` et n’ouvre pas de console parasite.
Le mode debug conserve son comportement de diagnostic. Le changement de code
nécessaire est uniquement l’attribut `windows_subsystem` du binaire.

Le workflow [`.github/workflows/windows.yml`](../.github/workflows/windows.yml)
exécute `cargo check --workspace`, `cargo test --workspace`, le build standard
et le build avec `RUSTFLAGS=-C target-feature=+crt-static`. Il publie les deux
variantes sous les artifacts `memoria-windows-x86_64` et
`memoria-windows-x86_64-static`. Aucun run GitHub Actions n’a encore été
exécuté dans ce dépôt local ; le workflow est donc préparé mais la CI native
n’est pas déclarée validée.

## Tests

```text
cargo test --workspace -q
cargo xwin test --target x86_64-pc-windows-msvc \
  -p mail-archive-experiment --lib
```

Les tests Linux passent. Les 18 tests de bibliothèque compilés MSVC et
exécutés via le mécanisme Wine passent également, notamment configuration,
archive, checksums/reprise, parsing, Tantivy et fixtures de synchronisation.
Cela vérifie la portabilité de la logique et de SQLite/Tantivy dans cet
environnement, pas l’ergonomie Windows native.

## Dépendances runtime observées

L’exécutable importe les DLL système Windows usuelles (`kernel32`, `user32`,
`dwrite`, `uiautomationcore`, `shell32`, `ws2_32`, etc.) et
`VCRUNTIME140.dll` ainsi que les API Universal CRT. Le runtime Visual C++
Redistributable correspondant doit donc être présent sur une machine Windows
qui ne fournit pas déjà ces composants. Aucun DLL Rust, SQLite, Tantivy ou
fontconfig externe n’est ajouté à côté de l’exécutable : SQLite est compilé
avec `rusqlite` bundled et Slint utilise son renderer logiciel.

## Variante CRT statique

Commande :

```text
RUSTFLAGS="-C target-feature=+crt-static" cargo xwin build --release \
  --target x86_64-pc-windows-msvc \
  -p mail-archive-experiment --bin mail-archive-app
```

La compilation réussit. Le résultat mesure `26 625 536` octets, contre
`26 455 552` octets pour la variante dynamique. Les 18 tests de bibliothèque
MSVC avec le même `RUSTFLAGS` passent également via Wine.

L’import table de la variante statique ne contient plus `VCRUNTIME140.dll` ni
les DLL Universal CRT. Elle conserve uniquement les DLL système Windows
nécessaires (`kernel32`, `user32`, `dwrite`, `uiautomationcore`, réseau,
COM, etc.). La variante CRT statique est donc une candidate raisonnable pour
un EXE portable sans redistributable Visual C++, sous réserve d’une validation
sur Windows réel. Elle n’est pas encore celle publiée par la CI ; le workflow
publie actuellement la build release native standard.

## Audit des hypothèses Linux

- Les chemins de configuration passent par `dirs::config_dir()` ; aucun
  `%APPDATA%` n’est construit manuellement.
- Le code produit utilise `PathBuf`/`Path`, pas un séparateur `/` manuel pour
  les chemins. Les `/tmp` restants appartiennent aux commandes expérimentales,
  aux valeurs de fixtures et au CLI de génération, pas au démarrage produit.
- Il n’y a pas d’appel shell dans Memoria. `webbrowser` ouvre le navigateur
  par son API multiplateforme.
- Le callback OAuth utilise une écoute loopback `127.0.0.1`; il ne dépend ni de
  Wayland ni de X11 et reste adapté à Windows.
- `rfd` fournit les dialogues natifs de dossier/fichier, sans branche Windows
  dans le contrôleur métier.
- Aucun code produit ne dépend de permissions Unix, symlinks, suppression d’un
  fichier ouvert ou d’un rename POSIX particulier.
- Les ressources d’archive sont fermées par les propriétaires Rust avant la
  fermeture normale ; les tests de lecture, indexation et reconstruction sont
  passés sous le chemin MSVC/Wine.

## Wine

Le lancement de l’EXE sous Wine a atteint Slint mais a échoué dans
`i-slint-renderer-software` lors de la découverte de polices système (`None`
déballé). Les messages Wine indiquent également des composants graphiques
incomplets. Cela révèle une limite de l’environnement Wine sans démontrer un
défaut Windows : une machine Windows réelle fournit DirectWrite et ses polices.
Wine n’est donc pas utilisé comme validation UX, HiDPI, dialogues rfd ou
AccessKit Windows.

## Portabilité de l’archive

Les tests MSVC/Wine valident le chemin logique de lecture des segments,
SQLite, checksums, index et reconstruction. Une archive RAW + catalogue reste
le support portable et l’index Tantivy reste dérivé/reconstructible. La
compatibilité byte-for-byte d’un répertoire Tantivy entre plateformes n’est
pas figée par cette expérience ; en cas de recommandation contraire de
Tantivy, il devra être reconstruit sous Windows sans toucher aux RAW.

## Ce qui reste à vérifier sur Windows réel

- démarrage sans argument, chemins Unicode/espaces et configuration standard ;
- ouverture/création d’archive et dialogues rfd ;
- fenêtres, menus, clavier, molette, copie et HiDPI 100/125/150/200 % ;
- inspection AccessKit/UI Automation ;
- OAuth loopback et synchronisation Gmail réelle ;
- alternance d’une même archive entre Linux et Windows.

## Décision

Le même crate et les mêmes fichiers Slint produisent désormais un EXE Windows
x86-64 MSVC. La première cible de compilation Linux/Windows est validée. La
validation UX et OAuth Windows doit être faite sur une machine Windows réelle,
sans ajouter de fork ni de code métier conditionnel par plateforme.
