# Journal de travail

Journal léger des découvertes provisoires, hypothèses, expériences et
questions ouvertes. Les conclusions réutilisables doivent être promues dans
`KNOWLEDGE.md`, avec un pointeur vers le détail conservé dans `experiments/`.

## 2026-08-29 — Recovery R2.1a Gmail exact

- La première implémentation ouvrait le writer normal et pouvait recréer un
  segment manquant avant la preuve distante.
- La séparation `authority-only` / writer a supprimé cette possibilité ; la
  publication utilise ensuite une destination fresh `create_new`.
- La réconciliation combine `inventory_records` et `inventory_physical`.
- Le profile OAuth est vérifié contre `source_account`, avec les helpers uniques
  `gmail_source_account` et `gmail_message_identity`.
- Le conflit CAS est démontré explicitement : `RecoveryConflict` laisse une
  frame `OrphanValidated` sûre, sans modifier l’ancien record.
- Les tests fail-closed snapshotent les segments, SQLite et le contenu des
  sidecars. Le GUI `--account` est une simple assertion sur le profile
  authentifié, jamais une identité persistée.
- Sol High a fermé R2.1a. Commit final :
  `f8c61a1 feat(mail-archive): recover missing gmail raw exactly`.

## 2026-08-29 — R2.1a recovery Gmail exact

- **Fait vérifié :** le checkout local était sur `mail-archive` à `2693e40`;
  `git pull --ff-only` n'a pas pu mettre à jour `.git/FETCH_HEAD`, refusé en
  lecture seule par l'environnement. Les répertoires historiques `target/`
  non suivis n'ont pas été touchés.
- **Décision de projet :** implémentation limitée à une
  récupération explicite d'un seul RAW Gmail manquant. Le plan R1 n'est pas
  traité comme une capability d'écriture.
- **Faits vérifiés par tests :** égalité exacte restaurée byte pour byte;
  digest divergent, 404, erreur réseau, source `deleted`, identité ambiguë,
  contradiction catalogue et RAW déjà disponible sont fail-closed. La
  frontière Gmail n'est pas utilisée par l'action.
- **Limitation ouverte :** le hook de panne après append durable avant
  transaction catalogue n'était pas encore ajouté à ce premier état ; ce
  point est traité par le correctif d'audit ci-dessous.

## 2026-08-29 — corrections d'audit Sol R2.1a

- **Correction :** `source_account` est désormais reconstruit depuis le profil
  Gmail par le helper unique `gmail_source_account`, dans le même espace opaque
  `gmail:<BLAKE3(email canonique)>` que l'ingestion. `gmail_message_identity`
  est également unique et partagé entre ingestion, pré-validation et CAS.
- **Preuves renforcées :** les tests snapshotent les segments RAW (taille et
  BLAKE3), SQLite et les sidecars avant les refus pré-append ; le test de
  conflit CAS capture aussi l'ancienne localisation, le digest et toutes les
  métadonnées Gmail, puis vérifie que seule une nouvelle frame orphan durable
  apparaît. R2.1a est fermée pour son périmètre, sous réserve de la prochaine
  extension IMAP.
- **Contrat produit :** le GUI et la synchronisation refusent désormais un
  profil sans e-mail, vérifient tout compte configuré contre le profil OAuth,
  et dérivent toujours `source_account` depuis ce profil ; `--account` n'est
  qu'une assertion d'adresse.

- **Faits vérifiés :** l'action acquiert maintenant l'autorité seule, réconcilie
  `inventory_records` avec `inventory_physical`, vérifie le lien canonique et
  le profil Gmail avant `get_raw`, puis vérifie l'ID retourné.
- **Décision de projet :** l'append de recovery ouvre un segment frais créé
  exclusivement avec `create_new`, après validation distante et digest exact ;
  il ne peut donc pas recréer la localisation manquante.
- **Fait vérifié par test :** une précondition catalogue rendue obsolète après
  append durable produit `RecoveryConflict`, conserve l'ancienne claim et
  laisse la nouvelle frame comme `OrphanValidated`.

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

## 2026-08-22 — Indexation du texte des pièces jointes

- Inventorié anonymement les 25 pièces jointes de l’archive Gmail réelle.
- Ajouté `attachment_text` comme champ Tantivy dérivé ; RAW, catalogue et
  framing inchangés.
- Intégré le décodage `text/*` et un provider PDF `pdftotext` optionnel,
  lancé sans shell avec limites d’entrée/sortie et timeout.
- Rebuild réel : 20 formats supportés, 19 textes extraits, 5 non supportés,
  0 échec bloquant ; recherche fixture validée pour texte et PDF.
- Rapport : `experiments/2026-08-22-mail-archive-attachment-text-indexing.md`.

## 2026-08-22 — Providers d’extraction observables

- Centralisé la découverte de `memoria-text` et `poppler-pdftotext`, avec
  disponibilité, version Poppler et chemin résolu sans shell.
- Ajouté `ProviderSelection::Automatic` et `Explicit(ProviderId)` ; le
  pipeline réel utilise désormais la même sélection que l’API d’observation.
