# Memoria — audit dépendances, binaire et surface de sécurité

Date : 2026-08-21

## Périmètre et commandes

Audit analytique du paquet `mail-archive-experiment` et du binaire
`mail-archive-app`. Aucun changement d’architecture n’a été effectué.

```text
cargo tree -p mail-archive-experiment
cargo tree -p mail-archive-experiment -d
cargo tree -p mail-archive-experiment -e features
cargo audit                 # cargo-audit 0.22.2
cargo bloat --release -p mail-archive-experiment --bin mail-archive-app --crates -n 40
ldd target/release/mail-archive-app
cargo test --workspace
cargo check --workspace
```

Le paquet possède 19 dépendances normales directes et une dépendance de build
directe (`slint-build`). Aucune dépendance Git n’est présente. Le graphe
normal comporte environ 640 paquets ; le graphe de features/build est plus
grand car il inclut les proc-macros, le compilateur Slint et les scripts de
construction. Ce nombre n’est pas une mesure de qualité.

## Graphe notable

| composant | rôle | nouveau depuis HTML/preview/i18n ? | décision |
|---|---|---:|---|
| `ammonia 4.1.4` | nettoyage HTML | oui, direct | KEEP |
| `html5ever 0.39.0` | parsing HTML utilisé par ammonia | oui, transitif | KEEP |
| `markup5ever 0.39.0` | représentation/parser HTML | oui, transitif | KEEP |
| `cssparser 0.37.0` | traitement CSS du sanitizer | oui, transitif | KEEP |
| `tendril`, `string_cache`, `phf*` | support HTML5/tokenisation | oui, transitif | KEEP |
| `url 2.5.8` | URLs Gmail/OAuth et dépendance ammonia | non | KEEP |
| `getrandom 0.3.4` | tokens opaques du serveur HTML | preview/HTML | KEEP |
| `open 5.4.1` | ouverture fichier/URL par le desktop | preview/HTML | KEEP |
| `rfd 0.17.2` | dialogues natifs | antérieur | KEEP |
| `slint 1.17.1` | UI, Winit, renderer logiciel, accessibilité | antérieur | KEEP |
| `tantivy 0.26.1` | index et recherche dérivés | antérieur | KEEP |
| `rusqlite 0.40.2` + `bundled` | catalogue local | antérieur | KEEP |

`ammonia` ajoute principalement `cssparser`, `html5ever`, `markup5ever` et
leur chaîne de support. Il n’y a pas de sanitizer maison à leur substituer.
`url` et `getrandom` ne sont pas des doublons HTML inutiles : ils ont aussi
des usages applicatifs indépendants.

Le serveur HTML local ne dépend d’aucune crate serveur ou runtime asynchrone :
il utilise exclusivement `std::net::TcpListener` et `TcpStream`. `tokio`,
`hyper`, `hyper-rustls` et `tower` apparaissent dans le graphe uniquement via
Reqwest/Gmail HTTPS. Aucun `axum`, `warp`, `actix` ou `async-std` n’est
présent.

Les crates natives/build significatives sont `libsqlite3-sys` (SQLite bundled),
`zstd-sys`, `ring`, `yeslogic-fontconfig-sys`, `wayland-sys`, `x11rb`, `cc`,
`pkg-config` et `vcpkg`. Elles correspondent aux choix de portabilité,
SQLite, compression, TLS et fontconfig déjà validés.

Les doublons sont surtout des familles de plateforme ou de génération :
`getrandom` 0.2/0.3/0.4, `calloop` 0.13/0.14, `rustix` 0.38/1.x,
`read-fonts` 0.39/0.41, `skrifa` 0.42/0.44 et `tiny-skia` 0.11/0.12.
Ils proviennent de Slint/Winit/AccessKit et de leurs build/proc-macros ; aucune
unification sûre n’a été démontrée.

Slint est configuré sans defaults : `backend-winit`, Wayland, X11,
`renderer-software`, `accessibility`, `std` et `compat-1-2`. `system-tray`,
`renderer-femtovg` et `backend-default` ne sont pas activés par le manifest.
`rfd` conserve `wayland` + `xdg-portal`, nécessaires au chemin KDE validé.
Reqwest conserve une seule pile TLS Rustls (`blocking`, `json`, `rustls-tls`)
et aucune pile native-tls/OpenSSL.

## RustSec

Avec `cargo-audit 0.22.2` et la base locale courante :

- vulnérabilités exploitables signalées : **0** ;
- crates yanked signalées : aucune ;
- avertissements de maintenance : `paste 1.0.15`, `rustybuzz 0.20.1`,
  `ttf-parser 0.25.1`, `bincode 2.0.1` ;
- alerte unsoundness : `lru 0.16.4`, `RUSTSEC-2026-0253`, dépendance de
  `tantivy 0.26.1`, corrigée à partir de `lru >=0.18.2`.

