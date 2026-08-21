# Memoria — passe FR/EN et identifiants techniques

Date : 2026-08-21

## Périmètre

Passe de maintenance limitée à l’interface Memoria et aux messages produits
par le contrôleur. RAW, SQLite, Tantivy et la logique fonctionnelle ne changent
pas.

## Choix i18n

**Fait vérifié.** L’audit local de Slint 1.17.1 ne révèle pas de catalogue de
traduction intégré utilisable directement depuis le fichier `.slint`. Les
textes visibles sont donc fournis par un petit catalogue Rust applicatif,
`src/i18n.rs`, sans runtime Qt/Gettext ni nouvelle dépendance Cargo.

**Décision de projet.** `Language` accepte une locale explicite pour un futur
réglage et détecte aujourd’hui `LC_ALL`, `LC_MESSAGES`, `LANGUAGE`, puis `LANG`.
Les locales commençant par `fr` sélectionnent le français ; toute autre locale
et l’absence de locale sélectionnent l’anglais.

Le catalogue contient les textes de chrome, menus, recherche, filtres,
archive, pièces jointes et aperçu. Les pluriels principaux sont produits par
des fonctions complètes, couvrant zéro, un et plusieurs éléments.

Ajouter une langue consiste à ajouter une variante de `UiStrings` et les
messages paramétrés dans `src/i18n.rs`; le fichier Slint reste inchangé.

## Identifiants

Les valeurs comme `application/pdf`, `gmail.readonly`, `has_attachment`, les
labels Gmail et les clés internes ne passent pas par le catalogue. La valeur
interne du filtre de pièces jointes reste distincte de son libellé traduit.

## HTML

Les sessions du serveur HTML local sont maintenant purgées après 10 minutes et
bornées à 8 sessions. La réponse conserve une CSP `img-src 'self'` et
`connect-src 'none'`, ce qui interdit les récupérations réseau automatiques ;
les tests vérifient cette politique ainsi que l’absence d’exécution active.

Le système ne fournit pas de navigateur headless (`google-chrome`, `chromium`
ou équivalent) dans le PATH de cette machine : aucun test navigateur automatisé
n’a donc été prétendu. Le contrôle réalisé est déterministe au niveau de la
réponse HTTP/CSP.

## Vérifications

```text
cargo fmt --all
cargo test -p mail-archive-experiment
cargo check --workspace
```

Les tests du crate passent, dont les tests FR/EN/fallback/pluriels,
l’expiration/bornage des sessions HTML et les tests HTML existants.
