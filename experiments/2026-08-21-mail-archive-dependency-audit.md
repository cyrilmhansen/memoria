# Memoria — audit ciblé des dépendances

Date : 2026-08-21. Périmètre : `mail-archive-experiment`, binaire
`mail-archive-app`, Linux x86-64 et graphe Windows déjà établi. Aucun
changement fonctionnel n'est conservé.

## Mesures de référence

Commandes exécutées :

```text
cargo tree -p mail-archive-experiment
cargo tree -p mail-archive-experiment -e features
cargo tree -p mail-archive-experiment -d
cargo build --release --timings -p mail-archive-experiment --bin mail-archive-app
cargo bloat --release -p mail-archive-experiment --bin mail-archive-app --crates -n 30
cargo check --workspace
cargo test --workspace
```

Le rapport Cargo est sous `target/cargo-timings/`. Le build release observé a
duré 48,19 s après recompilation du graphe. Cela mesure la compilation, pas le
démarrage de l'application.

Le graphe Linux contient environ 639 paquets dans les dépendances normales et
658 en incluant les dépendances build/proc-macro du chemin application. Il
contient 16 dépendances directes normales et 2 directes de build. Ce sont des
cardinalités de graphe, pas une métrique de qualité ni le nombre de crates
visibles dans l'ELF final.

| artefact | taille |
|---|---:|
| Linux `target/release/mail-archive-app` | 42 363 672 octets |
| Windows MSVC dynamique | 26 455 552 octets |
| Windows MSVC CRT statique | 26 625 536 octets |

`cargo bloat --crates` estime la section `.text` Linux à 20,6 MiB et le fichier
analysé à 46,9 MiB. Les contributions principales sont : `zbus` 1,1 MiB,
`tantivy` 1,1 MiB, `accesskit_unix` 956 KiB, `i-slint-backend-winit`
913 KiB, `i-slint-core` 796 KiB, `winit` 689 KiB, `reqwest` 450 KiB,
`rustls` 434 KiB et `i-slint-renderer-software` 386 KiB. Ce sont des
estimations avec code partagé et monomorphisation.

## Slint

Les manifests utilisent déjà `default-features = false` et activent
explicitement `std`, `backend-winit`, `renderer-software`, `accessibility` et
`compat-1-2`. Le graphe ne contient donc ni `renderer-femtovg`, ni
`backend-default`, ni `system-tray` via les defaults Slint.

- Linux : le backend Wayland natif a été observé précédemment ; X11 reste
  compilé pour Xvfb et le fallback.
- Windows : Winit Windows avec le renderer logiciel est le chemin attendu.
- AccessKit est requis et représente une contribution visible du binaire.
- Aucun tray n'est utilisé ni activé.

Aucune réduction Slint sûre n'est donc disponible sans sacrifier Wayland/X11,
Windows, accessibilité ou le renderer validé. `fontique` reste volontaire pour
`fontconfig-dlopen` et la contrainte Homebrew/fontconfig Linux.

## Reqwest/TLS

`reqwest` est déclaré avec `default-features = false` et seulement `blocking`,
`json` et `rustls-tls`. Le graphe contient Rustls, Hyper-Rustls, Tokio-Rustls,
Ring et WebPKI roots, mais ni `native-tls`, ni OpenSSL, ni Hyper-TLS. Il n'y a
donc qu'une pile TLS HTTPS. Tokio est transitif à Reqwest/Hyper ; aucun second
runtime async direct n'est ajouté.

## Tantivy

Les defaults activés sont `mmap`, `stopwords`, `lz4-compression`,
`columnar-zstd-compression` et `stemmer`. Le code utilise `Index`,
`IndexWriter`, `QueryParser`, les dates et le tokenizer `default`; il ne
configure pas explicitement stemmer ou stopwords.

Une variante isolée a désactivé seulement `stemmer` et `stopwords`, en gardant
`mmap`, LZ4 et Zstd. Tests et compilation passent. Sur le même benchmark
déterministe de 10 000 messages :

