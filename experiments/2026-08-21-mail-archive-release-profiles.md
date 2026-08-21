# Expérience — profils release et compression de Memoria

Date : 2026-08-21

## Périmètre

Mesure de `mail-archive-app` sans changement de dépendance ni de fonctionnalité.
Les builds Linux ont été effectués dans des répertoires `CARGO_TARGET_DIR`
séparés et vierges. Les builds Windows sont des cross-builds MSVC statiques
avec `cargo xwin`; leurs temps ne sont pas strictement comparables aux builds
Linux, car le cache xwin et les artefacts Cargo ont été réutilisés.

Commandes représentatives :

```text
cargo build --release -p mail-archive-experiment --bin mail-archive-app
cargo test --workspace --release
RUSTFLAGS="-C target-feature=+crt-static" cargo xwin build --release \
  --target x86_64-pc-windows-msvc -p mail-archive-experiment --bin mail-archive-app
```

Les variantes ont été injectées uniquement par `cargo --config` pendant la
mesure. Le profil release retenu ensuite est visible dans le `Cargo.toml` de
la racine.

## Résultats

Tailles en octets ; les builds Windows utilisent le CRT statique.

| variante | Linux | build Linux | Windows | build Windows | tests release | smoke Linux |
|---|---:|---:|---:|---:|---|---|
| actuelle | 42 363 672 | 125,09 s | 26 625 536 | 11,86 s* | OK, 21 tests | OK, processus vivant 3 s |
| `strip=true` | 34 310 624 | 119,32 s | 26 625 536 | 85,13 s | OK, 21 tests | OK, processus vivant 3 s |
| `strip=true`, `lto="thin"` | 34 816 640 | 112,58 s | 27 527 168 | 77,25 s | OK, 21 tests | OK, processus vivant 3 s |
| LTO complet, codegen 1 | 28 527 584 | 197,75 s | 24 065 024 | 145,88 s | OK, 21 tests | OK, processus vivant 3 s |
| précédent + `panic="abort"` | 26 134 368 | 179,51 s | 19 560 448 | 116,70 s | OK, 21 tests | OK, processus vivant 3 s |

\* Le build Windows actuel était déjà incrémental ; ce temps n'est donc pas
une mesure de compilation propre comparable aux autres.

Constats vérifiés :

- `strip=true` apporte un gain Linux important mais aucun gain Windows dans
  cette configuration statique.
- Thin LTO n'apporte aucun gain de taille ; il est même légèrement plus gros.
- LTO complet + une unité de code réduit la taille de 32,7 % sous Linux et de
  9,6 % sous Windows par rapport à la baseline. Le coût est un temps de build
  sensiblement supérieur.
- `panic="abort"` réduit encore la taille (38,3 % Linux, 26,5 % Windows),
  mais change le comportement en cas de panic non récupérée. Memoria ne met pas
  en œuvre `catch_unwind`; ses erreurs métier passent par `Result`, et les
  tests release sont passés. Cela ne constitue toutefois pas une validation
  suffisante pour imposer ce changement de contrat au profil par défaut.
- Aucun lancement GUI ne s'est terminé spontanément : sous Xvfb, le processus
  est resté vivant pendant trois secondes, ce qui valide le smoke test mais ne
  mesure pas le temps jusqu'à fenêtre visible. Aucune validation Windows
  interactive n'a été faite.

Le profil par défaut est donc fixé à `strip=true`, LTO complet et
`codegen-units=1`, sans `panic="abort"`.

## Compression de l'EXE Windows candidat

Le fichier source est le véritable profil retenu : CRT statique,
`strip=true`, LTO complet et `codegen-units=1`, sans `panic="abort"`. Il fait
24 065 024 octets. Les artefacts corrigés sont dans
`.local/memoria-packaging-retained/`, ignoré par Git.

| forme | taille | temps de production | validation |
|---|---:|---:|---|
| EXE brut | 24 065 024 | — | présent |
| ZIP `-9` | 10 797 492 | 2,67 s | `zip -T` OK |
| 7z `-mx=9` | 7 681 320 | 4,57 s | `7z t` OK |
| UPX `-9` | 10 656 256 | 7,89 s | `upx -t` OK |

ZIP et 7z sont des compressions de transport : l'utilisateur doit extraire
l'EXE avant de le lancer. UPX compresse l'exécutable lui-même et son résultat
reste directement lançable. Les tailles ne décrivent donc pas exactement le
même scénario de distribution.

UPX reste candidat non décidé. Il n'a pas encore été validé nativement sous
Windows pour le démarrage réel, SmartScreen/antivirus, OAuth ou une future
signature. Les temps de lancement Windows n'ont pas été mesurés, faute
d'exécution Windows native interactive ; Wine est volontairement hors
périmètre.

## Décision

**Fait vérifié :** le profil LTO complet/codegen 1 réduit fortement la taille
sans échec observé dans les tests ou le smoke test Linux.

**Décision de projet :** le profil release racine utilise `strip=true`,
`lto=true`, `codegen-units=1`. `panic=abort` reste une variante expérimentale,
non la valeur par défaut, jusqu'à une validation explicite de son contrat de
récupération sur les plateformes cibles.

**Décision de projet :** conserver l'EXE brut pour les essais et laisser le
choix d'un conteneur de transport au futur packaging. UPX reste ouvert jusqu'à
une validation Windows native ; aucune décision de distribution n'est prise
sur cette seule mesure de taille.
