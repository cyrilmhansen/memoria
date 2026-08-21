# Expérience — service de miniatures système

## Périmètre

Probe isolé dans `experiments/system-thumbnail-probe/`. Memoria, son format
d'archive et son UI n'ont pas été modifiés. Le probe ne contient aucun codec
PDF/Office/vidéo : il demande une miniature au desktop et accepte uniquement
une image PNG non vide comme résultat.

La commande commune est conceptuellement :

```text
thumbnail(path, max_size) -> thumbnail | unavailable | error
```

Une icône de fichier n'est jamais transformée en succès. Le probe retourne
également le chemin de la PNG temporaire, sa taille et si elle provient du
cache.

## Fait vérifié — Linux/KDE Wayland

Environnement observé : KDE, session Wayland. Aucun module KIO thumbnail dédié
n'était exposé par la session, mais le mécanisme freedesktop standard et les
fichiers `.thumbnailer` installés étaient disponibles.

Providers détectés localement :

- `glycin-thumbnailer` pour JPEG, PNG, SVG, TGA et plusieurs autres images ;
- `ffmpegthumbnailer` pour vidéo et audio ;
- `gsf-office-thumbnailer` pour Office/OpenDocument ;
- un provider Xournal++.

Le probe :

1. calcule la clé MD5 de l'URI `file://` et consulte `normal`, `large` et
   `x-large` sous `$XDG_CACHE_HOME/thumbnails` ;
2. identifie le MIME avec `gio` ;
3. sélectionne le `.thumbnailer` correspondant ;
4. lance directement le provider, sans shell, avec une limite de 10 secondes ;
5. valide la signature et les dimensions PNG.

Résultats sur des fichiers synthétiques locaux :

| Format | Résultat observé |
|---|---|
| PNG | thumbnail |
| JPEG | thumbnail |
| SVG | thumbnail |
| TGA | thumbnail |
| PDF | thumbnail via KDE/KIO (`gsthumbnail.so`) ; absent du chemin `.thumbnailer` |
| MP3 avec pochette | thumbnail |
| MP3 sans pochette | error : `ffmpegthumbnailer` échoue sans image exploitable |
| MP4 | thumbnail |
| ODS | thumbnail |
| ODT | error du provider GSF |
| DOCX | error du provider GSF |
| XLSX | error du provider GSF |
| PPTX/ODP | non mesurés avec un fixture valide dans cette passe |

Les erreurs Office sont des erreurs du provider installé, pas une conversion
locale ajoutée au probe. La liste des formats ne doit donc pas être codée dans
l'API : elle dépend des providers et de leur configuration.

## Correction — backend KDE/KIO

La conclusion initiale `PDF = unavailable` était limitée au seul inventaire
freedesktop `.thumbnailer`. Elle était donc incorrecte pour cette machine.

Le paquet installé est `kdegraphics-thumbnailers 26.04.3-1.1` et contient
`/usr/lib/qt6/plugins/kf6/thumbcreator/gsthumbnail.so`. Le probe KF6 isolé est
dans `experiments/kio-thumbnail-probe/`; il utilise Qt 6 Widgets et
`KIO::PreviewJob`, sans modifier Memoria.

`KIO::PreviewJob::availableThumbnailerPlugins()` découvre `gsthumbnail` et
ses MIME annoncés sont :

```text
application/x-dvi
application/postscript
application/pdf
image/x-eps
```

Avec exactement le fixture PDF synthétique de la campagne précédente, le
chemin KIO donne une PNG 256x192. Mesures locales : première génération
environ 137 ms et 61 MiB RSS ; seconde lecture depuis le cache environ 11 ms
et 60 MiB RSS. Le résultat est également écrit dans le cache `normal/large`
par Dolphin pour le même fichier : Dolphin obtient donc effectivement une
miniature du fixture.

Le probe KIO passe explicitement la liste des plugins disponibles à
`filePreview`. Cela évite de dépendre d'une configuration utilisateur neuve
qui peut ne pas activer le plugin malgré sa découverte. Cette observation est
une particularité du probe, pas une modification de Memoria.

Le backend réellement exécuté est observable avec `strace` :

```text
/usr/lib/kf6/kioworker ... thumbnail ...
  -> gsthumbnail.so
     -> /usr/bin/gs
```

