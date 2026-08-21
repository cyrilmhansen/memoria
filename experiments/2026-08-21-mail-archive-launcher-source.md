# Memoria — ouverture d’archive et source Gmail depuis l’UI

Date : 2026-08-21  
Statut : expérimentation terminée, sans synchronisation déclenchée pendant la validation automatisée

## Périmètre

Cette étape ajoute le minimum nécessaire pour enlever la dépendance au
lancement avec `--archive` : démarrage sans argument, ouverture/création d’une
archive, configuration locale d’une source Gmail et déclenchement explicite du
flux OAuth existant. Le format RAW, le catalogue et l’algorithme de
synchronisation n’ont pas été modifiés.

## Implémentation

- `MemoriaConfig` est sérialisée dans le répertoire de configuration standard,
  sous `memoria/config.json`.
- La configuration mémorise les chemins des huit dernières archives au plus,
  l’archive par défaut et une association archive/source Gmail.
- Les credentials sont sélectionnés comme fichier OAuth Desktop existant ; les
  tokens restent dans le répertoire de tokens passé au connecteur, hors archive.
- L’identité source locale est une clé opaque dérivée de l’adresse renvoyée par
  le profil Gmail, ou du chemin des credentials comme repli. Elle reste
  distincte des Gmail message IDs et des hashes de contenu.
- L’action d’ajout de source appelle `HttpGmail::authenticate`, puis le profil
  Gmail, dans un worker. Elle ne lance ni OAuth ni synchronisation au démarrage.
- La synchronisation UI réutilise `sync_account_with_progress`, puis
  `index_gmail_archive`, comme le CLI.

## Faits vérifiés

- `mail-archive-app` compile sans argument d’archive.
- Sans configuration récente, il affiche l’écran initial avec `Ouvrir une
  archive…` et `Créer une archive…`.
- Un dossier contenant `metadata.sqlite` et `archive/` est reconnu comme
  archive ; un dossier invalide n’est pas initialisé implicitement.
- Une création dans un dossier vide initialise le catalogue, le répertoire
  append-only et l’index dérivé vide. Un dossier non vide est refusé.
- Une configuration avec archive, credentials et token-dir est relue au
  lancement suivant sans arguments dans une session Xvfb de test. La validation
  n’a pas écrit de données personnelles dans le dépôt.
- Une archive existante reste ouvrable lorsque credentials ou tokens sont
  absents ; seule l’action Gmail est indisponible.
- Les tests Rust passent : 21 tests de workspace, dont la vérification de
  création/validation d’archive et le round-trip de configuration sans token.
- Le build d’interface utilise Slint 1.17.1 via la dépendance workspace et
  `rfd 0.17.2` uniquement pour les sélecteurs de dossier/fichier natifs.

## Hypothèses et limites

- Le sélecteur `rfd` est un dialogue modal déclenché par une action utilisateur ;
  il n’est pas ouvert automatiquement.
- La validation réelle du navigateur Google n’est pas automatisée. Elle reste
  couverte par les campagnes Gmail précédentes et doit être exercée
  manuellement via `Ajouter un compte Gmail…` avec un fichier OAuth Desktop
  local.
- L’interface ne gère qu’une source Gmail par archive, même si le catalogue
  conserve une clé de source explicite.
- La liste « Archives récentes » du menu ouvre encore le sélecteur de dossier ;
  elle ne constitue pas un sous-menu de raccourcis à cette étape.

## Commandes reproductibles

```text
cargo fmt --all
cargo test --workspace -q
cargo build -p mail-archive-experiment --bin mail-archive-app -q
XDG_CONFIG_HOME=/tmp/memoria-empty-config-20260821 env -u WAYLAND_DISPLAY \
  timeout 3s xvfb-run -a -s '-screen 0 1280x800x24' \
  target/debug/mail-archive-app
```

Le dernier test attend volontairement l’application pendant trois secondes et
se termine par timeout ; il vérifie seulement que le démarrage sans argument
reste vivant dans une session graphique de test.

## Décision de projet

Le parcours de configuration reste volontairement pragmatique : l’utilisateur
fournit encore son propre client OAuth Google Desktop. La distribution d’un
client OAuth et un assistant multi-compte sont reportés. La prochaine
limitation produit est la validation manuelle du parcours complet
ouvrir/créer → autoriser → synchroniser depuis cette nouvelle UI.
