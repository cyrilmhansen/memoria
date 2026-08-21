# KIO thumbnail probe

Probe isolé du chemin KDE/KIO, sans lien avec Memoria. Il utilise
`KIO::PreviewJob` et `KIO::ThumbnailCreator` via les bibliothèques KF6
installées par le système.

```text
cmake -S experiments/kio-thumbnail-probe -B /var/tmp/kio-thumbnail-build
cmake --build /var/tmp/kio-thumbnail-build
/var/tmp/kio-thumbnail-build/kio-thumbnail-probe plugins
/var/tmp/kio-thumbnail-build/kio-thumbnail-probe preview FILE 256 [OUTPUT.png]
```

Ce probe appelle l'API publique C++ `KIO::PreviewJob`, mais ne constitue pas
une bibliothèque cliente du protocole worker. Le traçage montre que KIO crée
`/usr/lib/kf6/kioworker` et des sockets Unix privés ; aucun endpoint D-Bus
public de preview n'a été trouvé. Un futur client non-Qt doit appeler un
helper de ce type, et non réimplémenter les messages KIO. Lorsque `OUTPUT.png`
est fourni, le probe écrit lui-même la miniature PNG et renvoie son chemin dans
la réponse JSON; c'est le contrat minimal retenu pour le helper lancé par
Memoria.
