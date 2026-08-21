# Journal de travail

Journal léger des découvertes provisoires, hypothèses, expériences et
questions ouvertes. Les conclusions réutilisables doivent être promues dans
`KNOWLEDGE.md`, avec un pointeur vers le détail conservé dans `experiments/`.

## 2026-08-20 — Initialisation

- **Décision de projet :** commencer avec un workspace Cargo virtuel vide.
- **Fait vérifié :** le dépôt était vide avant cette initialisation.
- **Hypothèses / questions ouvertes :** choix de la version Rust, de la
  version Slint et des stratégies de packaging à déterminer avec le premier
  projet réel.
- **Résultat :** aucune application et aucune abstraction commune créées. Un
  paquet racine vide sert d'ancre compilable, car Cargo refuse un workspace
  virtuel sans membre lors de `cargo check`.

## 2026-08-20 — Dépendance fontconfig Linux

- **Fait vérifié :** le build de Slint échoue si `pkg-config` ne trouve pas
  `fontconfig.pc`.
- **Décision de projet :** forcer `RUST_FONTCONFIG_DLOPEN=1` dans
  `.cargo/config.toml`, afin de ne pas dépendre du chemin Homebrew par défaut.
- **Précision vérifiée :** Slint 1.17.1 n'expose pas directement cette feature
  de `fontique`; une dépendance directe `fontique` permet à Cargo d'unifier la
  feature `fontconfig-dlopen` avec la dépendance transitive de Slint.
- **Limitation ouverte :** le fonctionnement du chargement dynamique et la
  présence de fontconfig sur les distributions Linux cibles restent à
  vérifier sur leurs machines respectives.

## 2026-08-20 — Démonstration native

- **Décision de projet :** la première application reste un exemple isolé dans
  `src/main.rs` et `ui/app.slint`; aucune abstraction métier ou bibliothèque
  commune n'a été introduite.
- **Faits vérifiés :** `cargo test --all-targets`, `cargo check`,
  `cargo build --release` et `cargo fmt --all` passent. Le filtre vérifie le
  dataset de 100 000 éléments par tests Rust.
- **Mesure :** voir `experiments/2026-08-20-slint-demo.md` pour le binaire,
  RSS et temps de préparation reproductibles.
