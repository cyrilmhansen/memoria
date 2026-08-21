# Expérience — QTextBrowser comme renderer HTML de mails

Date : 2026-08-21  
Statut : probe terminé ; Memoria inchangé.

## Méthode

Probe isolé : [`experiments/qt-textbrowser-mail-probe/`](qt-textbrowser-mail-probe/).
Qt 6 Widgets uniquement (`qt6-base 6.11.2-2`), sans Qt WebEngine, WebKitGTK,
Chromium ni wry.

Un extracteur temporaire local a parcouru les RAW déjà présents dans l'archive
Gmail et a identifié 3 002 messages contenant au moins une feuille
`text/html`. Il a ensuite sélectionné 31 HTML par caractéristiques et tailles
afin d'éviter les 50 premiers messages trop homogènes. Les fichiers de test
sont restés sous `/tmp/qt-textbrowser-corpus-4`; aucun contenu, sujet ou
adresse n'est dans Git ou dans ce rapport.

Échantillon : 31 HTML, moyenne 44 859 octets, médiane 26 597, p90 102 087,
maximum 154 088. 28 contiennent un tableau, 29 un bloc `<style>`, 10 font
plus de 50 KiB ; des références CID sont présentes dans une partie de
l'échantillon. Ces chiffres décrivent le compte et la sélection, pas le
courrier en général.

## Coût

| Mesure | QTextBrowser | Qt WebEngine précédent |
|---|---:|---:|
| binaire probe release dynamique | 65 096 octets | 39 768 octets |
| code C++ du probe | 112 lignes | 78 lignes |
| processus auxiliaires | 0 | 2 `QtWebEngineProcess` |
| RSS après affichage Wayland | ~92 MiB | ~259 MiB principal + ~125 MiB auxiliaires |
| max RSS smoke X11 | 83 876 KiB | 307 364 KiB |
| temps smoke X11 | 5,55 s | 5,92 s |
| paquet Qt principal | `qt6-base`, 66,53 MiB installé | `qt6-webengine`, 282,17 MiB installé |

Le binaire QTextBrowser est un peu plus grand dans ce probe parce qu'il
contient aussi le mini-sélecteur de corpus, mais ses dépendances liées sont
`Qt6Widgets`, `Qt6Gui`, `Qt6Core` et les bibliothèques Qt de base. `ldd` ne
montre aucune bibliothèque WebEngine, WebKitGTK ou Chromium. La différence
de RAM et de processus est la mesure importante : QTextBrowser est beaucoup
plus frugal.

Les temps incluent le lancement de Xvfb et ne sont pas des mesures fines.

## Politique de sécurité vérifiée

- `QTextBrowser` est configuré avec les liens externes désactivés.
- `loadResource()` refuse `http:`, `https:` et les schémas inconnus.
- Les fichiers `file:` sont limités au répertoire explicitement fourni.
- Les ressources `data:` restent traitées par Qt, sans réseau ; leur présence
  doit rester une décision explicite lors d'une future intégration.
- Aucun moteur JavaScript n'est impliqué. Les balises `<script>` ne sont pas
  exécutées ; formulaires, iframe, object et embed ne fournissent pas un
  comportement navigateur.
- Un clic sur un lien est intercepté par `anchorClicked`; le probe ne lance pas
  de navigateur.

Les captures X11 et le lancement Wayland ont confirmé le rendu, la sélection
de la liste, le resize et le scroll. La sélection/copie est fournie par le
contrôle texte Qt (Ctrl+A/Ctrl+C) ; aucun contenu sélectionné n'a été copié
dans un fichier de mesure. Les essais Wayland ont utilisé la session KDE
réelle ; X11 a servi à automatiser les 31 changements de sélection et les
captures locales.

## Classification visuelle

Une passe visuelle des 31 captures a été faite uniquement depuis les images
temporaires locales. La classification vise « lisible/utilisable », pas
l'identité pixel-perfect d'un navigateur :

| Classe | Nombre | Proportion |
|---|---:|---:|
| A — proche et utilisable | 9 | 29 % |
| B — différent mais utilisable | 22 | 71 % |
| C — dégradation gênante | 0 | 0 % |
| D — inutilisable | 0 | 0 % |

Les cas B comprennent des newsletters à tableau et des images non disponibles
localement. Le texte, les titres, liens, styles inline simples, couleurs,
fonds, tableaux et reflow restent lisibles. Les images distantes bloquent comme
prévu et apparaissent comme ressource absente plutôt que comme requête réseau.
La présence CID est détectée dans l'échantillon, mais le probe ne dispose pas
du catalogue MIME nécessaire pour résoudre chaque CID vers un `QImage`; on ne
conclut donc pas que les images CID réelles sont correctement rendues.

QTextBrowser n'est pas un navigateur : CSS complexe, dimensions fixes,
positionnement avancé, images et structures HTML exotiques peuvent différer.
Le compte testé ne fournit pas assez de diversité pour mesurer correctement
les vieux HTML mal formés ou les newsletters très sophistiquées.

## Intégration Qt minimale

Le probe contient une fenêtre `QMainWindow` avec liste, boutons et
`QTextBrowser`. Il confirme la qualité native de base sous Wayland et le coût
d'une petite UI Qt, mais ne constitue pas une décision de réécrire Memoria.
Il n'y a aucun lien avec le core Rust ni avec Slint.

## Conclusion

**Fait vérifié :** sur cet échantillon réel, QTextBrowser restitue des mails
HTML suffisamment lisibles dans tous les cas observés ; A+B = 100 %. Il gère
les structures courantes à ce niveau et apporte sélection, scroll et reflow
sans moteur actif.

**Limite vérifiée :** les images CID ne sont pas encore résolues par le probe,
et l'absence d'image distante est visuellement peu explicite. La fidélité
navigateur, le CSS avancé et les mises en page HTML très complexes restent
ouverts.

**Décision de projet :** QTextBrowser mérite une exploration ultérieure comme
renderer léger, notamment pour un éventuel frontend Qt Linux, mais cette
expérience ne justifie ni l'abandon de Slint ni l'intégration de Qt dans
Memoria.

