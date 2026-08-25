# Knowledge map

Carte concise des conclusions techniques vérifiées et réutilisables entre les
applications. Les expériences détaillées, mesures et échecs restent dans
[`experiments/`](experiments/).

## État initial

- **Décision de projet — vérifiée le 2026-08-20 :** le dépôt démarre comme un
  workspace Cargo virtuel sans application ni dépendance. Voir
  [`Cargo.toml`](Cargo.toml).
- **Décision de projet — vérifiée le 2026-08-20 :** les futurs projets sont
  réservés sous [`projects/`](projects/) sans architecture prédéfinie.
- **Décision de projet — vérifiée le 2026-08-20 :** les détails expérimentaux
  vont dans [`experiments/`](experiments/) ; cette carte ne conserve que les
  conclusions réutilisables.

## Conclusions par sujet

Cette section sera enrichie uniquement par des expériences reproductibles.

- **Slint 1.17.1 — fait vérifié le 2026-08-20 :** le code Rust est généré
  depuis `ui/app.slint` par `slint-build`; la configuration retenue utilise
  le backend Winit, le renderer logiciel et l'accessibilité, sans WebView.
  Les dépendances et limites sont détaillées dans
  [`README.md`](README.md) et
  [`experiments/2026-08-20-slint-demo.md`](experiments/2026-08-20-slint-demo.md).
- **Linux/fontconfig — fait vérifié le 2026-08-20 :** le build Slint peut
  tirer `yeslogic-fontconfig-sys`; `RUST_FONTCONFIG_DLOPEN=1` évite une
  dépendance à un chemin Homebrew/pkg-config fixe. La dépendance directe
  `fontique` active aussi la feature transitive requise par Slint 1.17.1.
  Voir [`.cargo/config.toml`](.cargo/config.toml) et les notes d'expérience.
- **Cargo — fait vérifié le 2026-08-20 :** `cargo check` refuse un workspace
  virtuel sans membre ; un paquet racine vide sert donc d'ancre compilable
  jusqu'à l'arrivée d'une première application. Voir l'expérience dans
  [`WORKLOG.md`](WORKLOG.md).
- **Performance — mesure locale Linux release le 2026-08-20 :** le binaire
  fait `26 976 736` octets ; la préparation du dataset prend `13 ms` et le RSS
  passe de `5 604 KiB` sans éléments à `12 896 KiB` avec 100 000 éléments.
  Ce n'est pas une mesure de fenêtre affichée ; voir
  [`experiments/2026-08-20-slint-demo.md`](experiments/2026-08-20-slint-demo.md).
- Rust : aucune autre conclusion locale vérifiée.
- Windows : compilation et essais réels à faire sur Windows.
- Linux / Wayland : compilation vérifiée, compositor réel à vérifier.
- Android : hors périmètre de cette démonstration.
- Accessibilité : rôles Slint/AccessKit déclarés, lecteur d'écran réel non
  vérifié localement.
- Packaging : aucun installeur ni paquet généré pour le moment.
- Problèmes rencontrés et solutions validées : fontconfig traité par
  chargement dynamique, comme décrit ci-dessus.
- **Memoria — fait vérifié le 2026-08-21 :** pour un binaire release GUI,
  `strip=true`, LTO complet et `codegen-units=1` réduisent fortement la taille
  sans régression observée dans les tests ni le smoke test Linux. Le profil
  est fixé dans le `Cargo.toml` racine ; `panic=abort` reste expérimental car
  il change le contrat des panics non récupérées. ZIP/7z sont des compressions
  de transport ; UPX reste candidat non décidé jusqu'à une validation native
  Windows du démarrage, SmartScreen/antivirus, OAuth et d'une future
  signature. Mesures détaillées dans
  [`experiments/2026-08-21-mail-archive-release-profiles.md`](experiments/2026-08-21-mail-archive-release-profiles.md).
- **Décision de projet :** les builds CI ordinaires utilisent le profil Cargo
  `ci` (`inherits = "release"`, ThinLTO, 8 unités de codegen) ; le profil
  `release` conserve le fat LTO et `codegen-units=1` pour les binaires de
  distribution. Le build CI Linux local mesuré fait 36 609 152 octets contre
  31 070 752 octets pour le release historique ; aucun run GitHub n'est encore
  associé à cette modification non poussée.
- **Slint/WebView — fait vérifié le 2026-08-21 :** wry 0.56.1 peut attacher
  une WebView système à une fenêtre Slint/Winit sous Windows et Linux/X11,
  mais `build_as_child` ne supporte pas Wayland. Wayland demande le chemin
  `build_gtk` et donc un conteneur GTK ainsi qu'une glue de boucle/fenêtre que
  Memoria n'adopte pas à ce stade. Le probe et les mesures sont dans
  [`experiments/2026-08-21-slint-wry-system-webview.md`](experiments/2026-08-21-slint-wry-system-webview.md).
- **Qt WebEngine — fait vérifié le 2026-08-21 :** Qt 6 WebEngine fonctionne
  comme fenêtre Qt top-level sous KDE/Wayland, avec `QWebEngineView` (QWidget)
  ou `WebEngineView` (Qt Quick), mais ces chemins imposent la hiérarchie de
  fenêtre/scène et la boucle Qt, ainsi que des processus `QtWebEngineProcess`.
  Aucun embedding public direct dans une fenêtre Slint/winit n'a été établi ;
  ne pas intégrer Qt WebEngine à Memoria pour l'instant. Voir
  [`experiments/2026-08-21-slint-qtwebengine-system-webview.md`](experiments/2026-08-21-slint-qtwebengine-system-webview.md).
