# Expérience — progression import / synchronisation Memoria

Date : 2026-08-21  
Statut : implémentation et validation par fixtures ; format d'archive inchangé.

## Sémantique

La progression expose désormais `examined` et `total` séparément.

- `X` (`examined`) est le nombre de messages parcourus, y compris ceux déjà
  présents et réconciliés par métadonnées.
- `N` (`total`) est le nombre d'identifiants retournés par l'énumération full
  sync, après pagination et application éventuelle de `--max-messages`.
- `new_messages`, `network_bytes` et `archive_bytes_added` restent des
  compteurs indépendants ; les messages connus n'augmentent pas le volume
  réseau RAW.
- Pour une synchronisation `history`, le total n'est pas connu à l'avance sans
  parcours supplémentaire : l'UI reste indéterminée et affiche le nombre
  examiné ainsi que les nouveaux et les octets reçus.

La full sync énumère d'abord les pages Gmail déjà nécessaires au parcours,
puis traite les identifiants. Elle n'effectue pas une seconde énumération
réseau pour obtenir N. Les IDs de la liste sont conservés en mémoire pendant
la campagne ; le format d'archive et le catalogue ne changent pas.

## UI

La vue Archive/Synchronisation conserve sa structure actuelle et ajoute une
barre compacte. Elle montre :

1. `Synchronisation Gmail en cours…` et `X messages sur N` lorsque N est
   fiable ;
2. une barre indéterminée et `X messages examinés` pour history ;
3. `Mise à jour de l’index de recherche…` après validation archive/catalogue ;
4. `Archive à jour · index de recherche à jour` à la fin.

La barre reste à emplacement fixe dans la carte source. Une synchronisation
vide peut aller trop vite pour être perceptible, mais ne bloque pas l'UI.

Tests ajoutés : progression 0/N, N/N, total inconnu, nouveaux séparés et
full sync avec messages connus comptés. Les tests existants couvrent les
erreurs d'index après archivage et les synchronisations incrémentales vides.

## Validation

`cargo test --workspace`, `cargo check --workspace`, `cargo fmt --all` et
`git diff --check` sont exécutés après la modification. Le binaire release
Memoria compile.

Une archive Gmail réelle existante a été ouverte localement sous KDE Wayland
et les statistiques hors ligne de pièces jointes ont été relues sans écriture.
L'animation d'une longue synchronisation réelle n'a pas été reproduite : le
compte est déjà synchronisé et le daemon `ydotoold` nécessaire à une saisie
automatisée Wayland n'est pas disponible. La logique UI est testée par
fixtures ; aucun contenu personnel n'est enregistré.

## Limites

Le total d'une synchronisation history reste volontairement indéterminé. Une
future campagne pourra mesurer si une collecte préalable des événements est
acceptable, mais elle n'est pas introduite ici uniquement pour obtenir une
barre déterminée.