- **Questions ouvertes :** validation interactive Windows/Linux (clavier,
  HiDPI, Wayland, lecteur d'écran) et packaging restent à exécuter sur des
  machines cibles.

## 2026-08-20 — Mail archive : changement d'échelle

- **Fait vérifié :** les indexeurs SQLite FTS5 et Tantivy lisent maintenant les
  frames par localisation depuis l'archive et le catalogue, puis parsèment
  les messages ; ils ne régénèrent plus les messages depuis `(seed, id)`.
- **Fait vérifié :** 100 000 messages terminent en 14,8 s avec 114 MiB de RSS
  maximale dans la configuration instrumentée ; 1 000 000 terminent en 76,2 s
  avec 239 MiB environ. Les tailles et détails sont dans le rapport dédié.
- **Fait vérifié :** Tantivy est plus rapide sur toutes les familles mesurées,
  surtout les requêtes très fréquentes, tandis que FTS5 reste une baseline
  conservatrice et simple. Les tailles comprennent des choix de restitution
  différents et sont ventilées dans le rapport.
- **Fait vérifié :** gzip bat légèrement zstd niveau 3 sur le corpus textuel
  et sur les pièces jointes pseudo-aléatoires ; aucune compression définitive
  n'est choisie.
- **Hypothèse :** 64 MiB est un bon point de départ pour les segments, mais
  l'expérience n'a pas encore mesuré une sauvegarde incrémentale réelle.
- **Décision de projet :** ne pas promouvoir Tantivy, l'index chaud ou le CAS
  au rang d'architecture définitive avant l'expérience suivante sur le CAS et
  la maintenance/reconstruction.

## 2026-08-20 — Mail archive : profils de corpus

- **Fait vérifié :** l'ancien pool implicite de payloads produisait une
  duplication artificiellement élevée. Le générateur a été remplacé par des
  profils `light`, `personal` et `heavy`, avec catégories de pièces jointes,
  queue asymétrique de tailles et `duplicate-rate` explicite.
- **Fait vérifié :** le profil light est majoritairement textuel ; personal
  représente une boîte hétérogène dominée par les octets de pièces jointes ;
  heavy produit 5,006 Go pour 5 000 messages. Les percentiles et les mesures
  détaillées sont dans le rapport des profils.
- **Fait vérifié :** l'économie théorique de déduplication est de 8,1 % light,
  12,8 % personal et 8,7 % heavy dans les échantillons mesurés. Le nombre
  d'objets dupliqués et les octets économisables diffèrent fortement.
- **Fait vérifié :** la compression séparée montre que le texte est très
  compressible, tandis que les blobs aléatoires peuvent grossir ; aucun codec
  n'est adopté dans l'archive.
- **Hypothèse :** le CAS mérite désormais une expérience limitée à personal
  et heavy, avec inline comme contrôle ; light ne doit pas guider la décision.
- **Incident expérimental :** une première tentative personal à plafond trop
  élevé a consommé plusieurs Go avant son manifeste. Les plafonds par profil
  ont été réduits et la compression rendue optionnelle (`--compression`).

## 2026-08-20 — Mail archive : CAS et fidélité

- **Fait vérifié :** un CAS placé à côté du MIME brut ne peut pas économiser
  les bytes MIME ; l'expérience externalise donc les payloads dans un magasin
  transformé avec références et manifeste.
- **Fait vérifié :** Blake3, blobs segmentés, manifeste TSV et magasin de
  messages permettent une reconstruction byte-exacte des corpus/fixtures
  testés. Sept tests couvrent déterminisme, ranges, reconstruction, fixture
  MIME et blob orphelin.
- **Fait vérifié :** personal/10k économise 12,2 % en CAS exact, 9,5 % en
  hybride 64 KiB ; heavy/1k économise environ 0,94 %, mais cet échantillon est
  plus variable. Le CAS décodé est identique au CAS exact car les payloads
  synthétiques sont déjà décodés.
- **Fait vérifié :** une campagne heavy/5k avec cinq variantes a rempli le
  volume temporaire après environ 12 Go de résultats partiels. Les variantes
  doivent être mesurées séquentiellement ou sur un volume adapté.
- **Décision de projet :** conserver byte-exact comme contrat de migration,
  ne pas adopter le CAS comme unique autorité et mesurer ensuite une vraie
  séquence incrémentale de sauvegarde en deux lots.

## 2026-08-20 — Mail archive : connecteur Gmail read-only

- **Fait vérifié :** le prototype compile avec `reqwest`, OAuth desktop loopback,
  `mailparse`, décodage RAW base64url et réutilisation de l’archive segmentée.
- **Fait vérifié :** les fixtures couvrent pagination, deux imports, seconde
  synchronisation sans nouvelle frame et repli après expiration de l’historique.
- **Décision de projet :** l’identité d’idempotence est le couple clé locale de
  compte + Gmail message ID ; le contenu/hash reste une identité distincte.
- **Décision de projet :** les suppressions et absences Gmail deviennent un
  état `deleted` dans le catalogue ; aucun byte d’archive n’est supprimé.
- **Hypothèse :** la full sync de réconciliation, qui relit les RAW connus pour
  rafraîchir labels et history, sera acceptable seulement après mesure réelle.
- **Limitation :** aucun credential ni compte Gmail n’a été utilisé ici ; les
  distributions réelles et résultats CAS réels restent inconnus.
- **Prochaine expérience :** campagne autorisée et agrégée 100 → 1 000 messages,
  puis comparaison initial / sans changement / incrémentale.

## 2026-08-20 — Mail archive : campagne Gmail réelle 100 → 1 000

- **Fait vérifié :** 1 000 messages et 1 000 IDs distincts sont conservés dans
  une archive de 57 633 178 octets, avec 1 000 checksums valides et un seul
  segment de moins de 64 MiB.
- **Fait vérifié :** la relance finale n’ajoute aucune frame ; le checkpoint
  incrémental sans changement examine zéro élément et n’ajoute aucun byte.
- **Fait vérifié :** p50/p90/p99/max RAW = 54 803 / 101 889 / 178 363 /
  267 410 octets.
- **Fait vérifié :** l’analyse hors ligne trouve 938 messages multipart,
  2 907 parties et 1 969 feuilles ; aucune feuille ne porte attachment,
  filename/name ou Content-ID. Le CAS théorique reste donc nul sur cette
  tranche, sans conclusion sur la boîte complète.
- **Décision de projet :** la synchronisation complète contrôlée peut être
  lancée en RAW-inline ; aucune décision CAS ou changement de format n’est
  promu par cette campagne.

## 2026-08-20 — Mail archive : MIME Gmail avec pièces jointes

- **Fait vérifié :** `has:attachment` a retourné 37 messages et
  `has:attachment larger:1M` en a retourné 7 dans deux archives séparées.
- **Fait vérifié :** Gmail/MIME ne sont pas des catégories identiques : 102
  candidats nommés contre 25 dispositions `attachment` strictes dans le
  premier échantillon ; l’écart est expliqué par inline/Content-ID.
- **Fait vérifié :** les payloads lourds sont bien lus et décodés ; 8 pièces
  jointes distinctes dans l’échantillon >1M, sans duplication observée.
- **Décision de projet :** conserver les deux agrégats `attachment` strict et
  `attachment_candidate`, sans modifier le format RAW ni activer de CAS.
- **Conclusion :** le pipeline MIME est suffisamment propre pour une full sync
  contrôlée ; les résultats CAS de ces petits échantillons restent nuls.

## 2026-08-20 — Mail archive : full sync Gmail complète

- **Fait vérifié :** l’énumération complète a examiné 3 012 messages, reconnu
  les 1 000 déjà présents et ajouté 2 012 messages, sans erreur comptée.
- **Fait vérifié :** l’archive finale fait 211 839 978 octets sur 4 segments ;
  3 012/3 012 checksums et localisations catalogue sont valides.
- **Fait vérifié :** RAW p50/p90/p99/max = 47 899 / 87 665 / 145 799 /
  16 865 580 octets ; moyenne 70 300 octets.
- **Fait vérifié :** `complete=true` est enregistré et la relance history sans
  changement retourne zéro nouveau message et zéro byte ajouté.
- **Fait vérifié :** les 102 candidats MIME représentent environ 20 % des
  bytes RAW, sans duplication de contenu observée ; CAS théorique nul.
- **Décision de projet :** ne pas modifier le format ni activer un CAS ; la
  prochaine incertitude est l’échelle multi-comptes/catalogue/index dérivé.

## 2026-08-21 — Audit duplication et réconciliation Gmail

- **Fait vérifié :** l’ancien calcul additionnait toutes les occurrences dans
  `unique_bytes`; il comptait les hashes mais ne calculait pas les octets
  uniques. Le test `A,A,B,C,C` fixe le calcul attendu : 10 totaux, 6 uniques,
  4 dupliqués.
- **Fait vérifié :** sur 3 012 RAW, la duplication candidate est de 1,43 %
  encodée et 1,50 % décodée ; le sous-ensemble >64 KiB est à 0,72 %.
- **Fait vérifié :** Gmail `format=METADATA` suffit pour rafraîchir les
  métadonnées d’un message connu ; une fixture 1 000 connus + 10 nouveaux ne
  récupère en RAW que les 10 inconnus.
- **Décision de projet :** conserver RAW comme autorité, éviter le RAW
  redondant pendant la réconciliation et garder le CAS facultatif.

## 2026-08-21 — Mail archive : Tantivy réel

- **Fait vérifié :** l’indexeur Gmail lit exclusivement les segments et le
  catalogue, parse avec `mailparse 0.16.1` et indexe 3 012 documents Tantivy
  sans échec MIME. L’index dérivé fait environ 11,1 MB.
- **Fait vérifié :** une construction propre prend environ 15,5 s et la RSS
  maximale mesurée avec Cargo est de 94 984 KiB. L’ouverture de l’index prend
  2,4 ms et la première requête 1,6 ms ; les requêtes lexicales chaudes ont
  généralement un p95 inférieur à 2 ms.
- **Fait vérifié :** les plages de dates sont plus lentes, autour de 11–15 ms
  dans cette baseline ; aucune optimisation séparée n’est lancée.
- **Fait vérifié :** la seconde indexation examine 3 012 lignes mais en saute
  3 012, sans lecture RAW ni parsing. Une suppression de l’index dérivé sur
  copie, suivie d’une reconstruction, réindexe les 3 012 messages et permet
  de rechercher à nouveau.
- **Fait vérifié :** le parseur HTML retire balises, scripts et styles dans le
  texte dérivé ; il ne promet ni rendu navigateur ni suppression intelligente
  des citations/signatures.
- **Décision de projet :** conserver BM25 comme baseline et exposer une API
  Rust indépendante de Slint/CLI (`GmailSearchIndex` et lecture RAW par
  `doc_id`). La prochaine étape est une UI Slint minimale de recherche.

## 2026-08-21 — Mail archive : première UI produit

- **Fait vérifié :** `mail-archive-app` ouvre l’archive locale hors ligne,
  affiche jusqu’à 50 résultats Tantivy et lit le RAW sélectionné dans un
  thread de fond avant d’en afficher le texte MIME dérivé.
- **Mesure :** benchmark contrôleur réel : ouverture index 4,2 ms, recherche
  2,2 ms, lecture/parsing 2,8 ms ; fenêtre détectée sous Xvfb en 194 ms.
- **Fait vérifié :** la sélection souris et le parcours flèches + Entrée ont
  été exercés sous Xvfb ; le redimensionnement 800×600 → 1500×900 ne plante
  pas. Les contenus réels restent hors des artefacts versionnés.
- **Limite :** HiDPI, lecteur d’écran et rendu Wayland physique ne sont pas
  validés dans cet environnement ; la petite largeur reste dense.
- **Décision de projet :** conserver le rendu texte sans WebView et ne pas
  optimiser l’infrastructure avant une courte validation UX sur machines
  Windows/Linux réelles.

## 2026-08-21 — Mail archive : validation Wayland de la première UI

- **Fait vérifié :** une session KDE/Wayland réelle est accessible ;
  `wayland-info` expose le compositeur et `mail-archive-app` démarre
  directement avec le backend Wayland natif, sans Xvfb.
- **Fait vérifié :** le rendu desktop observé est net, le focus initial est
  visible, les deux panneaux restent lisibles et la fermeture est propre.
- **Limite :** les outils d’injection clavier/inspection AT-SPI et un second
  facteur HiDPI ne sont pas disponibles dans cette session ; ces points ne
  sont pas déclarés validés sur Wayland.
- **Décision de projet :** aucune correction UI supplémentaire ; la première
  version est suffisante pour une utilisation desktop de recherche/lecture.
- **Détails reproductibles :** `experiments/2026-08-21-mail-archive-first-ui-wayland.md`.

## 2026-08-21 — Mail archive : premier retour manuel et corrections UI

- **Fait vérifié :** les lignes de résultats étaient réellement rendues sans
  largeur de délégué ; l’état initial était ambigu et deux textes d’état
  étaient mal alignés verticalement.
- **Correction :** contraintes de largeur explicites pour `ListView`,
  alignement vertical des textes, fenêtre avec taille préférée et minimum,
  message d’état initial explicite, effacement cohérent de la sélection et
  `ScrollView` du corps avec largeur de reflow.
- **Fait vérifié :** après correction, une recherche réelle affiche 50 lignes
  lisibles, la sélection clavier ouvre un message et 800×600 reste exploitable.
- **Décision :** `Memoria — Archive` est le nom provisoire ; aucun menu
  contextuel ni WebView n'est ajouté sans action produit justifiant leur
  coût.
- **Détails :** `experiments/2026-08-21-mail-archive-first-ui-manual-fixes.md`.

## 2026-08-21 — Mail archive : clavier et AT-SPI

- **Fait vérifié :** après activation de l'AT-SPI de session, AccessKit expose
  la fenêtre Wayland, la recherche, l'effacement, la liste, la région de
  lecture et les trois boutons de zoom avec des rôles cohérents.
- **Correction :** ajout de Ctrl+F, retour Échap vers la liste, navigation
  Home/End/PageUp/PageDown, zoom Ctrl+plus/Ctrl−/Ctrl+0 et labels accessibles.
- **Fait vérifié :** le scénario clavier complet passe sous Xvfb ; une
  incohérence Ctrl+Retour arrière qui conservait le texte de recherche a été
  corrigée.
- **Limite :** `ydotoold` ne peut pas ouvrir son périphérique uinput avec les
  permissions actuelles ; la frappe directe Wayland n'est pas déclarée
  validée. Détails : `experiments/2026-08-21-mail-archive-keyboard-atspi.md`.

## 2026-08-21 — Memoria : corrections ciblées après première utilisation

- **Fait vérifié :** le corps de lecture peut utiliser le `TextEdit` read-only
  de Slint pour obtenir scroll, sélection et copie sans WebView.
- **Correction :** hauteur explicite du champ de recherche, autoscroll de la
  sélection clavier, séparateurs de lignes et fallback `[lien externe]` pour
  les URL HTML dérivées.
- **Décision :** ajout d'une barre de menus traditionnelle comme ossature
  desktop ; l'espace Archive/Synchronisation reste préparé mais non branché.
- **Limite :** aucune icône portable directe n'est disponible dans l'API
  actuelle sans infrastructure spécifique.
- **Détails :** `experiments/2026-08-21-mail-archive-first-ui-desktop-structure.md`.

## 2026-08-21 — Memoria : boucle Archive → Synchronisation → Index

- **Fait vérifié :** `sync_account_with_progress` réutilise le moteur Gmail du
  CLI et fournit des snapshots agrégés au worker UI.
- **Correction :** ajout de la vue Archive/Synchronisation, du garde-fou contre
  deux sync simultanées, du lancement worker et du rechargement Tantivy après
  commit.
- **Fait vérifié :** une campagne réelle depuis l’UI est passée de 3 012 à
  3 013 messages, puis une seconde sync a affiché zéro nouveau message ; les
  RAW et l’index sont restés cohérents.
- **Limite :** pas d’annulation coopérative et saisie automatisée post-sync non
  fiable dans cette session Xvfb ; les tests clavier précédents restent la
  référence pour la navigation.
- **Détails :** `experiments/2026-08-21-mail-archive-sync-ui.md`.

## 2026-08-21 — Memoria : build Windows MSVC

- **Fait vérifié :** `cargo-xwin 0.23.1` produit
  `mail-archive-app.exe` en `x86_64-pc-windows-msvc`; les 18 tests de
  bibliothèque MSVC passent via Wine.
- **Correction :** le binaire release utilise le subsystem GUI Windows et ne
  crée plus de console parasite ; le debug conserve la console de diagnostic.
- **Limite :** Wine échoue dans la découverte de polices Slint avec son
  environnement graphique incomplet ; aucune conclusion UX Windows native
  n’en est tirée.
- **Détails :** `experiments/2026-08-21-mail-archive-windows-port.md`.

## 2026-08-21 — Memoria : CRT statique et artifact CI Windows

- **Fait vérifié :** le workflow Windows construit désormais le binaire release
  et publie l’artifact `memoria-windows-x86_64`.
- **Fait vérifié :** la variante `+crt-static` compile et ses 18 tests MSVC
  passent via Wine ; `VCRUNTIME140.dll` et Universal CRT disparaissent des
  imports, pour environ 170 KiB supplémentaires.
- **Limite :** aucun run GitHub Actions ni test Windows natif n’a encore été
  exécuté depuis cet environnement local.
- **Détails :** `experiments/2026-08-21-mail-archive-windows-port.md`.

## 2026-08-21 — Memoria : audit ciblé des dépendances

- **Fait vérifié :** Slint est déjà en features explicites sans femtovg ni
  system-tray ; Reqwest utilise uniquement Rustls ; rfd conserve Wayland et
  xdg-portal pour les dialogues Linux.
- **Expérience :** retirer stemmer/stopwords de Tantivy économise environ
  346 KiB mais ne change pratiquement pas l’index ni les latences ; les
  defaults sont conservés.
- **Limite :** `cargo deny` n’est pas installé ; aucun audit advisories/licences
  n’a été exécuté.
- **Détails :** `experiments/2026-08-21-mail-archive-dependency-audit.md`.

## 2026-08-21 — Memoria : pièces jointes et progression sync

- **Fait vérifié :** l'ouverture d'une pièce jointe passe maintenant par
  l'association desktop (`open::that_detached`) ; le test KDE a lancé
  Gwenview pour une image locale.
- **Fait vérifié :** les noms réservés Windows avec extension, `COM1..9`,
  `LPT1..9`, séparateurs et suffixes espace/point sont couverts par des tests.
- **Fait vérifié :** une full sync émet désormais `examined/total` après une
  énumération paginée unique ; history reste indéterminé sans second parcours.
- **Détails :** `experiments/2026-08-21-mail-archive-attachments-ui.md` et
  `experiments/2026-08-21-mail-archive-sync-progress.md`.

## 2026-08-21 — Mail archive : recherche structurée

- **Fait vérifié :** `SearchRequest` combine texte, expéditeur, destinataire,
  dates, présence/MIME de pièce jointe et labels directement dans Tantivy,
  avant la limite de résultats. Les fixtures couvrent aussi une ressource
  inline/CID exclue des pièces jointes utilisateur.
- **Fait vérifié :** l’ancien schéma Tantivy est reconstruit depuis RAW +
  catalogue ; le format RAW et le catalogue ne changent pas. L’index Gmail
  réel reconstruit fait 11 225 345 octets.
- **Limite rencontrée :** la tentative de campagne synthétique 1M a saturé le
  tmpfs ; la mesure 100k a réussi mais ne contient pas une distribution Gmail
  exploitable pour labels/MIME.
- **Détails :** `experiments/2026-08-21-mail-archive-advanced-search.md`.

## 2026-08-21 — Campagne recherche structurée 1M

- **Fait vérifié :** un probe dédié génère 1M de messages MIME avec labels,
  dates pondérées, MIME, pièces jointes et correspondants déterministes, puis
  mesure le pipeline réel `archive → catalogue → mailparse → Tantivy`.
- **Incident corrigé :** une adresse complète de correspondant ne répondait
  pas dans le premier probe ; le workload utilisait un identifiant différent
  de celui généré. Le filtre adresse est maintenant couvert par fixture et
  smoke test.
- **Fait vérifié :** 1M atteint 136,6 MB d’index et environ 1,2 GiB RSS de
  pointe ; les requêtes combinées restent sous 12,2 ms au p95 dans ce corpus.
- **Décision :** aucune modification d’architecture Tantivy/SQLite ; le RSS
  est la prochaine incertitude prioritaire.
- **Détails :** `experiments/2026-08-21-mail-archive-structured-search-1m.md`.

## 2026-08-21 — Mémoire de reconstruction Tantivy 1M

- **Fait vérifié :** la chronologie instrumentée a isolé la collecte complète
  de `gmail_catalog_rows` (~259 MiB à 1M) et le vecteur `state_upserts` comme
  deux matérialisations inutiles.
- **Correction validée :** le catalogue est parcouru ligne par ligne et les
  mises à jour de l'état Tantivy sont exécutées dans une transaction bornée.
  Le schéma RAW/catalogue et les résultats de recherche restent inchangés.
- **Mesure :** le pic passe d'environ 1 255 920 KiB à 816 236 KiB à 1M ;
  100k/300k/500k corrigés donnent respectivement 160 844/379 092/667 992
  KiB. Tantivy reste le principal poste mémoire pendant `add_document` et la
  fusion.
- **Détails :** `experiments/2026-08-21-mail-archive-index-memory-1m.md`.

## 2026-08-21 — Réglage IndexWriter Tantivy 1M

- **Fait vérifié :** Tantivy 0.26.1 utilise actuellement 50 000 000 octets,
  3 workers effectifs sur cette machine et 4 threads de merge par défaut.
- **Mesure :** 64 MiB et 1 worker augmentent fortement le RSS ; le minimum
  valide à 3 workers ralentit l'indexation ; 1 merger est pratiquement neutre
  ; `NoMergePolicy` est défavorable avec 157 segments.
- **Décision :** conserver le réglage produit dynamique actuel ; aucun
  changement de budget/concurrence n'est justifié.
- **Détails :** `experiments/2026-08-21-mail-archive-tantivy-writer-tuning.md`.

## 2026-08-21 — Probe miniature système

- **Action :** création d'un probe isolé Rust Linux/Windows, sans modification
  de Memoria, puis inventaire et mesure des providers KDE réellement installés.
- **Résultat :** images, SVG/TGA, vidéo, MP3 avec pochette et ODS réussissent ;
  PDF est indisponible et certains providers audio/Office échouent proprement.
  Le cache freedesktop est reconnu au second accès.
- **À retenir :** l'appel Linux est déjà isolé par provider enfant avec timeout ;
  un helper supplémentaire reste une option surtout à confirmer pour Windows.
- **Rapport :** `experiments/2026-08-21-system-thumbnail-service.md`.

## 2026-08-21 — Audit IPC KIO

- **Fait vérifié :** `KIO::PreviewJob` ne fournit pas de service D-Bus public
  pour les miniatures ; il lance `kioworker` et utilise des sockets Unix
  privés. Le worker lance ensuite les ThumbnailCreator/providers.
- **Mesure :** le helper KF6 release fait 122 560 octets, dépend d'environ
  91 bibliothèques ELF transitives et consomme environ 61 MiB RSS sur le PDF.
- **Décision :** ne pas créer de client Rust du protocole KIO ; conserver un
  helper Qt/KF6 séparé comme frontière d'adaptation, avec timeout et option de
  désactivation.
- **Rapport :** `experiments/2026-08-21-system-thumbnail-service.md`.

## 2026-08-21 — Correction du probe PDF KDE/KIO

- **Erreur corrigée :** le premier probe avait conclu `PDF unavailable` après
  avoir sondé uniquement les fichiers freedesktop `.thumbnailer`.
- **Fait vérifié :** `kdegraphics-thumbnailers` installe
  `gsthumbnail.so`, qui annonce `application/pdf`; `KIO::PreviewJob` produit
  une miniature du même fixture PDF et Dolphin remplit son cache.
- **Isolation observée :** `KIO::PreviewJob` lance `/usr/lib/kf6/kioworker`
  hors processus, puis le creator PDF lance Ghostscript hors processus.
- **Décision provisoire :** ordre KDE = KIO puis freedesktop ; `unavailable`
  seulement après épuisement des deux backends.
- **Rapport :** `experiments/2026-08-21-system-thumbnail-service.md`.
## 2026-08-21 — Intégration preview pièces jointes

- Raccordé l'overlay Slint au helper KIO/freedesktop existant, sans Qt/KF6
  dans Memoria et sans modification du stockage.
- Ajouté timeout, désactivation explicite, fallback provider et fermeture par
  Escape/clic extérieur/bouton.
- Vérifié : `cargo test --workspace`, `cargo check --workspace`, build release
  Memoria et helper KIO. Une validation interactive image/PDF sur archive réelle
  reste une étape manuelle, sans contenu à consigner.
- Validation KDE Wayland effectuée via AT-SPI : un PDF réel a été affiché dans
  l'overlay après passage KIO ; un défaut de clic extérieur a été corrigé par
  des zones de hit-testing explicites. La résolution du helper a aussi été
  durcie : nom d'environnement, voisin de l'exécutable, puis PATH, sans faux
  positif sur un nom absent.

## 2026-08-21 — HTML dans le navigateur système

- Ajouté un serveur localhost éphémère avec sessions opaques, CSP stricte et
  routes CID en mémoire ; aucun WebView ni moteur HTML n'est lié à Memoria.
- `ammonia` nettoie le document avant ouverture explicite dans le navigateur.
- Fixtures HTML/CID/sécurité et smoke tests sur HTML réel, dont un message avec
  ressource embarquée, passés sans journaliser le contenu.

## 2026-08-21 — Passe i18n FR/EN et identifiants

- Ajouté le catalogue applicatif `src/i18n.rs`, détection FR/EN et pluriels
  principaux ; aucune dépendance Cargo supplémentaire.
- Migré le chrome Slint, menus, filtres et libellés de pièces jointes vers les
  textes localisés ; les valeurs MIME/Gmail restent stables.
- Borné les sessions HTML à 8 entrées/10 minutes et renforcé le test CSP.
- Validations : `cargo test -p mail-archive-experiment`, `cargo check --workspace`.
- Rapport : `experiments/2026-08-21-mail-archive-i18n-identifiers.md`.

## 2026-08-21 — Correction des images CID HTML

- Diagnostic hors ligne anonymisé sur l'archive réelle : 78 références
  `cid:` correspondantes ; après correction, 78 réécritures, 78 réponses
  HTTP 200 et 78 réponses `image/*`.
- Corrigé la normalisation exacte des CID (angles et percent-encoding) avant
  ammonia ; les images HTTP/HTTPS restent bloquées par la CSP.
- Ajouté les fixtures CID simples, avec `@`, percent-encodées, absentes,
  multiples, non-image et image externe ; le smoke test navigateur système
  a été relancé sans journaliser de contenu personnel.
- Rapport : `experiments/2026-08-21-mail-archive-html-browser.md`.

## 2026-08-21 — Audit dépendances, binaire et sécurité

- Capturés `cargo tree`, `cargo tree -d`, `cargo tree -e features`,
  `cargo bloat`, `ldd` et les métadonnées Cargo du paquet Memoria.
- `cargo-audit 0.22.2` installé hors dépôt puis exécuté : aucune vulnérabilité
  connue ; warnings de maintenance et alerte `lru 0.16.4` documentés.
- Tentative de mise à jour directe vers `lru 0.18.2` refusée par la contrainte
  `tantivy 0.26.1` (`lru ^0.16.3`) ; aucun changement de dépendance conservé.
- Binaire release restauré après l’analyse bloat : 31 070 752 octets.
- Rapport : `experiments/2026-08-21-mail-archive-dependency-security-audit.md`.

## 2026-08-21 — Profil CI ThinLTO

- Ajouté `[profile.ci]`, hérité de `release` avec ThinLTO et 8 unités de
  codegen ; le profil de distribution fat LTO reste inchangé.
- Le workflow Windows utilise `ci` pour les builds ordinaires push/PR et garde
  `release` dans un job `workflow_dispatch` explicite, pour les variantes CRT
  dynamique et statique.
- Validation locale : formatage, `cargo check --workspace`,
  `cargo test --workspace` et build `cargo build --profile ci` passés.
- Mesure locale : binaire CI Linux de 36 609 152 octets contre 31 070 752
  octets pour le release historique ; aucun run GitHub n'a encore été exécuté.