- Aucun écran Settings n’existe encore dans Memoria ; aucune surface UI
  artificielle n’a été ajoutée.
- La version `pdftotext -v` n’est plus interrogée : la présence du chemin
  suffit à déclarer le provider disponible. `display_name` reste diagnostique
  et l’ID stable est la frontière i18n. La découverte `OnceLock` reste figée
  pendant le processus ; un refresh est réservé à une future surface Settings.

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

## 2026-08-22 — Probe Windows IFilter

- Créé un probe/helper Windows isolé utilisant les bindings officiels `windows`
  et `LoadIFilter`/`IFilter`, avec limites d'entrée et de sortie.
- Vérifié la compilation native Linux et le `cargo check` ciblant
  `x86_64-pc-windows-msvc`; le cross-link `cargo-xwin` reste bloqué par
  l'absence réseau/cache CRT dans l'environnement courant.
- Aucun IFilter réel n'a été chargé : la validation PDF/DOCX native et toute
  intégration de `windows-ifilter` sont volontairement reportées.
- Rapport : `experiments/2026-08-22-windows-ifilter-text-extraction.md`.

## 2026-08-22 — Intégration IFilter PDF Memoria

- Ajouté le provider Windows `windows-ifilter` uniquement pour
  `application/pdf`, avant `poppler-pdftotext` sous Windows.
- Ajouté `memoria-ifilter-helper.exe`, avec résolution registre dynamique et
  chemin `CoCreateInstance` + `IPersistStream` + `IFilter`; le processus
  principal ne charge aucun IFilter.
- Le parent écrit un fichier temporaire contrôlé `.pdf`, borne entrée/sortie/
  durée et nettoie après fermeture des handles.
- Linux reste inchangé; aucun support DOCX n’est annoncé.

## 2026-08-22 — Validation IFilter sur Windows natif

- Probe exécuté sur Windows 11 Pro 25H2 x64 (`N16PRO-memoria-gui`) avec
  fixtures générées localement et sans données personnelles.
- HTML : succès via `LoadIFilter`; PDF : `LoadIFilter` unsupported mais CLSID
  direct + `IPersistStream` extrait la phrase fixture ; DOCX :
  `FILTER_E_UNKNOWNFORMAT`; TXT : succès sans chunks.
- Helper release : 170 496 octets ; extraction PDF directe environ 68 ms.
- Reproduction native des trois tests signalés : recovery et HTML échouent
  par durées de vie de handles/fichiers Windows ; structured search reste à
  isoler, assertion inchangée.
- Conclusion : pas d’intégration produit IFilter dans cette passe.

## 2026-08-22 — Résolution dynamique IFilter Windows

- Étendu le probe avec la résolution registre effective et le chemin moderne
  `CLSID → IPersistStream → IFilter`, sans CLSID machine dans le code.
- Validé dynamiquement PDF sur N16PRO; HTML reste validé mais hors périmètre
  utile, TXT est couvert par `memoria-text`.
- Word COM a été tenté avec un sous-processus borné à 30 s; le runner n’a pas
  produit de fixture DOCX, donc le support DOCX reste inconclusif.
- Exercé le superviseur contrôlé : timeout, sortie >8 MiB et crash sont
  terminés/attendus proprement. Aucun backend produit n’a encore été intégré.
- Rapport : `experiments/2026-08-22-windows-ifilter-text-extraction.md`.

## 2026-08-22 — Intégration IFilter DOCX Memoria

- Étendu `windows-ifilter` à
  `application/vnd.openxmlformats-officedocument.wordprocessingml.document`
  avec une extension temporaire contrôlée `.docx`; PDF reste `.pdf`.
- Le helper résout désormais dynamiquement les handlers `.pdf` et `.docx`.
  Aucun CLSID Office n’a été ajouté au produit et Word Automation reste hors
  du processus Memoria.
- Ajouté un test produit Windows : terme présent uniquement dans un DOCX,
  extraction via IFilter, champ `attachment_text`, puis recherche Tantivy.
- Validation native : test ciblé réussi (`1 passed`), extraction DOCX helper
  environ 105 ms, helper CI 169 472 octets, application CI 30 287 360
  octets. Linux reste inchangé : PDF `pdftotext`, DOCX non supporté.

## 2026-08-22 — Probe IMAP

- Créé `experiments/imap-probe/` avec fixtures MIME synthétiques, client
  `async-imap`/Tokio, `EXAMINE`, `LIST`, UID FETCH `BODY.PEEK[]`, comparaison
  SHA-256 et erreurs bornées.
- Validé 12/12 messages byte-exactement en local et depuis Windows vers
  GreenMail Linux sur le LAN, sans marquage `\\Seen`.
- Ajouté un mini-chemin IMAPS rustls pour GreenMail local ; le certificat de
  test est accepté uniquement dans le probe. Rapport :
  `experiments/2026-08-22-mail-archive-imap-probe.md`.

## 2026-08-22 — IMAPS avec CA de test

- Remplacé le chemin TLS de diagnostic par une validation normale rustls avec
  CA PEM explicite, certificat GreenMail PKCS#12 signé et SAN `localhost`.