- **QTextBrowser — fait vérifié le 2026-08-21 :** sur 31 HTML réels
  sélectionnés localement, `QTextBrowser` a fourni un rendu lisible (A+B 100 %)
  sans moteur actif ni processus auxiliaire, avec une empreinte nettement
  inférieure à Qt WebEngine. Les images CID et le CSS HTML complexe restent
  ouverts ; détails dans
  [`experiments/2026-08-21-qt-textbrowser-mail-rendering.md`](experiments/2026-08-21-qt-textbrowser-mail-rendering.md).
- **Pièces jointes Memoria — fait vérifié le 2026-08-21 :** l'extraction à la
  demande depuis le RAW fonctionne sans modifier l'archive ; les attachments
  stricts/nommés sont séparés des ressources inline/CID, et les octets décodés
  peuvent être ouverts ou enregistrés après assainissement du nom. Voir
  [`experiments/2026-08-21-mail-archive-attachments-ui.md`](experiments/2026-08-21-mail-archive-attachments-ui.md).
- **Progression Gmail — fait vérifié le 2026-08-21 :** une full sync peut
  exposer `examined/total` après son unique parcours paginé des IDs, tandis
  qu'une sync history reste indéterminée sans second parcours réseau. Les
  nouveaux messages et les octets réellement reçus restent séparés. Voir
  [`experiments/2026-08-21-mail-archive-sync-progress.md`](experiments/2026-08-21-mail-archive-sync-progress.md).
- **Dette d'échelle — à mesurer :** la full sync conserve actuellement les
  IDs Gmail énumérés afin de connaître exactement `N` avant traitement. Cette
  stratégie est adaptée aux volumes validés, mais devra être mesurée à
  l'échelle de millions de messages ; alternatives possibles : représentation
  compacte des IDs ou spool temporaire, sans second parcours réseau.
- **Mail archive — fait vérifié le 2026-08-20 :** un prototype isolé sous
  [`projects/mail-archive/`](projects/mail-archive/) sépare archive brute
  append-only segmentée, catalogue structuré et index dérivés SQLite/Tantivy.
  Les détails et mesures restent dans
  [`experiments/2026-08-20-mail-archive.md`](experiments/2026-08-20-mail-archive.md).
- **Mail archive — décision de projet :** les index de recherche ne sont pas
  une source d'autorité et doivent pouvoir être reconstruits sans réécrire les
  segments d'archive.
- **Mail archive — fait vérifié le 2026-08-20 :** à 100 000 puis 1 000 000
  messages synthétiques, le chemin archive→catalogue→lecture→parsing est
  mesurable et Tantivy a une latence de recherche nettement inférieure à FTS5
  dans ce workload, au prix d'une dépendance et d'un index dérivé distinct.
  Ce résultat n'est pas une preuve de tenue à 300 Go. Voir
  [`experiments/2026-08-20-mail-archive-scale.md`](experiments/2026-08-20-mail-archive-scale.md).
- **Mail archive — fait vérifié le 2026-08-20 :** 64 MiB est un compromis
  provisoire raisonnable entre 16 et 256 MiB ; les coûts de sauvegarde et de
  reprise à grande échelle restent ouverts.
- **Mail archive — hypothèse renforcée :** la déduplication des pièces
  jointes mérite un CAS expérimental séparé ; le corpus 10 000 messages/30 %
  présente 98,5 MiB de contenu pour 1,44 MiB uniques. Aucun format définitif
  n'est adopté.
- **Mail archive — fait vérifié le 2026-08-20 :** le corpus synthétique
  dispose désormais de profils `light`, `personal` et `heavy`, avec taille
  asymétrique et taux de duplication explicite. Le profil heavy atteint
  environ 5 Go pour 5 000 messages ; les profils et mesures sont dans
  [`experiments/2026-08-20-mail-archive-corpus-profiles.md`](experiments/2026-08-20-mail-archive-corpus-profiles.md).
- **Mail archive — décision de projet :** les conclusions CAS ne doivent pas
  être tirées du profil light ; l'expérience CAS suivante sera limitée à
  personal/heavy et comparera inline à contenu adressé.
- **Mail archive — fait vérifié le 2026-08-20 :** externaliser les payloads
  MIME dans un CAS transformé est nécessaire pour économiser ; ajouter un CAS
  à côté d'une copie MIME brute ne ferait que dupliquer les octets. Le
  prototype `cas-exact` reconstruit byte pour byte les fixtures testées et
  économise 12,2 % sur personal/10k dans cette représentation. Voir
  [`experiments/2026-08-20-mail-archive-cas.md`](experiments/2026-08-20-mail-archive-cas.md).
- **Mail archive — décision de projet :** le contrat recommandé pour
  restauration/migration reste une représentation byte-exacte exportable ; un
  CAS doit rester reconstructible et facultatif tant que le décodage MIME réel
  n'est pas validé.
- **Gmail — fait vérifié le 2026-08-20 :** l’API `format=RAW` fournit une
  représentation base64url qui peut être décodée puis conservée byte-exacte
  dans les frames existantes ; les métadonnées Gmail restent dans le catalogue
  séparé. Voir
  [`experiments/2026-08-20-mail-archive-gmail-readonly.md`](experiments/2026-08-20-mail-archive-gmail-readonly.md).
- **Gmail — décision de projet :** le premier connecteur utilise uniquement
  `gmail.readonly`, conserve les suppressions comme état source sans supprimer
  l’archive, et ne calcule le CAS réel qu’en lecture seule.