Le `bincode` signalé est une entrée inutilisée par le graphe actif observé
par `cargo tree`, probablement conservée dans le lockfile via une branche
conditionnelle/build ; il n’est pas utilisé par le code Memoria.

Les avertissements de police et de typographie appartiennent principalement
à Slint/Resvg et ne sont pas liés au chemin MIME/HTML. L’alerte `lru` est à
surveiller : Tantivy l’utilise pour le cache de blocs du document store, mais
le code Memoria ne contrôle pas les valeurs qui y sont stockées et le chemin
observé utilise `get`, `put` et `peek_lru`, pas une utilisation directe de
`pop`. Une mise à jour directe est impossible : Tantivy 0.26.1 exige
`lru ^0.16.3`, qui exclut 0.18.x. Aucune modification forcée n’a été faite.
La prochaine mise à jour compatible de Tantivy devra reconsidérer cette
alerte.

`cargo-deny` n’est pas installé et aucune configuration minimale existante ne
permet encore de distinguer proprement les artefacts expérimentaux des
dépendances produit. `cargo audit` est conservé comme contrôle sécurité
actuel ; pas de configuration cargo-deny ajoutée dans cette passe.

La surface non fiable reste bornée : RAW/MIME est parsé par `mailparse`, HTML
est nettoyé par ammonia, les ressources CID sont servies depuis une table en
mémoire, les URL distantes sont bloquées par CSP, et les images de preview
passent par des providers système hors processus avec timeout. Memoria ne lie
ni Qt, ni KF6, ni GTK, ni WebKit, ni WebEngine, ni Chromium.

## Taille binaire et bloat

Après reconstruction release normale, le binaire Linux fait **31 070 752
octets**. `ldd` ne montre que `libgcc_s`, `libm`, `libc` et le loader système.

`cargo bloat --crates` a produit une analyse de `.text` d’environ 19,7 MiB
avec les principaux contributeurs : `std` 3,2 MiB, `zbus` 926 KiB, le code
de l’application 874 KiB, `tantivy` 803 KiB, `winit` 581 KiB,
`i-slint-core` 571 KiB, `accesskit_unix` 487 KiB, `usvg` 417 KiB,
`wayland-client` 416 KiB, `rustls` 370 KiB, `zstd_sys` 327 KiB,
`i-slint-renderer-software` 307 KiB et `html5ever` 141 KiB.

Le bloat report a reconstruit un artefact diagnostique non strippé de 37,1
MiB ; cette taille n’est donc pas comparable au fichier release strippé de
31 070 752 octets. Les estimations de `cargo bloat` sont également
approximatives avec LTO et code partagé.

La passe i18n ajoute environ 17,6 KiB de source statique et aucune dépendance.
Les +692 KiB entre les historiques 30 378 656 et 31 070 752 ne peuvent donc
pas être attribués honnêtement au catalogue : les builds historiques ne sont
pas une comparaison bit-à-bit contrôlée et la variation inclut linkage,
monomorphisation, chaînes déjà présentes et autres changements de l’arbre de
travail. Le bloat ne montre pas une nouvelle grosse crate i18n.

## Identifiants techniques

Le scope Gmail est déjà centralisé dans `gmail.rs`. Les noms de champs
Tantivy sont regroupés dans `TantivyFields`, avec conversion au schéma à la
frontière. Les variables `MEMORIA_*` appartiennent au module thumbnail. Les
routes HTML sont locales à `html_preview.rs`. Les MIME `application/pdf` et
`image/*` sont répétés dans la logique de pièce jointe/preview, mais leur
répétition représente la même politique locale et ne justifie pas un
`constants.rs` global dans cette passe.

## Classification

| catégorie | éléments |
|---|---|
| KEEP | ammonia, pile Rustls, Slint explicite, Tantivy, SQLite bundled, KIO/freedesktop hors graphe Cargo |
| UPDATE | `lru` lorsque Tantivy publiera une contrainte compatible avec la correction RustSec |
| REMOVE | aucun candidat démontré ; le serveur HTML n’a pas de runtime serveur superflu |
| WATCH | warnings maintenance Slint/Resvg, doublons plateforme, séparation future des outils de benchmark, cargo-deny |

## Conclusion

Le graphe paie le coût attendu d’une GUI accessible multiplateforme, du moteur
de recherche, de SQLite et de TLS. HTML ajoute un sanitizer généraliste
raisonnable, pas un moteur de navigateur. Preview système n’ajoute aucune
dépendance Qt/KF6 au binaire principal. Aucun changement de code produit n’est
justifié par cet audit, à l’exception d’une surveillance explicite de `lru`
bloquée par la contrainte actuelle de Tantivy.