- Validé IMAPS GreenMail depuis Linux et depuis Windows vers le serveur Linux,
  avec greeting, login, EXAMINE et FETCH des 12 fixtures.
- L’échec Windows initial est classé `GREENMAIL_TEST_CERTIFICATE`; STARTTLS
  reste hors de cette campagne.

## 2026-08-22 — Premier import IMAP Memoria

- Ajouté le module `imap` et le CLI `imap-import` : IMAPS validé par CA,
  `EXAMINE`, `BODY.PEEK[]`, runtime Tokio isolé, insertion RAW/catalogue et
  mise à jour Tantivy.
- Ajouté `imap_messages` comme table de provenance IMAP avec identité
  source/mailbox/UIDVALIDITY/UID ; ces métadonnées observées ne sont pas
  reconstructibles depuis le RAW seul. Pas de modification du framing RAW.
- Validé GreenMail Linux puis Windows natif : 12 nouveaux messages, second
  import à 0 nouveau, recherches Unicode/attachment et export EML byte-exact.
- Rapport : `experiments/2026-08-22-mail-archive-imap-import.md`.

## 2026-08-22 — Synchronisation IMAP incrémentale minimale

- Ajouté `imap_scan_state` avec frontière explicite par source/mailbox/
  UIDVALIDITY ; elle n'avance qu'après une campagne complète terminée.
- `EXAMINE`/UIDNEXT borne désormais le FETCH aux UID nouveaux du snapshot ;
  chaque tranche `--limit` réussie publie sa borne UID effectivement parcourue,
  jamais le maximum des UID retournés.
- Remplacé l'accumulation de tous les FETCH par un traitement progressif
  message par message avant archivage/catalogue.
- GreenMail Linux : 12 initiaux, 0 refetch inchangé, 3 nouveaux seuls ; le
  changement UIDVALIDITY est refusé. Le runner Windows était injoignable pour
  le replay incrémental de cette passe.
- Rapport : `experiments/2026-08-22-mail-archive-imap-import.md`.
## 2026-08-24 — Import IMAP multi-mailbox

- Ajouté la découverte `CAPABILITY`/`LIST`, avec delimiter, attributs et
  SPECIAL-USE conservés séparément dans `imap_mailboxes`.
- Étendu `imap-import` à plusieurs `--mailbox` et `--all-mailboxes`, sans
  fusion d’occurrences et avec frontières incrémentales indépendantes.
- Validé sous Linux avec GreenMail 2.1.12 et une CA dédiée : hiérarchie
  construite avec le delimiter découvert `.`, `Projects.Alpha`/`.Beta`,
  relance à zéro et ajout limité à `Projects.Alpha`.
- LIST a retourné le nom protocolaire modified UTF-7 `Caf&AOk-`, delimiter `.`,
  sans SPECIAL-USE ni capacité IMAP4rev2/UTF8-ACCEPT. Le replay Windows a
  ensuite été validé sur N16PRO.

## 2026-08-24 — Replay Windows IMAP incrémental et multi-mailbox

- Rejoué sur N16PRO au commit `0f348371515d1e57b241894e2a767019541750e0` avec
  GreenMail Linux, CA de test dédiée et validation rustls normale.
- Validé l'incrémental `12 → 0 → 3 → 0`, puis `--limit 5` avec frontières
  `0→5→10→15` et répétition sans fetch.
- Validé `LIST`/`EXAMINE` multi-mailbox avec delimiter `.`, hiérarchie
  `Projects.Alpha`/`Projects.Beta`, `Caf&AOk-` et `Projects/with-slash`;
  seul `Projects.Alpha` a fetché le message ajouté au troisième passage.
- Vérifié l'absence de `\\Seen`, 20 occurrences, 12 RAW distincts et 8
  duplications byte-identiques sans fusion de provenance; export EML byte-exact
  vérifié sur une frame de l'archive Windows.
- `cargo check --workspace` et `cargo test --workspace` natifs Windows passent.

## 2026-08-24 — Corpus expérimental MDN/DSN

- Durci `experiments/mdn-dsn-corpus-probe/` : 44 messages MIME synthétiques,
  oracles golden générés par spécification mais indépendants de mailparse,
  et mutations qui doivent échouer avec l’oracle inchangé.
- Vérifié avec `mailparse 0.16.1` la représentation effective des rapports
  MDN/DSN, des groupes DSN, des troisièmes parties et des types RFC 6533 ;
  ajouté les champs DSN per-message RFC 3464, les contrôles MDN requis et les
  cas croisés `Original-Message-ID`.
- Corrigé `dsn-13` : champs per-recipient déplacés dans leurs blocs, avec
  `Will-Retry-Until` uniquement sur le destinataire `delayed`; mutation
  invalidante ajoutée.
- Le probe classe les rapports valides, malformés et non supportés sans
  modifier Memoria ; rapport : `experiments/2026-08-24-mdn-dsn-corpus.md`.

## 2026-08-25 — Parser produit MDN/DSN initial

- Ajouté `delivery_report::analyze_delivery_report` dans le crate
  `mail-archive-experiment`, sans changement de stockage ou d’index.
