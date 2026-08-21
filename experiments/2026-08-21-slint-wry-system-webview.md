# Expérience — Slint + WebView système

Date : 2026-08-21

## Périmètre

Probe indépendant sous [`experiments/slint-wry-probe/`](slint-wry-probe/).
Memoria, ses dépendances et son profil release n'ont pas été modifiés.

Versions : Slint 1.17.1, wry 0.56.1, WebKitGTK 4.1 système. Le probe active
le raw window handle Slint 0.6, le renderer logiciel et l'accessibilité déjà
utilisés par le workspace.

## Dépendances et taille

Dépendances directes spécifiques au probe :

- `wry 0.56.1` ; WebView2 via le runtime système Windows et WebKitGTK via
  GTK3 sous Linux ;
- `url 2.5` pour fabriquer les URL `file:` ;
- `gtk 0.18` uniquement sous Linux, requis pour `gtk::init()` et le pompage
  de la boucle GTK ;
- `fontique 0.10` est la même adaptation fontconfig que dans Memoria, pas une
  nouvelle dépendance produit.

Le graphe Linux du probe contient 581 crates runtime uniques (`cargo tree`),
et 605 crates en comptant les dépendances de build/dev. Le coût principal
additionnel est la pile GTK/WebKitGTK et ses codecs, pas une seconde UI Slint.

| cible | taille | build |
|---|---:|---:|
| Linux release | 23 720 624 octets | 79,87 s |
| Windows x86-64 MSVC CRT statique | 11 783 680 octets | 41,27 s |

Le PE Windows importe uniquement les DLL système Windows observées par
`llvm-objdump`; aucun moteur Chromium/WebKit n'est embarqué. Le runtime
WebView2 reste une précondition de la machine Windows.

Sur Linux, `ldd` liste 143 bibliothèques système uniques pour le probe. Par
rapport au binaire Memoria, les familles nouvelles visibles comprennent
`libwebkit2gtk-4.1`, `libjavascriptcoregtk-4.1`, `libgtk-3`, `libgdk-3`,
`libsoup-3`, GLib/GObject/GIO, Cairo/Pango/ATK, ainsi que les codecs utilisés
par WebKitGTK. Le chiffre est une mesure de linkage/runtime local, pas une
taille de paquet redistribuable complète.

Le premier `cargo check` sans environnement explicite échoue parce que le
`pkg-config` local pointe vers `/opt/brew`; avec
`PKG_CONFIG_PATH=/usr/lib/pkgconfig:/usr/share/pkgconfig`, GTK3 et WebKitGTK
sont trouvés. Cette contrainte est spécifique à l'environnement de mesure.

## Sécurité du contenu

Le probe charge uniquement un fichier `file:` local et une image SVG locale.
Il configure :

- JavaScript désactivé ;
- autofill général désactivé ;
- navigation autorisée seulement pour les URL `file:` ;
- aucun contenu distant ni WebView Chromium bundlé.

Le lien HTTPS du document est intercepté par le handler de navigation et n'est
pas ouvert. Le champ local de test reçoit le focus et accepte la frappe sous
X11. Le bouton de masquage, le remplacement par un second document local et
le fallback Slint sont également exercés.

## X11

Commande de mesure :

```text
env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 GDK_BACKEND=x11 \
  PKG_CONFIG_PATH=/usr/lib/pkgconfig:/usr/share/pkgconfig \
  xvfb-run -a -s '-screen 0 1280x800x24' \
  experiments/slint-wry-probe/target/release/slint-wry-probe
```

Faits vérifiés :

- `webview_created=true` ;
- le titre, le texte gras, les couleurs, le tableau, le lien et l'image SVG
  locale sont visibles ;
- la molette fait défiler le document et affiche une scrollbar WebKitGTK ;
- le champ HTML reçoit le focus et la frappe ;
- le masquage révèle le rectangle Slint de fallback ;
- le second document local remplace le premier sans recréer la fenêtre ;
- la géométrie suit un redimensionnement 1000×640 → 800×600.

L'intégration nécessite toutefois trois éléments de glue dans le probe :
`gtk::init()`, `GDK_BACKEND=x11` pour ce test explicite, et un timer Slint qui
appelle `gtk::main_iteration_do(false)` puis `WebView::set_bounds()` à chaque
tick. Sans le pompage GTK, la fenêtre enfant existe mais sa surface reste
noire.

## Wayland KDE réel

Commande utilisée dans la session Wayland courante :

```text
env -u DISPLAY PKG_CONFIG_PATH=/usr/lib/pkgconfig:/usr/share/pkgconfig \
  timeout 5s experiments/slint-wry-probe/target/release/slint-wry-probe
```

Fait vérifié : la fenêtre Slint reste vivante, mais wry rapporte
`UnsupportedWindowHandle` pour `build_as_child`. Le fallback Slint reste
affiché.

La documentation/API de wry précise que `build_as_child` Linux est X11-only
et recommande `WebViewBuilderExtUnix::build_gtk` pour Wayland. Cette voie
demanderait de fournir un vrai `gtk::Container`/`gtk::Fixed`, de faire vivre
un modèle de fenêtre GTK parallèle à la fenêtre Slint et de synchroniser son
placement avec Slint. Slint n'expose pas un conteneur GTK dans sa fenêtre
Winit. Cela dépasse le caractère minimal de cette expérience et n'a pas été
introduit dans Memoria.

HiDPI n'a pas été validé sur un écran physique : `GDK_SCALE=2` sous Xvfb ne
constitue pas un facteur HiDPI réel. Le chemin de bounds utilise néanmoins
les tailles physiques retournées par Slint, sans constante de pixels physiques.

## Conclusion

**Fait vérifié :** wry fournit un WebView système utilisable comme enfant de
la fenêtre Slint sous Windows et sous Linux/X11, avec HTML local, scroll,
focus, remplacement et redimensionnement fonctionnels dans le probe.

**Fait vérifié :** la même approche ne peut pas attacher une WebView enfant à
la fenêtre Slint sous Wayland KDE ; wry exige alors une intégration GTK.

**Décision de projet :** ne pas modifier Memoria et ne pas ajouter de WebView
à son lecteur actuel. Le TextEdit reste le fallback multiplateforme. Une
future exploration Wayland devrait d'abord choisir explicitement entre une
fenêtre GTK séparée, une architecture d'intégration différente ou l'abandon
de la WebView pour ce cas d'usage.