Le `ThumbnailCreator` n'est donc pas chargé dans le processus appelant de
`PreviewJob`. Le worker KIO est un processus séparé, et Ghostscript est encore
un processus enfant. Un crash du creator ou de Ghostscript ne fait pas tomber
le probe appelant. Le probe impose en plus un délai de 15 secondes et annule
le `KIO::Job`; ce délai est celui de l'expérience, pas une garantie globale de
tous les appels KIO.

La première expérience `.thumbnailer` doit maintenant être lue comme suit :

- **absence freedesktop :** aucun `.thumbnailer` PDF n'était installé ;
- **disponibilité KDE :** KIO + `gsthumbnail.so` supportent bien PDF ;
- **indisponibilité réelle :** seulement après échec des deux backends.

Le premier appel image a pris environ 130–190 ms dans cette session. Après
copie contrôlée de la PNG vers la clé freedesktop correspondante, le second
appel a été reconnu `cached=true` et a pris environ 0 ms. Le probe ne remplit
pas lui-même le cache global : il le consulte ; la génération et la politique
d'écriture du cache restent celles du desktop/provider.

Une mesure `/usr/bin/time -v` sur une vidéo a relevé environ 56 MiB de RSS pour
le probe appelant. Le helper séparé était du même ordre (environ 55 MiB) et
ajoutait environ 20 ms. Ces chiffres incluent surtout le processus probe et le
provider, et ne constituent pas une consommation Memoria intégrée.

## Fait vérifié — Windows

Le backend Windows compile avec `cargo xwin` pour
`x86_64-pc-windows-msvc`. Il utilise `IShellItemImageFactory::GetImage` avec
`SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK`, puis convertit le bitmap HBITMAP
en PNG RGBA. Une réponse icône seule n'est donc pas acceptée.

La taille release Linux du probe est de 420 384 octets ; le binaire Windows
cross-compilé fait 378 880 octets. Les imports Windows observés sont les DLL
système Shell/COM/GDI et le runtime MSVC (`VCRUNTIME140.dll` et les DLL CRT
API-set). Le comportement réel des providers Shell, l'association des fichiers
et les cas sans miniature nécessitent encore un Windows natif ; Wine n'est
pas utilisé comme validation UX.

## Isolation et contrat de robustesse

Sous Linux, l'appel au provider est déjà un processus enfant avec timeout. Un
provider qui plante ne fait pas tomber le processus appelant ; un provider qui
bloque est interrompu après 10 secondes. L'appel direct peut néanmoins retenir
le thread qui attend le résultat pendant ce délai.

Le mode `helper` lance en plus le probe dans un processus enfant. Il apporte une
isolation du thread et du processus appelant face aux défauts du backend, au
prix d'un processus et d'une copie/IPC très simple. Il ne supprime pas le
timeout du provider : il le double seulement comme frontière supplémentaire.

Sous Windows, l'appel Shell est actuellement dans le processus du probe. Le
helper est donc la voie à retenir si l'expérience Windows native montre qu'un
provider Shell peut bloquer ou faire remonter une panne au processus appelant.
Le backend doit rester désactivable : `unavailable` ou une erreur doivent être
des résultats normaux, jamais une obligation d'afficher une preview.

## Dépendances et coût

Le probe Linux utilise uniquement `md-5` pour la clé freedesktop. Le backend
Windows ajoute `windows` et `png`, uniquement sous Windows. Le binaire Linux
ne lie ni GTK, ni Qt, ni WebKit, ni FFmpeg : `ldd` ne montre que libc, libgcc et
le chargeur système. Le probe Windows ne bundle aucun moteur de rendu.

## Conclusions

**Fait vérifié :** une API très petite peut obtenir des miniatures réelles de
plusieurs familles via les services installés du desktop, sans embarquer de
renderer. Sous KDE, KIO doit être considéré comme un backend distinct du
chemin freedesktop `.thumbnailer`.

**Limite vérifiée :** la couverture et la qualité ne sont pas garanties par
format. Un provider peut être absent ou échouer, notamment pour PDF, certains
documents et audio sans pochette.

**Décision de projet provisoire :** ne pas intégrer encore ce probe dans
Memoria. Le contrat `thumbnail | unavailable | error`, une politique de
timeout et l'option de désactivation sont suffisamment clairs pour une future
intégration.