- Les tests produit lisent directement les 44 `.eml` et `.expected.json` du
  corpus commité et valident Ordinary, MDN, DSN, Malformed et Unsupported.

## 2026-08-25 — Hardening parser MDN/DSN

- Étendu la syntaxe Status DSN, l’OWS/modifiers de Disposition MDN et
  l’unfolding des champs pliés.
- Ajouté deux golden fixtures ciblées, portant le corpus à 44, et des tests
  directs pour MIME non parsable et multipart/report incohérent.
- Les MIME non parsables retournent désormais `Unparseable` sans classification
  textuelle ; aucune fonctionnalité produit connexe n’a été modifiée.

## 2026-08-25 — Corpus HTML remote-resource evidence

- Créé `experiments/html-remote-evidence-probe/` avec 30 fixtures HTML et
  30 golden JSON, entièrement offline.
- Observé séparément les URLs distantes, références locales, liens et signaux
  explicables ; aucun jugement de tracking ni modification Memoria.
- Rapport : `experiments/2026-08-25-html-remote-evidence-corpus.md`.

## 2026-08-25 — API produit HTML remote-resource evidence

- Ajouté `html_remote_evidence::analyze_html_remote_evidence` sans réseau,
  UI, SQLite ni Tantivy.
- Les tests produit consomment directement les 30 fixtures/goldens du probe ;
  exploration déterministe de HTML malformé ajoutée sans nouvelle dépendance
  de fuzzing.
- Validation produit : les tests de l’API passent ; `cargo test --workspace`
  échoue encore uniquement sur le test HTML préexistant dépendant de `/var/tmp`
  en lecture seule dans l’environnement local.

## 2026-08-25 — Correction de propriété du TreeSink HTML

- Remplacé le stockage `Box::leak(QualName)` par des handles `Rc<RefCell<_>>`.
- Ajouté des tests de libération, répétition sur 1 000 éléments et absence de
  doublons lors de la reconstruction html5ever.
- Les 5 tests ciblés passent ; le workspace complet reste en échec uniquement
  sur le test HTML dépendant de `/var/tmp` en lecture seule dans l’environnement
  local.

## 2026-08-25 — Adoption de la politique d’assurance

- Adopté [`ASSURANCE.md`](ASSURANCE.md), version 0.2, comme spécification de
  référence pour la criticité, les frontières d’autorité et les exigences de
  conservation de Memoria.

## 2026-08-25 — Checkpoint Tier A1

- **Superseded/corrigé par l’audit cold-start du 2026-08-26 :** le checkpoint
  annonçait A1 clôturé, mais ne couvrait ni la validation v1 du catalogue ni
  la liaison BLAKE3 ; A1 global reste ouvert jusqu’à A1.2.

## 2026-08-25 — Implémentation A2.1 : inventaire RAW/catalogue

- Ajout de `inventory_records`, API minimale d’inventaire par record, en
  lecture seule stricte de SQLite et des segments.
- La validation de frame reste celle de `read_record`; aucune seconde
  implémentation du framing, aucun appel à `recover_segments` et aucune
  recherche globale de magic ne sont utilisés.
- Tests ciblés ajoutés pour les cinq corruptions centrales, la disparition
  d’un segment, une coordonnée négative et une queue tronquée. Le test de
  corruption compare aussi les octets du segment et l’état (présence et
  contenu) de `metadata.sqlite`, `metadata.sqlite-wal` et
  `metadata.sqlite-shm` avant/après l’inventaire.
- Commande de reproduction ciblée :
  `cargo test -p mail-archive-experiment inventory_ -- --nocapture`.

## 2026-08-25 — Implémentation A2.2 : indexation Tantivy partielle

- Le chemin incrémental `index_gmail_archive`, appelé après les
  synchronisations Gmail/IMAP, revalide désormais le RAW lié même sur son
  fast-path pour les fingerprints inchangés. `rebuild_gmail_archive` revalide tous les RAW `present` via
  `inventory_records`; les corruptions postérieures à l’indexation sont ainsi
  détectées explicitement en mode rebuild.
- Les suppressions Tantivy et `indexed_docs` passent par le même chemin local
  pour les RAW indisponibles/incohérents, les mismatches d’ID, les records
  supprimés et les erreurs de parsing.
- Statistiques ajoutées : RAW indisponible, RAW incohérent et index partiel.
- Tests ajoutés : corruption centrale avec voisins recherchables,
  segment manquant, restauration/réindexation et échec de parsing avec
  cohérence exacte de Tantivy et `indexed_docs`.
- Contrôles : 68 des 69 tests du crate Memoria passent; le seul échec est le
  test HTML préexistant qui écrit dans `/var/tmp` en lecture seule.
  `cargo check --workspace`, Clippy, fmt et `git diff --check` passent.

## 2026-08-26 — Checkpoint Tier A2.3 : ancien recovery destructif hors service

- `recover_segments`, la commande `recover-demo` et le test garantissant la
  troncature automatique ont été retirés des surfaces actives.