- **Gmail — fait vérifié par fixtures :** pagination, identité
  `source_account + gmail_message_id`, seconde synchronisation sans nouvelle
  frame et repli après expiration d’historique sont couverts sans compte réel.
  Les statistiques d’un compte réel restent ouvertes jusqu’à une campagne
  locale autorisée.
- **MIME — fait vérifié le 2026-08-20 :** `mailparse 0.16.1` est adapté à une
  analyse dérivée des payloads encodés/décodés, mais les bytes RAW doivent rester
  l’autorité car les APIs de parsing peuvent normaliser la représentation.
- **Gmail réel — fait vérifié le 2026-08-20 :** une campagne anonymisée de
  1 000 messages a validé RAW-inline, checksums, métadonnées séparées et
  idempotence ; le chemin incrémental sans changement est vide. Voir
  [`experiments/2026-08-20-mail-archive-gmail-real-1000.md`](experiments/2026-08-20-mail-archive-gmail-real-1000.md).
- **Gmail réel — limite vérifiée :** cette tranche ne contenait aucune feuille
  MIME candidate comme pièce jointe (`attachment`, `filename`, `name` ou
  `Content-ID`). Elle ne permet donc pas d’estimer le CAS ou la duplication
  réelle des pièces jointes du compte complet.
- **Gmail réel — fait vérifié le 2026-08-20 :** `has:attachment` peut
  sélectionner des messages dont les feuilles sont `inline` mais portent
  `filename`/`name` ou `Content-ID`. Les statistiques doivent distinguer
  attachment strict et candidat nommé ; voir
  [`experiments/2026-08-20-mail-archive-gmail-attachments.md`](experiments/2026-08-20-mail-archive-gmail-attachments.md).
- **Gmail réel — fait vérifié le 2026-08-20 :** deux échantillons contenant
  37 puis 7 messages avec pièces jointes ont conservé des RAW vérifiables ;
  aucun doublon de payload n’a été observé dans ces échantillons, sans que cela
  permette d’extrapoler au compte complet.
- **Gmail réel — fait vérifié le 2026-08-20 :** la full sync contrôlée a
  atteint 3 012 messages, 211,8 MB RAW et 4 segments ; `complete=true` puis
  une relance `history` sans changement n’a ajouté aucun byte. Voir
  [`experiments/2026-08-20-mail-archive-gmail-full-sync.md`](experiments/2026-08-20-mail-archive-gmail-full-sync.md).
- **Gmail réel — conclusion corrigée le 2026-08-21 :** les 102 candidats MIME
  représentent environ 20 % des octets RAW ; l’agrégation corrigée trouve
  1,43 % de duplication encodée, 1,50 % décodée et 0,72 % sur les candidats
  encodés >64 KiB. Voir le rapport full sync ; le gain reste faible mais n’est
  pas nul.
- **Gmail réel — fait vérifié le 2026-08-21 :** en réconciliation, les Gmail
  IDs connus peuvent être rafraîchis par `format=METADATA` pour labels/history/
  dates/thread, tandis que seuls les IDs nouveaux nécessitent `format=RAW`.
  Une fixture 1 000 connus + 10 nouveaux vérifie 1 000 appels metadata et 10
  appels RAW. La taille exacte des réponses metadata reste non instrumentée.
- **Mail archive — fait vérifié le 2026-08-21 :** Tantivy 0.26.1 indexe les
  3 012 RAW Gmail via `catalogue → lecture frame → mailparse` sans accès Gmail
  ni échec MIME. L’index dérivé fait environ 11,1 MB ; une relance saute les
  3 012 documents inchangés et une reconstruction depuis RAW+catalogue réussit.
  Les détails de latence et les limites HTML sont dans
  [`experiments/2026-08-21-mail-archive-gmail-tantivy.md`](experiments/2026-08-21-mail-archive-gmail-tantivy.md).
- **Mail archive — décision de projet :** l’API de recherche dérivée reste
  indépendante de Slint et du CLI ; elle renvoie des résultats paginés avec
  `doc_id`, score, date et identité d’archive, puis permet de relire le RAW.
  BM25 reste le classement de référence ; embeddings et reranking restent
  hors périmètre.
- **Mail archive — fait vérifié le 2026-08-21 :** les filtres structurés
  (expéditeur, destinataire, bornes de date, présence/MIME de pièce jointe et
  labels) sont évalués dans Tantivy avant la limite de résultats. Les champs
  dérivés exacts ajoutés pour MIME/labels et le fast-field de date augmentent
  l’index Gmail réel d’environ 0,7 % ; les détails, limites et tests sont dans
  [`experiments/2026-08-21-mail-archive-advanced-search.md`](experiments/2026-08-21-mail-archive-advanced-search.md).
- **Décision de projet :** une requête sans texte mais avec filtres est valide
  et retourne les messages les plus récents ; une requête vide sans filtre
  reste neutre. Les labels sélectionnés ont une sémantique AND et la borne
  haute de date est exclusive.
- **Fait vérifié le 2026-08-21 :** sur un corpus structuré déterministe de
  1 000 000 messages, l’index Tantivy fait environ 136,6 MB et les requêtes
  combinées restent sous environ 12 ms au p95, mais le RSS de pointe atteint
  environ 1,2 GiB. La taille reste presque linéaire entre 100k et 1M ; la
  mémoire devient la prochaine incertitude prioritaire. Voir
  [`experiments/2026-08-21-mail-archive-structured-search-1m.md`](experiments/2026-08-21-mail-archive-structured-search-1m.md).