**Ordre Linux provisoire :** sous KDE, préférer KIO/`PreviewJob`, car il
réutilise le worker et les `ThumbnailCreator` KDE, puis utiliser le backend
freedesktop `.thumbnailer` comme fallback lorsque KIO n'est pas disponible ou
échoue. Sur un bureau non-KDE, utiliser directement le backend freedesktop.

**Helper :** KIO apporte déjà une frontière de processus Linux pour le worker
et les providers observés ; un helper Memoria supplémentaire n'est donc pas
justifié par cette expérience. Il devient probablement préférable pour le
backend Windows si un test natif établit un risque de blocage/crash dans
l'appel Shell. Cette décision reste ouverte jusqu'à la validation Windows.

**Crate indépendant :** l'abstraction mérite d'être conservée comme petit
module expérimental, mais pas encore publiée. Il faut d'abord définir le
contrat Windows natif et éventuellement une représentation d'image plus
directement exploitable par Slint.

## Reproduction

```text
cargo run --release --manifest-path experiments/system-thumbnail-probe/Cargo.toml -- providers
cargo run --release --manifest-path experiments/system-thumbnail-probe/Cargo.toml -- thumbnail FILE 256
cargo run --release --manifest-path experiments/system-thumbnail-probe/Cargo.toml -- helper FILE 256
cargo xwin build --release --manifest-path experiments/system-thumbnail-probe/Cargo.toml --target x86_64-pc-windows-msvc

cmake -S experiments/kio-thumbnail-probe -B /var/tmp/kio-thumbnail-build
cmake --build /var/tmp/kio-thumbnail-build
/var/tmp/kio-thumbnail-build/kio-thumbnail-probe plugins
/var/tmp/kio-thumbnail-build/kio-thumbnail-probe preview FILE 256
```

Les corpus et sorties contenant des fichiers temporaires restent hors Git,
dans `/var/tmp`, puis sont supprimés à la fin de la campagne.

## Audit IPC KIO — 2026-08-21

Le probe KF6 a été tracé avec `strace -ff` et `dbus-monitor --session`.
`KIO::PreviewJob` ne passe pas par une interface D-Bus publique pour demander
la miniature. La chaîne observée est :

```text
programme Qt/KF6
  ├─ fork/vfork → /usr/lib/kf6/kioworker
  │                arguments : worker thumbnail, sockets locaux
  ├─ socket Unix privé du type
  │    /run/user/1000/<application>.<n>.kioworker.socket
  └─ protocole KIO/worker privé sur ces sockets
```

Le worker lance ensuite `gsthumbnail.so` et, pour le PDF, Ghostscript est
exécuté comme processus enfant. Les échanges D-Bus observés pendant la
requête ne contiennent aucune méthode thumbnail/preview ; les noms de session
KIO visibles sont ceux d'autres composants KDE, pas un endpoint public de
miniature.

Le binaire `kioworker` expose seulement son interface de lancement interne :

```text
kioworker <worker-lib> <protocol> <klauncher-socket> <app-socket>
```

Ce n'est pas un contrat IPC destiné à un client externe. Réimplémenter les
messages KIO ou fabriquer les sockets n'est donc pas retenu.

### Coût de la frontière d'adaptation

Le petit helper C++ `kio-thumbnail-probe`, lié dynamiquement à Qt 6/KF6 KIO,
fait 122 560 octets en release et dépend d'environ 91 bibliothèques ELF
transitives (`libQt6*`, `libKF6*`, DBus, X11/Wayland et bibliothèques système).
Une génération PDF observée avec ce helper a utilisé environ 61 MiB RSS et
0,38 s de temps processus, dont environ 0,14 s de génération KIO mesurée par
le job. Le résultat est transférable au client par un protocole stdio simple
et anonymisable, sans exposer le protocole KIO.

Un programme Rust non lié à Qt/KF6 ne peut donc pas demander directement la
même preview via une API D-Bus publique découverte sur cette machine. La
solution raisonnable reste un helper KDE/KF6 séparé, lancé à la demande avec
timeout, ou l'usage du cache freedesktop quand une miniature existe déjà.
Le helper doit rester optionnel et désactivable.