- A2.1 reste le diagnostic read-only et A2.2 la reconstruction non destructive
  des index dérivés. Aucun scanner global au magic et aucune nouvelle fonction
  de troncature n’ont été ajoutés.
- Le test d’inventaire avec queue incomplète vérifie que les frames cataloguées
  restent disponibles et que les segments, `metadata.sqlite`, `metadata.sqlite-wal`
  et `metadata.sqlite-shm` restent byte-à-byte inchangés lorsqu’ils existent.
- La réparation physique est différée jusqu’au chantier
  crash-consistency/publication.

## 2026-08-26 — Implémentation A3.1 : publication catalogue atomique

- `insert_gmail_metadata` et `insert_imap_metadata` utilisent désormais une
  transaction SQLite unique pour `messages` et l’identité/provenance source.
- Le catalogue Tier A est configuré en `journal_mode=DELETE` et
  `synchronous=EXTRA`; un test vérifie les valeurs PRAGMA réellement actives.
- Une fault injection déterministe entre les deux insertions vérifie le
  rollback complet pour Gmail et IMAP, puis le retry sans blocage `UNIQUE`.
- Cette étape ne modifie ni l’ordre RAW → SQLite, ni `ArchiveWriter`, ni les
  syncs, ni Tantivy ; A3 global reste non résolu.

## 2026-08-26 — Implémentation A3.2a : primitive de durabilité RAW

- `ArchiveWriter` distingue désormais les locations RAW pending des lots
  durablement barriérés et expose l’état minimal Ready/Poisoned.
- Une erreur d’écriture de magic, ID, longueur, checksum ou payload empoisonne
  le writer ; les barrières et append ultérieurs échouent sans recalage ni
  troncature. Une nouvelle ouverture reprend à l’EOF.
- Les barrières synchronisent les écritures dirty et, sous Unix, le namespace
  du répertoire d’archive lorsqu’un segment a été créé, sous la précondition
  que ce répertoire est déjà établi durablement. `ArchiveWriter::open` peut
  créer le root, mais son parent n’est pas synchronisé ; cette durabilité
  initiale reste une dette séparée. La garantie namespace n’est pas revendiquée
  sous Windows.
- Tests ajoutés pour les composants en échec, les barrières retryables, la
  rotation, le namespace, les barrières vides et la réouverture.
- Gmail/IMAP, SQLite, le format et l’ordre de publication restent inchangés ;
  A3.2b est nécessaire pour leur coordination.

## 2026-08-26 — Implémentation A3.2b : group commit RAW → catalogue

- Les chemins d’import Gmail et IMAP initialisent `next_doc_id` une seule fois,
  dédupliquent les identités pending et regroupent les métadonnées après
  `append_raw`.
- `durable_barrier()` précède chaque transaction SQLite de lot ; le publisher
  vérifie le `batch_id`, le nombre de records et la somme des `frame_bytes`
  avant d’insérer `messages` et la ligne source correspondante.
- Les erreurs avant/pendant la barrière ou dans la transaction ne publient pas
  le lot ; les RAW durables restent orphelins. Les curseurs et frontières
  restent hors périmètre ; A3 global reste ouvert jusqu’à A3.3.
- La liaison runtime est maintenant exacte : chaque location contient un
  ordinal et le `doc_id` écrit ; les publishers refusent les doublons, trous,
  ordinaux hors plage et identités discordantes, sans relire les RAW.

## 2026-08-26 — Implémentation A3.3 : frontières source et retry fail-closed

- Gmail sépare maintenant les parcours complets des parcours `query`/`max` ;
  la fence complète non vide est prise avant le scan et `profile.historyId`
  n’est pas utilisé comme fence. Une boîte vide ne publie aucune nouvelle
  frontière ; l’incremental exige un `historyId` terminal avant le commit
  séparé du curseur.
- Les skips Gmail/IMAP valident désormais la ligne `messages`, l’identité
  canonique et la frame RAW avec A1. Les identités existantes incohérentes et
  les collisions historiques canoniques bloquent sans réparation ; une
  suppression Gmail inconnue reste un no-op explicite.
- IMAP rejette `limit=0` et ne publie la frontière qu’après FETCH, lots A3.2 et
  session réussis. Aucune réconciliation complète des flags/suppressions ni
  migration d’archives pré-A3 n’a été ajoutée.

## 2026-08-26 — Implémentation A1.1 : liaison catalogue v1 ↔ RAW par BLAKE3

- Catalogue neuf et ouverture existante sont séparés : l’ouverture refuse
  explicitement les versions/structures incompatibles avant modification.
- `PendingRawLocation` transporte désormais une référence liée (`doc_id`,
  `ArchiveLocation`, BLAKE3). Les publishers Gmail/IMAP et le générateur
  écrivent `raw_blake3` atomiquement avec les coordonnées, après la barrière
  RAW existante ; le format des segments reste inchangé.
- `read_archived_raw` vérifie maintenant la liaison BLAKE3 après les contrôles
  de frame existants. Le test de substitution après restart, ainsi que les
  variantes hash/payload/FNV et le refus byte-à-byte inchangé d’un catalogue
  legacy, passent.
