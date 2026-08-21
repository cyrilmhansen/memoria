# Memoria — première prévisualisation système des pièces jointes

Date : 2026-08-21

## Périmètre

Cette passe ajoute une prévisualisation temporaire dans la fenêtre Memoria,
sans modifier le RAW, le catalogue SQLite ou l'index Tantivy. Le contenu est
toujours extrait à la demande depuis le message MIME et le fichier temporaire
est supprimé avec le magasin temporaire de la session.

## Implémentation

La couche interne `thumbnail` ne connaît que des chemins et un résultat
`PathBuf` PNG : `Unavailable`, `Provider`, `Timeout`, `Io` ou `Disabled` sont
convertis en message UI. Slint ne voit ni KIO, ni Qt, ni le protocole des
workers. Le travail de lecture MIME, d'extraction et de lancement du provider
se fait dans un thread secondaire ; l'image PNG est chargée ensuite par le
thread Slint.

Sous KDE, Memoria tente le helper KF6/KIO existant (`KIO::PreviewJob`) puis le
probe freedesktop. Le helper est lancé sans shell, avec une limite de 15 s et
un fichier de sortie PNG explicite. Le binaire Memoria principal n'est pas lié
à Qt/KF6. Sous Windows, le point d'injection d'un futur helper Shell est
présent, mais aucun backend Windows n'est activé à cette étape.

Le mode `MEMORIA_DISABLE_SYSTEM_PREVIEWS` désactive entièrement les previews.
Les boutons Ouvrir et Enregistrer sous restent indépendants. Escape, Fermer et
un clic hors du panneau ferment le suraffichage.

## Vérifications reproductibles

```text
cargo fmt --all
cargo test --workspace
cargo check --workspace
cargo build --release -p mail-archive-experiment --bin mail-archive-app
cmake -S experiments/kio-thumbnail-probe -B /var/tmp/atlas-kio-build-20260821-preview
cmake --build /var/tmp/atlas-kio-build-20260821-preview -j2
/var/tmp/atlas-kio-build-20260821-preview/kio-thumbnail-probe preview /usr/share/pixmaps/archlinux-logo.png 256 /var/tmp/atlas-kio-preview.png
ldd target/release/mail-archive-app | rg -i 'qt|kf6|webkit|webengine|gtk'
```

Les tests Rust workspace passent (21 tests de bibliothèque et 5 tests du
binaire/UI). Le helper KIO produit bien une PNG 256×256 sur une image locale,
environ 121 ms dans cette exécution. Le binaire release Memoria mesure
29 327 488 octets dans le profil courant. `ldd` ne révèle aucune dépendance
Qt, KF6, WebKit, WebEngine ou GTK dans Memoria.

Le probe KIO a déjà validé séparément le chemin PDF KDE avec `gsthumbnail.so`;
le même helper est maintenant consommable par Memoria. Les mesures de ce
rapport ne recopient aucun contenu réel de l'archive.

## Validation KDE Wayland sur l'archive réelle

Memoria a été lancé directement dans la session KDE Wayland avec l'archive
locale existante et le helper passé par `MEMORIA_KIO_THUMBNAIL_HELPER`.
L'arbre AT-SPI a permis de sélectionner un résultat réel filtré par présence
de pièce jointe, puis d'activer son bouton Aperçu. Le lecteur a exposé un
overlay nommé Aperçu, une image accessible et un bouton Fermer. Le PDF réel a
été extrait à la demande, traité par KIO/Ghostscript et affiché comme PNG de
première page ; l'extraction et le traitement n'ont pas modifié l'archive.
Une image réelle extraite hors dépôt a également été acceptée par le même
helper KIO et convertie en PNG.

La fermeture par le bouton a été vérifiée. Le chemin Escape est installé dans
le handler clavier Slint ; l'injection de touche AT-SPI de cette session ne
permet toutefois pas d'en faire une mesure indépendante. Le clic extérieur a
révélé un défaut de hit-testing : une TouchArea pleine placée derrière le
panneau n'était pas fiable avec ce backend. Quatre zones explicites autour du
panneau ont été ajoutées ; elles ciblent uniquement l'extérieur et ne
recouvrent pas le contenu modal. La correction est couverte par la compilation
Slint et les tests workspace, sans changer le modèle de preview.

La zone de lecture reste fonctionnelle après fermeture. Les actions Ouvrir et
Enregistrer sous restent des callbacks distincts ; aucune application externe
n'est lancée par Aperçu.

La résolution sans helper a été vérifiée par le chemin de
fallback/indisponibilité ; le mode
`MEMORIA_DISABLE_SYSTEM_PREVIEWS=1` court-circuite la génération. Dans les deux
cas, le lecteur conserve le message et ses actions de fichier.

## Localisation du helper

En développement, Memoria accepte `MEMORIA_KIO_THUMBNAIL_HELPER`, puis cherche
`memoria-kio-thumbnail-helper` à côté de l'exécutable, puis un helper nommé
`kio-thumbnail-probe` à côté de l'exécutable ou dans `PATH`. Le helper est
donc explicitement fourni par l'environnement de développement ; Memoria ne
compile ni ne charge Qt/KF6 lui-même.

Après installation, le paquet doit placer `memoria-kio-thumbnail-helper` à
côté de Memoria ou dans un répertoire `PATH` contrôlé. Si aucun helper KIO
n'est trouvé, Memoria tente le probe freedesktop existant ; si aucun backend
ne répond, l'état est `Aperçu indisponible` sans bloquer la lecture.

## Décisions et limites

**Fait vérifié.** Le service système est préférable à l'ajout d'un décodeur
raster ou d'un catalogue de codecs : il couvre les formats desktop et garde
les dépendances Qt/KF6 hors du processus principal.

**Décision de projet.** L'aperçu est limité aux images et PDF détectés comme
pièces jointes téléchargeables. Une erreur, un provider absent ou un timeout
laisse le lecteur et les actions Ouvrir/Enregistrer sous disponibles.

**Limite.** La validation interactive a été faite avec AT-SPI dans la session
Wayland ; aucune capture ni donnée personnelle n'est conservée. Les
différences de providers sur une autre installation Linux et la validation du
backend Windows restent ouvertes.

L'overlay utilise `ImageFit.contain` dans une zone bornée : les grandes images
sont réduites sans déformation et les petites restent centrées. Un zoom
interactif et un viewer PDF multipage restent hors périmètre.
