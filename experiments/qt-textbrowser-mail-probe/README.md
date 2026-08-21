# qt-textbrowser-mail-probe

Probe Qt 6 Widgets isolé pour évaluer `QTextBrowser` sur des HTML de mails.
Il ne fait pas partie du workspace Cargo et ne modifie pas Memoria.

Le programme accepte un répertoire local de fichiers `.html`, affiche une
liste et un lecteur, intercepte les liens et bloque les ressources distantes.
Le corpus réel utilisé pendant l'expérience est resté sous `/tmp` et n'est
pas une donnée du dépôt.

```sh
cmake -S experiments/qt-textbrowser-mail-probe -B /tmp/qt-textbrowser-build -DCMAKE_BUILD_TYPE=Release
cmake --build /tmp/qt-textbrowser-build -j2
QT_QPA_PLATFORM=wayland /tmp/qt-textbrowser-build/qt-textbrowser-mail-probe /tmp/qt-textbrowser-corpus-4
```