- **Fait vérifié le 2026-08-21 :** la reconstruction Tantivy ne doit pas
  matérialiser toutes les lignes du catalogue ni toutes les mises à jour de
  l'état dérivé. Une itération SQLite et une transaction bornée réduisent le
  pic 1M d'environ 1,23 GiB à environ 0,80 GiB, sans modifier RAW, catalogue
  ou résultats. Tantivy reste alors le principal poste mémoire observable ;
  détails dans
  [`experiments/2026-08-21-mail-archive-index-memory-1m.md`](experiments/2026-08-21-mail-archive-index-memory-1m.md).
- **Fait vérifié le 2026-08-21 :** sur le corpus structuré 1M, le réglage
  produit `Index::writer(50_000_000)` est un compromis meilleur que 64 MiB,
  le minimum valide ou un seul worker ; un seul merger est pratiquement
  neutre et `NoMergePolicy` est nettement défavorable. La configuration
  produit reste inchangée et dynamique selon le matériel. Voir
  [`experiments/2026-08-21-mail-archive-tantivy-writer-tuning.md`](experiments/2026-08-21-mail-archive-tantivy-writer-tuning.md).
- **Mail archive — fait vérifié le 2026-08-21 :** une première UI Slint hors
  ligne ouvre l’archive réelle, recherche jusqu’à 50 résultats, sélectionne un
  document au clavier/souris et affiche une vue texte du RAW dérivée dans un
  thread de fond. Le contrôleur mesure environ 2 ms pour une recherche et 3 ms
  pour lecture/parsing d’un message dans cette archive. Voir
  [`experiments/2026-08-21-mail-archive-first-ui.md`](experiments/2026-08-21-mail-archive-first-ui.md).
- **Décision de projet :** ne pas ajouter de WebView ni de moteur de recherche
  supplémentaire pour cette première UI ; vérifier ensuite la densité,
  l’accessibilité et le HiDPI sur des écrans Windows/Linux réels.
- **Slint/Linux — fait vérifié le 2026-08-21 :** la première UI mail démarre
  directement dans une session KDE/Wayland réelle avec le backend Wayland
  natif ; le rendu desktop et le focus initial sont corrects. Les détails et
  limites de validation sont dans
  [`experiments/2026-08-21-mail-archive-first-ui-wayland.md`](experiments/2026-08-21-mail-archive-first-ui-wayland.md).
- **Limite durable :** l’accessibilité AT-SPI, un facteur HiDPI supérieur à 1
  et l’injection automatisée de clavier n’ont pas été vérifiés dans cette
  session ; ne pas les présenter comme couverts par ce test.
- **Slint/UI — fait vérifié le 2026-08-21 :** un délégué `ListView` doit avoir
  une largeur explicite lorsqu’il contient une ligne desktop composée ; sans
  cela, les données peuvent être présentes mais invisibles. Les tailles
  préférées et minimales de `Window` doivent être séparées pour conserver le
  redimensionnement. Voir
  [`experiments/2026-08-21-mail-archive-first-ui-manual-fixes.md`](experiments/2026-08-21-mail-archive-first-ui-manual-fixes.md).
- **Slint/UI — décision de projet :** pour ce prototype, le chrome du lecteur
  reste fixe et seul le corps dérivé défile ; le HTML est reflué en texte sans
  WebView. Une restitution HTML fidèle reste ouverte.
- **Slint/accessibilité — fait vérifié le 2026-08-21 :** avec AccessKit et
  AT-SPI activé dans la session, Slint expose la fenêtre, l'entrée de
  recherche, les boutons, la liste et la région de lecture sous Wayland. Les
  détails du contrôle et les limites d'injection sont dans
  [`experiments/2026-08-21-mail-archive-keyboard-atspi.md`](experiments/2026-08-21-mail-archive-keyboard-atspi.md).
- **Slint/UI — décision de projet :** Échap revient d'abord du message vers
  la liste ; Ctrl+F revient à la recherche ; Ctrl+plus/Ctrl−/Ctrl+0 ajuste le
  corps du message. Les raccourcis sont complétés par des boutons accessibles
  pour le zoom.

## Références de départ

- **Slint/UI — fait vérifié le 2026-08-21 :** `TextEdit` en lecture seule
  fournit une sélection/copier et un défilement avec ascenseur adaptés à un
  lecteur de texte dérivé ; il permet de garder le chrome du lecteur fixe.
  Une `ListView` peut conserver sa sélection visible en ajustant son
  `viewport-y` pour une ligne de hauteur fixe. Détails :
  [`experiments/2026-08-21-mail-archive-first-ui-desktop-structure.md`](experiments/2026-08-21-mail-archive-first-ui-desktop-structure.md).
- **Décision de projet :** le premier lecteur HTML reste un fallback texte :
  URL masquées comme liens externes non cliquables, sans promesse sur
  typographie, couleurs, images ou structure HTML ; le RAW reste autoritaire.
- **Mail archive — fait vérifié le 2026-08-21 :** Memoria peut appeler le
  connecteur Gmail readonly dans un worker, valider l’archive, mettre à jour
  Tantivy avec `index_gmail_archive`, puis recharger le reader sans redémarrer.
  Les snapshots de progression restent indépendants du CLI ; voir
  [`experiments/2026-08-21-mail-archive-sync-ui.md`](experiments/2026-08-21-mail-archive-sync-ui.md).
- **Décision de projet :** l’interface conserve deux espaces seulement,
  Recherche/Consultation et Archive/Synchronisation. Sans credentials, la
  consultation locale reste accessible et aucune autorisation OAuth n’est
  lancée au démarrage.
