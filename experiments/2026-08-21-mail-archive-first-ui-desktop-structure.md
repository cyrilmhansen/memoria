# Memoria — corrections ciblées et structure desktop

Date : 2026-08-21  
Périmètre : uniquement les défauts observés lors de la première utilisation
manuelle de Memoria. Aucun accès Gmail n'a été effectué.

## Corrections

- Le champ de recherche reçoit une hauteur explicite de 36 px dans un bandeau
  légèrement plus haut, pour éviter le clipping vertical observé.
- La navigation clavier ajuste `ListView.viewport-y` après chaque déplacement.
  La ligne sélectionnée reste donc dans le viewport, y compris avec 50
  résultats.
- Une séparation de 1 px est rendue entre les lignes, sans modifier la liste
  ni sa sélection.
- Le corps du message utilise le `TextEdit` Slint en lecture seule. Il fournit
  le défilement vertical à la molette, un ascenseur selon le besoin, la
  sélection de texte, Ctrl+C et le menu contextuel Copy/Select All fourni par
  Slint. Le bandeau de métadonnées reste hors de cette zone défilante.
- Le fallback HTML masque les URL brutes dans le texte dérivé sous la forme
  `[lien externe]`. Ce choix évite une ligne illisible en attendant une vraie
  représentation de lien ; le RAW et l'index ne sont pas modifiés.

## Structure desktop

Une `MenuBar` Slint traditionnelle accueille les menus `Fichier`, `Archive`,
`Recherche`, `Affichage` et `Aide`. Les commandes de synchronisation restent
désactivées : le second espace Archive/Synchronisation est seulement préparé,
et Recherche/Consultation reste l'espace fonctionnel unique de ce prototype.
Le raccourci Ctrl+F et l'effacement Ctrl+Retour arrière sont conservés dans le
contrôleur clavier et dans le menu Recherche.

Une icône portable n'a pas été ajoutée : Slint 1.17.1/winit n'expose pas ici
de propriété d'icône de fenêtre multiplateforme simple. Ajouter une icône
nécessiterait une intégration plateforme spécifique disproportionnée pour ce
prototype.

## Limites HTML toujours explicites

Le rendu reste une extraction texte dérivée : il ne conserve pas la
typographie, les liens cliquables, les couleurs/backgrounds, les images ni la
structure HTML et ne promet pas la mise en page fidèle des newsletters à
largeur fixe. Les messages restent consultables en texte reflué ; le RAW
demeure la représentation faisant autorité.

## Vérification

Commandes reproductibles :

```text
cargo fmt --all
cargo test --workspace -q
cargo check --workspace -q
cargo build -p mail-archive-experiment --bin mail-archive-app -q
timeout 8s target/debug/mail-archive-app --archive .local/gmail-real-20260820
```

Les 15 tests mail et les 2 tests du workspace passent. Le lancement direct
dans la session Wayland réelle reste vivant jusqu'à son arrêt contrôlé par
timeout, sans erreur de démarrage ni accès réseau Gmail. Les captures et les
contenus de l'archive ne sont pas conservés dans le dépôt.

## Classification

- **Fait vérifié :** `TextEdit` read-only de Slint expose la sélection/copie et
  son défilement, et compile avec le backend actuel.
- **Fait vérifié :** la transformation URL est couverte par un test unitaire
  sans données personnelles.
- **Décision de projet :** différer l'icône et le renderer HTML complet.
- **Limite :** l'injection clavier Wayland et l'inspection AT-SPI restent les
  limites documentées dans le rapport précédent ; cette passe ne les présente
  pas comme une validation supplémentaire.