| mesure | defaults | sans stemmer/stopwords |
|---|---:|---:|
| binaire Linux | 42 363 672 | 42 017 192 octets |
| index Tantivy | 3 073 227 | 3 074 622 octets |
| indexation Tantivy | 1 426 ms | 727 ms |
| p50 Tantivy | 799 µs | 771 µs |
| p95 Tantivy | 4 191 µs | 3 994 µs |

L'index est pratiquement identique et les écarts de temps sont du bruit sur
une petite mesure. Le gain binaire d'environ 346 KiB ne justifie pas de
retirer ces capacités de recherche potentielles. Le manifest est revenu aux
defaults Tantivy.

## rfd

`rfd 0.17.2` est utilisé uniquement pour les dialogues. Ses defaults sont
exactement `xdg-portal` et `wayland`, avec `pollster`, les clients Wayland et
leurs protocoles. Cela correspond au dialogue KDE/Portal validé. Ces
dépendances sont conditionnées à Linux et ne deviennent pas des DLL Windows.

Déclarer `default-features = false` avec ces mêmes deux features ne change
pas le graphe ni le comportement ; aucun changement sans gain n'est introduit.

## Dépendances directes

| dépendance/feature | pourquoi présente | décision |
|---|---|---|
| `base64` | RAW Gmail base64url | conservée |
| `blake3` | checksums et CAS expérimental | conservée |
| `dirs` | configuration utilisateur portable | conservée |
| `flate2` | mesure gzip du générateur | conservée, expérimentale |
| `fontique` | fontconfig dynamique Slint | conservée |
| `mailparse` | parsing MIME dérivé | conservée |
| `reqwest` + Rustls | Gmail/OAuth HTTPS | conservée |
| `rfd` | dialogues natifs | conservée |
| `rusqlite bundled` | catalogue local | conservée |
| `serde`/`serde_json` | config et réponses Gmail | conservée |
| `tantivy` defaults | recherche dérivée | conservée |
| `url` | URLs OAuth/Gmail | conservée |
| `webbrowser` | navigateur OAuth | conservée |
| `zstd` | mesure compression du générateur | conservée, expérimentale |
| `slint-build` | compilation `.slint` | conservée, build-only |

`flate2` et le `zstd` direct appartiennent surtout au CLI/corpus expérimental;
`zstd_sys` visible dans le bloat provient aussi de Tantivy. Les retirer
nécessiterait de séparer le prototype expérimental du paquet produit, sans
gain mesuré prioritaire.

## Doublons et sécurité

`cargo tree -d` montre surtout des doublons attendus entre Slint/Winit/rfd et
leurs branches build/runtime : calloop, smithay-client-toolkit, tiny-skia,
read-fonts, skrifa, font-types, thiserror, syn et hashbrown. Aucune
unification forcée n'est sûre.

`cargo deny` n'est pas installé ; l'audit advisories/licences/sources n'a donc
pas été exécuté. Aucune version n'a été modifiée sur cette base.

## Classification finale

- **Poids intrinsèque raisonnable :** Slint + Winit + AccessKit + fonts,
  Tantivy + mmap/compression, Reqwest + Rustls, SQLite bundled.
- **Gaspillage supprimable identifié :** aucun candidat suffisamment sûr. Les
  346 KiB Tantivy ne justifient pas de retirer des fonctionnalités.
- **Dette à reconsidérer :** séparer un jour les outils de génération et de
  compression expérimentaux du paquet produit ; réévaluer les doublons lors
  d'une mise à jour majeure coordonnée.

## Conclusion

Le graphe est volumineux parce que Memoria combine GUI accessible,
multiplateforme, recherche locale et OAuth HTTPS. Il ne révèle ni seconde
pile TLS, ni renderer Slint inutile, ni tray activé, ni codec manifestement
mort dans le chemin produit. Aucun changement de dépendances n'est conservé.