- A1.2 doit encore propager cette référence liée à tous les lecteurs
  autoritatifs secondaires ; ce patch ne traite pas A3.2, suppressions Gmail,
  orphelins, namespace ni A4.

## 2026-08-26 — Correction audit cold-start A1.1

- La clôture A1 précédente est invalidée : elle décrivait `read_record` comme
  lecteur autoritatif et ne couvrait pas la validation complète du catalogue
  v1.
- La validation existante passe désormais par une connexion SQLite strictement
  read-only, puis `open_catalogue` réouvre en read-write et rétablit les PRAGMA
  runtime. Les catalogues incompatibles sont rejetés sans mutation observable.
- La création construit le schéma dans une transaction avant de publier les
  marqueurs `application_id`/`user_version`. Le corpus et
  `structured-search-benchmark` synchronisent maintenant le RAW avant le
  commit SQLite et insèrent la référence BLAKE3.
- **État :** A1.1 est obtenu ; A1.2 reste ouvert pour les lecteurs
  autoritatifs secondaires.

## 2026-08-26 — A1.2 : propagation aux lecteurs secondaires

- Inventaire : `read_record` est conservé pour la lecture physique et
  `inventory_records` pour le diagnostic indépendant ; ils ne sont pas des
  lectures catalogue autoritatives. L’analyse MIME, `for_each_archived_message`,
  la validation d’identité source et l’indexation/reconstruction Tantivy sont
  les lecteurs autoritatifs traités.
- Ces chemins consomment maintenant `RawReference`/`read_authoritative_raw`;
  le fast-path incrémental revalide également le RAW avant de conserver une
  entrée indexée. Les coordonnées valides vers une autre frame et les digests
  erronés échouent sans substitution silencieuse.
- Test ajouté : l’index secondaire refuse la substitution de la frame du
  `doc_id=0` à celle du `doc_id=1` et un digest catalogue incorrect, tandis
  que la référence correcte reste indexable. Le scénario même `doc_id`, avec
  contenu différent, est déjà couvert par le test A1.1 de `read_archived_raw`.
- **État :** A1 global est fermé : les lecteurs autoritatifs identifiés par
  cet inventaire ont été corrigés ; les primitives physiques restent hors de
  cette frontière d’autorité.

## 2026-08-26 — Correction Sol High : validation catalogue complète

- Le dernier bypass `Connection::open` de `archive_summary` a été supprimé ;
  `inventory_records` valide également le catalogue v1 avant sa seconde
  ouverture read-only. Les helpers de création restent le cas distinct des
  catalogues neufs.
- La validation compare désormais, via les PRAGMA structurels, l’ordre, le
  type, la nullabilité, les défauts, la PK et l’absence de colonnes
  supplémentaires de chaque table contractuelle, ainsi que les index
  explicites/implicites, leur ordre, unicité et caractère partiel.
- `raw_blake3` est vérifié par comparaison de la représentation
  `sqlite_schema.sql` produite par SQLite depuis la DDL canonique Memoria dans
  une base en mémoire ; aucune écriture ou récupération n’est effectuée sur le
  fichier existant. Les tests vérifient aussi les
  catalogues version 0 à application_id correct, colonnes/index incompatibles,
  SQL trompeur et l’absence byte-à-byte de sidecars lors du refus.
- **État corrigé :** la correction A1.2 ci-dessus a depuis propagé la
  référence liée aux lecteurs autoritatifs secondaires ; A1 global est fermé.

## 2026-08-27 — A3.2 sealing : fermeture des API de publication

- Inventaire exhaustif des écritures `messages` et des usages
  `ArchiveLocation`/offset/length/doc_id sous `projects/mail-archive` : les
  bypass produit retrouvés étaient les insertions unitaires acceptant un
  `PendingRawLocation`, les champs publics et le `Deref<ArchiveLocation>` de
  ce type, ainsi que les transactions corpus et
  `structured-search-benchmark` ouvertes/alimentées avant `sync`.
- `PendingRawLocation` ne transporte plus que l’association opaque
  sceau/ordinal/doc_id/taille, tous champs privés. Le writer conserve les
  coordonnées et le BLAKE3 jusqu’à `durable_barrier`, qui construit les seules
  `DurableRawLocation` publiables. Aucun getter/conversion pending vers
  `ArchiveLocation` ou `RawReference` n’existe.
- `GmailBatchRecord`, `ImapBatchRecord` et `CatalogueBatchRecord` sont des
  structures de staging à champs privés. Les publishers valident le sceau de
  lot par identité, les ordinaux exacts et uniques, le `doc_id`, le compte de
  records et les bytes, puis commencent seulement la transaction SQLite. Les
  coordonnées et `raw_blake3` insérés viennent de l’entrée durable résolue.
- Gmail et IMAP conservaient déjà l’ordre barrière puis publisher ; ils ont
  été adaptés à la nouvelle API. Le corpus est maintenant publié par lots
  bornés 256 records/16 MiB après chaque barrière. Le benchmark structuré
  utilise le même publisher Gmail et ne contient plus de SQL `messages`
  direct. Les outils physiques sans publication gardent `sync`, `read_record`
  et `inventory_records` hors de cette frontière.