- **Memoria — fait vérifié le 2026-08-21 :** l’application peut démarrer sans
  `--archive`, rouvrir une archive récente configurée ou proposer l’ouverture
  et la création d’une archive vide. La configuration légère est séparée des
  données d’archive dans `memoria/config.json`; les credentials et tokens
  restent hors archive. Détails et limites :
  [`experiments/2026-08-21-mail-archive-launcher-source.md`](experiments/2026-08-21-mail-archive-launcher-source.md).
- **Décision de projet :** l’ajout Gmail depuis Memoria reste explicite et
  pragmatique : sélection d’un client OAuth Desktop local, scope readonly,
  puis réutilisation du flux loopback existant. Aucun OAuth automatique au
  démarrage et aucune UI multi-source à ce stade.
- **Windows — fait vérifié le 2026-08-21 :** le même crate Memoria produit un
  EXE `x86_64-pc-windows-msvc` avec `cargo-xwin`; les 18 tests de bibliothèque
  compilés MSVC passent via Wine. Le binaire release est configuré GUI sans
  console parasite. Le rapport détaille les DLL système et les limites Wine :
  [`experiments/2026-08-21-mail-archive-windows-port.md`](experiments/2026-08-21-mail-archive-windows-port.md).
- **Décision de projet :** RAW + catalogue restent le contrat portable ;
  Tantivy demeure dérivé et reconstructible par plateforme/version si
  nécessaire. La validation native Windows de l’UX, HiDPI, AccessKit et OAuth
  reste obligatoire avant de déclarer le port complet.
- **Windows — fait vérifié le 2026-08-21 :** `RUSTFLAGS="-C
  target-feature=+crt-static"` produit une variante MSVC fonctionnelle ; les
  tests de bibliothèque passent et `VCRUNTIME140.dll`/Universal CRT ne sont
  plus importées. Elle est environ 170 KiB plus grande et reste candidate,
  non encore publiée par la CI. Voir le rapport Windows.
- **Memoria — audit de dépendances fait vérifié le 2026-08-21 :** Slint est
  déjà en `default-features = false` avec seulement Winit/software/accessibilité;
  Reqwest n’utilise que Rustls; rfd conserve Wayland/xdg-portal pour les
  dialogues Linux. La tentative de retirer stemmer/stopwords de Tantivy
  économise environ 346 KiB mais ne change pratiquement pas l’index ni les
  latences, donc les defaults Tantivy restent conservés. Détails :
  [`experiments/2026-08-21-mail-archive-dependency-audit.md`](experiments/2026-08-21-mail-archive-dependency-audit.md).

Pointeurs officiels à consulter au besoin, sans les considérer comme des
résultats d'expérience locale :

