# Expérience — probe Qt 6 WebEngine sous KDE Wayland

Date : 2026-08-21  
Statut : arrêtée après validation de la limite d'embedding  
Portée : probe isolé uniquement ; Memoria n'a pas été modifié.

## Résultat court

Qt 6 WebEngine est installé comme dépendance système (`qt6-webengine
6.11.2-1`). Il fonctionne dans une fenêtre Qt top-level sous la session KDE
Wayland réelle, mais cette expérience ne fournit pas de chemin supporté pour
placer un `QWebEngineView` dans la région droite d'une fenêtre Slint/winit.

Le probe est donc volontairement un programme Qt autonome. Il ne crée ni
copie framebuffer→Slint, ni fenêtre Qt enfant de Slint, ni seconde glue
présentée comme une intégration produit.

## Architecture vérifiée

### Fait vérifié

- `QWebEngineView` est un `QWidget` et son usage documenté est celui d'un
  composant enfant d'une hiérarchie QWidget/Qt.
- `WebEngineView` Qt Quick est un `Item` d'une scène Qt Quick, initialisée par
  `QtWebEngineQuick::initialize()` et exécutée avec `QGuiApplication`/la
  boucle Qt.
- Qt WebEngine utilise un processus séparé `QtWebEngineProcess` pour le rendu
  et l'exécution du contenu Chromium.
- Les deux chemins donnent donc à Qt la responsabilité d'une hiérarchie de
  fenêtre/scène et de sa boucle d'événements. Aucun mécanisme public Qt/Slint
  permettant d'insérer directement ces objets dans le `wl_surface` géré par
  Slint/winit n'a été identifié.

Références officielles : [Qt WebEngine Overview](https://doc.qt.io/qt-6/qtwebengine-overview.html),
[`QWebEngineView`](https://doc.qt.io/qt-6/qwebengineview.html),
[`WebEngineView` Qt Quick](https://doc.qt.io/qt-6/qml-qtwebengine-webengineview.html),
[déploiement et processus QtWebEngine](https://doc.qt.io/qt-6/qtwebengine-deploying.html).

### Hypothèse écartée

Faire de Qt Quick le propriétaire de la fenêtre puis afficher Slint à
l'intérieur, ou faire cohabiter deux top-levels synchronisés, ne répond pas à
l'objectif d'une région enfant de la fenêtre Slint. Une copie framebuffer
serait une autre architecture et perdrait le focus, le scroll et la
composition native attendus.

## Probe et sécurité

Sources : `experiments/slint-qtwebengine-probe/`.

Le probe C++/CMake charge un document HTML local dans `QWebEngineView`,
désactive JavaScript, interdit l'accès des contenus locaux aux URLs distantes,
intercepte les requêtes et n'autorise que `file:` et `data:`. Il expose donc
les conditions minimales demandées sans réseau et sans modifier Memoria.

Le HTML contient titre, gras, couleur, tableau, lien externe et une zone haute
permettant de tester scroll, focus et resize dans Qt. Les tests X11 sous Xvfb
et Wayland KDE ont lancé le probe ; le test X11 a signalé une erreur GPU
transitoire liée à Xvfb, sans empêcher le smoke test.

## Mesures locales

Machine : Linux x86-64, Qt 6.11.2, qt6-webengine installé par le système.

| Mesure | Résultat |
|---|---:|
| glue C++ | 78 lignes / 3 fichiers probe |
| binaire Qt probe release | 39 768 octets, dynamique |
| paquet système `qt6-webengine` | 282,17 MiB installé |
| processus principal Wayland | environ 259 MiB RSS après initialisation |
| 2 processus `QtWebEngineProcess` observés | environ 62 + 63 MiB RSS |
| Chromium dans le binaire probe | non ; fourni par bibliothèques/processus système |
| cross-build Windows | non pertinent sans Qt 6/WebEngine SDK Windows installé |

Les RSS sont un point de mesure du probe, pas une promesse de consommation
minimale universelle : GPU, profil, cache, codecs et version Qt peuvent les
modifier. Le paquet système contient néanmoins le moteur Chromium et ses
ressources ; il ne s'agit pas d'une petite bibliothèque WebView native.

## Comparaison avec Wry/WebKitGTK

| Sujet | Wry/WebKitGTK | Qt 6 WebEngine |
|---|---|---|
| intégration Slint/winit testée | enfant possible sous Windows/X11 | aucune insertion native supportée identifiée |
| KDE Wayland | `build_as_child` échoue ; chemin GTK séparé requis | Qt top-level/Qt Quick requis |
| moteur | WebKitGTK système | Chromium/QtWebEngine système |
| boucle supplémentaire | GTK à pomper pour le chemin Linux | boucle Qt et initialisation Qt obligatoires |
| coût observé | bibliothèques GTK/WebKit nombreuses | paquet 282 MiB + processus WebEngine |
| décision Memoria | ne pas intégrer à ce stade | ne pas intégrer à ce stade |

## Décision

**Décision de projet :** arrêter ce probe avant une intégration Slint. Qt
WebEngine est utilisable comme navigateur Qt autonome, mais son modèle de
composition ne satisfait pas la contrainte d'une région dans la fenêtre
Slint/winit sans introduire précisément les coûts et responsabilités exclus
par l'expérience. Le fallback texte de Memoria reste la solution retenue.