- Tests adversariaux : batch exact, doublon/trou/mauvais ordinal, mauvais
  `doc_id`, reçu d’un autre batch, compte/bytes divergents, rollback/retry,
  erreur de sync sans identité, correspondance exacte des entrées durables et
  BLAKE3 Gmail/IMAP issu de l’append. L’impossibilité de publier un pending
  avant barrière est garantie par les types et la visibilité, plutôt que par
  un test runtime artificiel.
- Validation finale : `cargo test --workspace` (92 tests de bibliothèque
  mail-archive, 8 tests de ses binaires et 2 tests workspace, tous passants),
  `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check` et `git diff --check` passent.
- **Décision :** A3.2 sealing est fermé ; aucun bypass produit restant n’a été
  retrouvé. Aucun commit n’est créé.

## 2026-08-27 — A3.2 sealing : correction de la liaison catalogue/writer

- La surface publique ne renvoie plus de connexion RW de catalogue : les
  constructeurs de catalogue sont internes et `CatalogueConnection` garde sa
  connexion derrière des champs privés. `create_sqlite_fts` expose maintenant
  `SqliteFtsIndex`, un handle opaque dont la connexion est privée ; les APIs
  publiques SQLite restantes servent aux index/états dérivés et ne constituent
  pas une API SQL de publication `messages`.
- Une autorité opaque process/session est maintenant partagée par le writer et
  le catalogue associés. Le reçu `DurableRawBatch` conserve cette autorité et
  chaque publisher la vérifie avant d’ouvrir sa transaction ; un batch durable
  de A est refusé par B avant toute insertion.
- Test adversarial ajouté : publication A→B refusée avec catalogue B inchangé,
  puis publication A→A réussie. Les chemins Gmail, IMAP, corpus et benchmark
  utilisent l’API liée existante.
- **État corrigé :** après cette correction, le sealing A3.2 est fermé pour
  l’échappement de connexion RW et la liaison reçu/catalogue. Cela ne clôt pas
  les travaux hors périmètre A3.2 listés ci-dessus. Aucun commit n’est créé.

## 2026-08-27 — A3.3 : audit et correction des transitions Gmail

- **Inventaire vérifié :** `sync_account` choisit full initiale, full après
  état incomplet, incremental history, ou full de repli après expiration.
  Full sans `query`/`max` énumère tout et réconcilie les absences ; les modes
  borné/filtré mettent à jour les messages observés sans toucher aux absences.
  Les métadonnées connues sont rafraîchies par `METADATA`, les nouveaux RAW
  passent par append durable puis publication catalogue.
- **Bug trouvé :** une tentative intermédiaire utilisait profile.historyId,
  puis l’audit a conduit à revenir au historyId du premier message de la
  première page comme frontier de full sync.
- **Bug trouvé :** delete et absence ne validaient pas le RAW autoritatif.
  Toute identité connue est maintenant revalidée avant transition ; une
  corruption bloque sans avancer le frontier et sans supprimer le RAW.
- **Bug trouvé :** une tentative intermédiaire donnait priorité à delete
  dans un record, puis l’audit a conduit à supprimer toute priorité
  intra-record et à résoudre les ambiguïtés via Gmail.
- **Tests adversariaux :** ajoutés pour `add+delete`, RAW conservé/état final
  deleted, delete sur RAW invalide, échec de page et retry, plus absence full
  contre parcours borné/filtré. Les tests existants couvrent la boîte vide et
  l’idempotence/pagination.
- **État :** corrections A3.3 implémentées ; validation globale restante avant
  verdict. Aucun commit n’est créé.
- 2026-08-27 — chantier RAW orphelins : ajout de `inventory_physical`, scan
  séquentiel sans mutation et classification par identité physique complète.
  Les frames valides sans référence exacte sont `OrphanValidated`, y compris
  lorsque leur `doc_id` est réutilisé par une frame publiée. Les anomalies de
  catalogue restent `Inconsistent`; corruption non resynchronisable et queue
  partielle restent séparées. Ajout de compteurs à `ArchiveSummary`, de la
  commande diagnostique `archive-inventory` et de tests same-`doc_id`, tail,
  MIME invalide et rebuild Tantivy catalogue-only. Aucun recovery n’est
  implémenté et aucun commit n’a été créé.
- 2026-08-27 — correction Sol High : le scanner s’arrête désormais après toute
  corruption de framing/checksum car l’en-tête n’est pas authentifié ; une
  longueur au-delà de l’EOF est corruption, tandis qu’une queue trop courte
  pour un header reste `IncompleteTail`. L’association catalogue minimale est
  `segment + offset`, ce qui empêche le double classement orphan/inconsistent
  sur un `frame_bytes` erroné. Ajout du compteur physically missing, d’une
  allocation `try_reserve_exact` et d’une preuve summary strictement
  read-only. Aucun recovery ni commit.
