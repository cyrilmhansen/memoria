# slint-qtwebengine-probe

Probe isolé de faisabilité pour Qt 6 WebEngine sous KDE Wayland. Il ne fait
pas partie du workspace Cargo et ne modifie pas Memoria.

Le programme est volontairement un **top-level Qt autonome**. Il mesure le
comportement de `QWebEngineView` avec HTML local, JavaScript désactivé et une
politique qui bloque toute URL autre que `file:`/`data:`. Cela permet de
mesurer le moteur système et de rendre explicite la frontière d'intégration :
ce probe ne prétend pas insérer un QWidget dans une fenêtre Slint/winit.

```sh
cmake -S experiments/slint-qtwebengine-probe -B /tmp/slint-qtwebengine-probe-build
cmake --build /tmp/slint-qtwebengine-probe-build -j2
QT_QPA_PLATFORM=wayland /tmp/slint-qtwebengine-probe-build/slint-qtwebengine-probe
```

`--smoke` ferme après le chargement initial. Le probe dépend des bibliothèques
Qt 6 WebEngine et de `QtWebEngineProcess` installés par le système; il ne
recompile ni n'embarque Chromium.