- [Slint documentation](https://docs.slint.dev/)
- [The Rust Book](https://doc.rust-lang.org/book/)
- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)

## 2026-08-21 — Miniatures système

- **Fait vérifié :** sous KDE Wayland, le cache freedesktop et les providers
  `.thumbnailer` installés fournissent des miniatures réelles sans lier de
  renderer PDF/Office/vidéo à l'application. La couverture dépend toutefois
  des providers présents et peut produire `unavailable` ou `error`.
- **Contrat retenu pour l'expérience :** accepter uniquement une image valide,
  avec timeout et possibilité de désactiver les previews ; ne jamais confondre
  une icône de fichier avec une miniature.
- **Limite :** le backend Windows `IShellItemImageFactory` compile, mais la
  validation native Windows de l'isolation et des providers reste ouverte.
- **Détails :** `experiments/2026-08-21-system-thumbnail-service.md`.
- **Correction vérifiée :** l'absence de `.thumbnailer` PDF ne signifie pas
  l'absence de support KDE. `KIO::PreviewJob` découvre
  `gsthumbnail.so` (`application/pdf` notamment), lance le worker KIO hors
  processus et produit une miniature PDF sur cette machine. Sous KDE, KIO est
  le backend à essayer avant le fallback freedesktop. Voir le rapport système
  et `experiments/kio-thumbnail-probe/`.
- **IPC KIO — fait vérifié :** `KIO::PreviewJob` lance
  `/usr/lib/kf6/kioworker` et communique avec lui via des sockets Unix privés ;
  aucun endpoint D-Bus public de preview n'a été observé. Un client Rust ne
  doit pas réimplémenter ce protocole interne. Le petit helper KF6 est la
  frontière d'adaptation provisoire. Détails dans le rapport système.

## 2026-08-21 — Première preview Memoria

- **Fait vérifié :** Memoria peut demander une PNG au helper KIO ou au probe
  freedesktop sans lier Qt/KF6 au binaire principal. L'appel est hors thread
  UI, borné par timeout et désactivable par configuration d'exécution.
- **Décision de projet :** l'overlay de lecture reste temporaire et les
  erreurs de provider n'empêchent ni la lecture du message ni Ouvrir/
  Enregistrer sous. RAW/catalogue/index ne changent pas.
- **Détails et mesures :**
  [`experiments/2026-08-21-mail-archive-attachment-preview.md`](experiments/2026-08-21-mail-archive-attachment-preview.md).
- **Validation Wayland :** un PDF réel a été prévisualisé via KIO dans
  l'overlay Memoria ; le helper doit être fourni explicitement en développement
  ou installé à côté de l'application/dans `PATH`. Un helper absent déclenche
  le fallback freedesktop puis `unavailable` sans empêcher la lecture.

## 2026-08-21 — HTML dans le navigateur système

- **Fait vérifié :** un serveur éphémère `127.0.0.1` avec token aléatoire peut
  servir un HTML MIME dérivé et ses ressources CID sans ajouter de WebView au
  binaire Memoria. Sessions et CID sont gardés en mémoire puis invalidés à la
  fin du processus.
- **Décision de projet :** utiliser `ammonia` et une CSP stricte ; scripts,
  formulaires, objets/iframes et ressources distantes sont bloqués, tandis que
  les liens externes nécessitent une action explicite.
- **Détails, dépendances et limites :**
  [`experiments/2026-08-21-mail-archive-html-browser.md`](experiments/2026-08-21-mail-archive-html-browser.md).

## 2026-08-21 — Internationalisation et identifiants

- **Fait vérifié :** Slint 1.17.1 n’offre pas de catalogue i18n applicatif
  intégré dans l’API utilisée par Memoria ; le petit catalogue Rust local
  couvre FR/EN sans nouvelle dépendance runtime.
- **Décision de projet :** la locale système sélectionne FR pour les locales
  `fr*`, sinon EN ; les pluriels sont centralisés et les identifiants
  protocole/schéma restent hors traduction.
- **Détails :** `experiments/2026-08-21-mail-archive-i18n-identifiers.md`.

- **Fait vérifié :** les sessions HTML sont bornées à 8 et expirent après
  10 minutes ; la CSP interdit les chargements réseau automatiques.
- **Correction vérifiée :** les références `cid:` doivent être normalisées
  (angles et percent-encoding) avant sanitisation, puis comparées exactement
  au `Content-ID` MIME. Les réponses CID conservent leur MIME image pour
  `nosniff`; les ressources HTTP/HTTPS restent bloquées. Voir le rapport HTML.

## 2026-08-21 — Audit dépendances et sécurité Memoria

- **Fait vérifié :** le serveur HTML local utilise uniquement `std::net` ;
  `tokio`/`hyper` proviennent de Reqwest et ne servent pas au listener local.
- **Fait vérifié :** `ammonia 4.1.4` ajoute html5ever/markup5ever/cssparser,
  sans moteur navigateur ni dépendance Qt/KF6/GTK au binaire Memoria.
- **Sécurité :** `cargo-audit 0.22.2` signale zéro vulnérabilité connue, mais
  une alerte unsoundness `lru 0.16.4` transitive de Tantivy ; sa correction
  nécessite une version de Tantivy acceptant `lru >=0.18.2`. Voir le rapport
  d’audit pour l’évaluation d’exploitabilité et les warnings de maintenance.
- **Taille :** le binaire release Linux courant fait 31 070 752 octets ; les
  +692 KiB historiques après i18n ne sont pas attribuables au catalogue sans
  rebuild contrôlé bit-à-bit.
- **Rapport :** `experiments/2026-08-21-mail-archive-dependency-security-audit.md`.

## 2026-08-22 — Indexation bornée du texte des pièces jointes

- **Fait vérifié :** le champ Tantivy dérivé `attachment_text` permet de
  retrouver un message par un terme présent uniquement dans une pièce jointe,
  sans modifier le RAW ni le catalogue.
- **Décision de projet :** supporter `text/*` par décodage interne et PDF par
  le provider système optionnel `pdftotext`, avec entrée/sortie bornées et
  timeout ; ne pas embarquer de moteur Office généraliste pour le moment.
- **Fait vérifié sur le corpus réel :** 25 pièces jointes observées, 20
  formats traitables, 19 textes produits, 5 non supportées et 0 échec
  d’extraction bloquant. Détails :
  [`experiments/2026-08-22-mail-archive-attachment-text-indexing.md`](experiments/2026-08-22-mail-archive-attachment-text-indexing.md).
- **Décision de projet :** la découverte et la sélection des extracteurs sont
  centralisées dans `attachment_text`: `Automatic` choisit
  `memoria-text` pour `text/*` et `poppler-pdftotext` pour PDF lorsqu’il est
  disponible. Le chemin reste un diagnostic, pas un identifiant de
  compatibilité ; la version n’est pas interrogée pour éviter tout blocage.
  `display_name` est également diagnostique : une future UI traduira depuis
  l’ID technique. La découverte est mise en cache par `OnceLock` pour la durée
  du processus ; une future surface Settings pourra proposer un refresh.
  Aucun écran Settings n’existe encore ; l’API est prête à l’alimenter.

## 2026-08-22 — Probe Windows IFilter

- **Fait vérifié :** un helper isolé peut appeler l’API Windows officielle
  `LoadIFilter` puis `IFilter::Init`/`GetChunk`/`GetText` avec les bindings
  `windows`, sans charger un filtre tiers dans Memoria.
- **Limite vérifiée :** l’environnement Linux ne permet pas d’observer les
  handlers enregistrés sur Windows. PDF/DOCX et leur disponibilité réelle
  restent à valider sur Windows natif.
- **Décision de projet :** ne pas intégrer `windows-ifilter` dans la sélection
  Memoria avant cette campagne native ; les CLSID/DLL concrets resteront des
  diagnostics, pas des identifiants de provider.
- **Probe :** `experiments/windows-ifilter-probe/` et son rapport.

- **Fait vérifié sur Windows 11 Pro 25H2 x64 :** HTML est extrait par
  `LoadIFilter`; le handler PDF Windows fonctionne par CLSID direct avec
  `IPersistStream`, mais `LoadIFilter` renvoie `E_NOINTERFACE` sur ce poste.
  Le fixture DOCX est rejeté par `IFilter::Init` (`FILTER_E_UNKNOWNFORMAT`) et
  le fixture TXT ne produit aucun chunk.
- **Décision de projet :** ne pas intégrer encore `windows-ifilter` ; une
  intégration devrait découvrir dynamiquement les handlers et conserver le
  helper isolé, sans CLSID codé en dur.

### Mise à jour du probe natif

- **Fait vérifié :** la résolution dynamique extension/`PersistentHandler`/
  `PersistentAddinsRegistered`/`IID_IFilter` retrouve sur N16PRO les handlers
  `.txt`, `.html`, `.pdf` et `.docx`, sans CLSID codé en dur.
- **Fait vérifié :** PDF est validé par le modèle moderne `CoCreateInstance` +
  `IPersistStream` + `IFilter`; l’échec `LoadIFilter` n’est pas un rejet.
- **Fait vérifié :** timeout, sortie excessive et crash contrôlés sont bornés
  par terminaison et attente du processus enfant.
- **Limite :** Word COM n’a pas produit le DOCX contrôlé en 30 secondes sur le
  runner; DOCX reste inconclusif. Aucun backend produit n’est encore intégré.

### Intégration produit PDF Windows

- **Décision :** intégrer `windows-ifilter` uniquement pour
  `application/pdf`; Automatic le préfère à `poppler-pdftotext` sous Windows.
- **Fait vérifié dans le code :** Memoria utilise un helper séparé, un fichier
  temporaire `.pdf` contrôlé et des bornes parentales de 64 MiB / 8 MiB / 10 s.
  Le helper résout le handler dynamiquement; aucun CLSID n’est dans le code
  produit. Linux conserve son chemin `pdftotext`.
- **Limite :** DOCX reste explicitement inconclusif et hors support.

### Intégration produit DOCX Windows

- **Fait vérifié :** sur Windows natif, deux documents Word contrôlés ont été
  extraits par le handler `.docx` résolu dynamiquement via le helper isolé.
- **Décision de projet :** `windows-ifilter` couvre désormais
  `application/pdf` et
  `application/vnd.openxmlformats-officedocument.wordprocessingml.document`
  sous Windows. Le helper choisit uniquement les extensions contrôlées `.pdf`
  et `.docx`; aucun CLSID Office n’est codé en dur.
- **Fait vérifié :** le test produit DOCX retrouve un terme présent uniquement
  dans la pièce jointe via `attachment_text` et Tantivy. Linux reste inchangé :
  PDF via `pdftotext`, DOCX non supporté.
- **Limite :** Word COM Automation a été testé dans une session interactive,
  mais ne fait pas partie de Memoria et n’est pas requis par l’IFilter.

## 2026-08-22 — Probe IMAP

- **Fait vérifié :** avec GreenMail 2.1.12, `async-imap` 0.11.3 et
  `BODY.PEEK[]`, 12 fixtures MIME synthétiques sont récupérées byte-for-byte,
  y compris via un client Windows vers un serveur Linux sur le LAN.
- **Décision de projet :** IMAP est exploitable pour une future intégration
  readonly, avec UID associé à mailbox + UIDVALIDITY et aux métadonnées
  flags/INTERNALDATE ; le client async devra rester derrière un worker. Le
  probe utilise Tokio ; `async-imap` supporte notamment Tokio et async-std, et
  le choix du runtime produit reste ouvert.
- **Fait vérifié :** avec une CA de test dédiée chargée dans le
  `RootCertStore` rustls, IMAPS GreenMail 2.1.12 est validé depuis Linux et
  Windows ; le certificat serveur porte un SAN cohérent et aucun verifier
  dangereux n’est utilisé sur le chemin principal.
- **Diagnostic :** l’échec Windows initial provenait du certificat/configuration
  de test GreenMail par défaut (`GREENMAIL_TEST_CERTIFICATE`), pas d’une
  incompatibilité rustls Windows. STARTTLS reste non testé.
- Détails : `experiments/2026-08-22-mail-archive-imap-probe.md`.
- **Question future :** RFC 8474 `OBJECTID` pourrait fournir `MAILBOXID`,
  `EMAILID` et éventuellement `THREADID` pour compléter le modèle ; cette
  extension reste optionnelle et n’est pas requise pour IMAP de base.

## 2026-08-22 — Premier import IMAP readonly

- **Fait vérifié :** un CLI IMAPS dédié utilise `EXAMINE` et `BODY.PEEK[]`,
  écrit les RAW via `ArchiveWriter`, conserve une table de provenance IMAP
  (`imap_messages`) non reconstructible depuis le RAW seul, puis réutilise le
  pipeline Tantivy existant. Le RAW MIME reste l'autorité ; parsing, catalogue
  de recherche et index restent dérivables/reconstructibles.
- **Fait vérifié :** GreenMail 2.1.12 a produit 12 nouveaux messages Linux et
  Windows, puis 0 nouveau message lors d’un import identique ; les RAW et une
  exportation EML sont byte-exacts.
- **Décision :** l’identité de cette première passe est
  source+mailbox+UIDVALIDITY+UID ; un changement de UIDVALIDITY est signalé,
  sans rapprochement automatique. Cette identité est protégée par une
  `PRIMARY KEY` SQLite, et Tokio reste isolé derrière `sync_imap`.
- **Atomicité :** `append_raw` précède l'insertion du mapping IMAP ; une panne
  peut laisser un RAW orphelin mais ne doit pas créer de référence catalogue
  vers une frame inexistante. Une transaction durable RAW+SQLite reste une
  question distincte.
- **Fait vérifié :** la synchronisation incrémentale minimale conserve une
  frontière `scanned_through_uid` dans `imap_scan_state`, par source/mailbox/
  UIDVALIDITY. Après un `EXAMINE` avec UIDNEXT stable, seuls les UID jusqu'au
  snapshot `UIDNEXT-1` sont demandés, ou jusqu'à la borne de la tranche
  `--limit`. Une tranche limitée réussie avance la frontière jusqu'à sa borne,
  tandis qu'une campagne interrompue ne l'avance pas. Les UID absents ne sont
  pas utilisés pour calculer la frontière. Le parcours FETCH traite les
  messages un par un.
- Détails : `experiments/2026-08-22-mail-archive-imap-import.md`.

## 2026-08-24 — Probe synthétique MDN/DSN

- **Fait vérifié :** `mailparse 0.16.1` expose l’arbre MIME
  `multipart/report` et ses sous-parties, mais ne fournit pas de modèle
  sémantique MDN/DSN : les champs et groupes destinataires doivent être
  interprétés par une couche dédiée au-dessus de `ParsedMail`.
- **Décision expérimentale :** conserver un corpus synthétique de 44 fixtures
  (15 MDN, 14 DSN, 11 négatifs, 4 RFC 6533 prospectives) ; les oracles sont
  indépendants du modèle mailparse mais générés depuis les mêmes spécifications
  déterministes que les `.eml`. Les tests de mutation vérifient leur pouvoir
  discriminant. Les messages textuellement similaires sans structure MIME
  restent ordinaires.
- **Fait vérifié :** les champs per-message/per-destinataire RFC 3464 sont
  séparés ; `Will-Retry-Until` n’est présent que pour un destinataire `delayed`.
  `Original-Message-ID` dépend du `Message-ID` original, pas de la présence
  d’une troisième partie.
- Détails : `experiments/2026-08-24-mdn-dsn-corpus.md`.

## 2026-08-25 — Parser produit MDN/DSN initial

- **Fait vérifié :** `delivery_report::analyze_delivery_report` analyse les
  MDN/DSN classiques démontrés par les 44 golden fixtures, conserve séparés
  les champs DSN per-message/per-destinataire, et classe les quatre types
  RFC 6533 comme `Unsupported`.
- **Décision :** cette API reste une analyse isolée du RAW ; elle ne corrèle
  pas de message, ne modifie ni catalogue ni index, et ne traite pas
  l’authenticité DKIM/SPF.
- **Fait vérifié :** les statuts DSN valident la syntaxe extensible
  `class.subject.detail` sans imposer un catalogue IANA ; les MIME complets
  non parsables sont `Unparseable` plutôt que devinés MDN/DSN. Le corpus compte
  désormais 44 fixtures.

## 2026-08-25 — HTML remote-resource evidence probe

- **Fait vérifié :** l’analyse des ressources distantes doit partir du HTML
  MIME original, avant réécriture CID et `ammonia`; `ammonia` retire le contenu
  actif mais peut conserver une URL d’image distante, tandis que la CSP
  actuelle bloque son chargement automatique.
- **Décision expérimentale :** séparer les observations déterministes (URL,
  élément, dimensions, visibilité inline, query) des signaux heuristiques ; ne
  jamais produire une conclusion `is_tracker` dans cette couche.
- Détails : `experiments/2026-08-25-html-remote-evidence-corpus.md`.
- **Fait vérifié :** Memoria expose maintenant cette analyse offline via
  `html_remote_evidence::analyze_html_remote_evidence`; les 30 fixtures sont
  réutilisées directement par les tests produit. Le résultat ne contient
  aucun verdict de tracking.
- **Correction vérifiée :** le `TreeSink` possède ses `QualName` dans des
  nœuds `Rc<RefCell<_>>` ; aucune fuite volontaire `Box::leak` ne subsiste.
## IMAP multi-mailbox

- `LIST` expose les noms protocolaire, delimiter, attributs et SPECIAL-USE sans
  supposer de séparateur ou de nom d’affichage; ces métadonnées sont conservées
  dans `imap_mailboxes` comme provenance serveur. Une campagne GreenMail a
  confirmé un nom modified UTF-7 (`Caf&AOk-`) réutilisable par EXAMINE, mais pas
  sa présentation Unicode.
- `imap_scan_state` reste indépendant pour chaque source/mailbox/UIDVALIDITY.
  Les occurrences d’un même RAW dans plusieurs mailboxes ne sont pas fusionnées;
  l’identité logique reste mailbox + UIDVALIDITY + UID.
- Le CLI accepte plusieurs `--mailbox` et `--all-mailboxes`; GreenMail 2.1.12
  a validé l’import indépendant, la relance sans refetch et l’ajout limité à
  une mailbox. Le même scénario a été rejoué nativement sous Windows avec
  delimiter `.`, `Caf&AOk-` réutilisable tel quel, `/` ordinaire et sans
  `\\Seen`; SPECIAL-USE n’était pas annoncé par ce serveur.
- **Fait vérifié :** le replay Windows multi-mailbox a importé 20 occurrences,
  puis 0 refetch au second passage; après un ajout dans `Projects.Alpha`, seul
  ce mailbox a fetché un RAW. L’archive contenait 12 RAW distincts et 8
  occurrences byte-identiques supplémentaires, sans fusion heuristique.
- **Fait vérifié :** le replay Windows incrémental a validé
  `0→12→15`, puis `0` fetch inchangé, ainsi que les tranches `--limit 5`
  `0→5→10→15` et une répétition à zéro. `UIDVALIDITY` est restée stable et
  les FLAGS sont restées sans `\\Seen`.
- La validation native `cargo check --workspace` et `cargo test --workspace`
  a passé sur le clone au commit `0f348371515d1e57b241894e2a767019541750e0`.
