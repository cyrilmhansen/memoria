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

Pointeurs officiels à consulter au besoin, sans les considérer comme des
résultats d'expérience locale :

- [Slint documentation](https://docs.slint.dev/)
- [The Rust Book](https://doc.rust-lang.org/book/)
- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