- 2026-08-27 — correction finale de classification : `catalogue_frame_sets`
  conserve maintenant la revendication `segment + offset` avant de valider
  `doc_id` et `frame_bytes`. Une ligne négative mais physiquement localisable
  empêche donc `OrphanValidated` et produit `CataloguedInconsistent`.

## 2026-08-27 — Tier A : single-writer inter-processus

- **Inventaire vérifié :** avant cette passe, `ArchiveWriter::open` créait ou
  ouvrait le segment avant toute contention inter-processus. SQLite fournissait
  seulement une contention indirecte sur le catalogue; elle ne protégeait pas
  les segments RAW. Gmail et IMAP ouvraient aussi leur catalogue RW avant leur
  writer. Aucun lockfile, `flock`, `fcntl`, `FileExt`, PID file ou `create_new`
  ne constituait une autorité d’archive complète.
- **Correction :** `fs4 0.13.1` est déclaré directement. Un lock exclusif OS
  stable, hors du sous-arbre resettable de l’archive logique, est acquis avant
  `create_dir_all(archive)`,
  l’ouverture/création de segment et toute création/ouverture RW SQLite.
  `ArchiveAuthority` garde le handle au-delà du lifetime de
  `ArchiveWriter`; le fichier persistant est un rendez-vous uniquement et
  l’OS porte l’autorité.
- **Surface fermée :** `ArchiveSession`, création UI, Gmail, IMAP, corpus et
  benchmark utilisent le même contrat. A3.2 reste intact : le lock complète,
  sans la remplacer, l’association `DurableRawBatch`/catalogue et le refus
  cross-archive.
- **Faits vérifiés :** second writer intra-processus et inter-processus refusé
  avant mutation observable; archives distinctes indépendantes; sortie normale
  et kill brutal du processus enfant libèrent le lock; alias relatif et
  symlink existant convergent par canonicalisation du rendez-vous. Les
  lecteurs read-only ne prennent pas le lock.
- **Limites :** protection des filesystems locaux uniquement; pas de verrou
  distribué NFS/SMB ni d’identité préservée lors d’un rename en utilisation.
  Aucun protocole PID stale n’est utilisé.
- **Validation ciblée :** `cargo test -p mail-archive-experiment --test
  archive_lock -- --nocapture` passe. Validation globale ci-dessous.
- **Décision :** `single-writer enforced; multiwriter deliberately unsupported`.

## 2026-08-28 — Correction Sol High du chantier single-writer

- **Corrections vérifiées :** le guard a été déplacé dans `ArchiveAuthority`,
  `ArchiveSession::reset` verrouille avant suppression, le rendez-vous est
  sibling du conteneur supprimable, et le benchmark ne précrée plus
  `archive/`. Les mutateurs catalogue acceptant une `Connection` arbitraire
  sont désormais internes; les helpers de création restants sont `cfg(test)`.
- **Tests ciblés :** remplacement du writer avec catalogue/session survivants,
  drop order, reset refusé puis réussi après libération, reset CLI réel,
  création concurrente d’une cible inexistante, sortie normale, kill brutal,
  alias relatif/symlink et archives A/B indépendantes.
- **Décision :** la propriété reste `single-writer enforced; multiwriter
  deliberately unsupported`; NFS/SMB et rename concurrent restent hors
  garantie.

## 2026-08-28 — Tier A R1 : recovery read-only

- **Fait vérifié :** inspection des inventaires, lecteurs autoritatifs,
  `ArchiveSession`, schéma SQLite v1 et identités Gmail/IMAP. Le join
  physique est complet ; `doc_id` n'est pas une identité source.
- **Implémentation :** ajout de `recovery::plan_recovery` et de la commande
  `recovery-plan`. Les actions sont proposées, jamais automatiques : aucun
  re-fetch, relink, adoption, truncation, frontier update ou écriture.
- **Tests ciblés :** cible absente sans création, catalogue invalide
  fail-closed et répétition déterministe ; les tests d'inventaire existants
  couvrent same-`doc_id`, corruption, coordonnées/BLAKE3 incohérents, missing
  segment et incomplete tail.
- **Décision :** le catalogue v1 ne représente pas honnêtement un salvage
  sans provenance ; future exécution = modèle séparé ou évolution explicite.
  Aucun commit.

## 2026-08-28 — R1 : corrections d’audit Sol High

- **Correction :** `metadata.sqlite` est considéré absent uniquement sur
  `NotFound`; toute autre erreur d’accès reste fail-closed. Catalogue perdu
  et archive vide ne produisent aucun salvage fictif : chaque salvage vient
  d’une frame RAW réellement validée.
- **Correction :** les identités source sont exposées dans
  `RecoveryEvidence` avec leurs valeurs observées. Compte/ID ou mailbox/UID
  invalides, état source non interprétable, identité supprimée ou conflit
  rendent la classification non optimiste.
- **Correction :** une contradiction au même emplacement physique prime sur
  `PhysicallyMissing`; une corruption n’est jamais `SalvageOnly`; une tail
  revendiquée devient unsafe. Une archive RAW absente est traitée record par
  record.
- **État :** implémentation candidate corrigée, validations finales Sol High
  en cours. Aucun recovery exécuté et aucun commit.
