# Expérience — pièces jointes à la demande dans Memoria

Date : 2026-08-21  
Statut : implémentation et validation locale ; format d'archive inchangé.

## Contrat retenu

Le RAW archivé reste la seule autorité. `list_attachments(doc_id)` et
`read_attachment(doc_id, attachment_id)` relisent le RAW, reparsent le MIME et
renvoient respectivement les métadonnées et les octets décodés. Les
identifiants sont des positions de feuilles MIME et ne sont pas persistés dans
un nouveau catalogue.

Une pièce est affichée dans l'UI si elle possède `Content-Disposition:
attachment`, ou un `filename`/`name` exploitable. Une ressource `inline` avec
`Content-ID` n'est pas présentée comme pièce jointe ordinaire ; elle est
conservée dans `list_mime_resources` pour un futur rendu HTML. Une pièce
`attachment` stricte reste téléchargeable même si elle porte aussi un
`Content-ID`.

## Modifications

- API Rust dérivée dans `projects/mail-archive/src/lib.rs` :
  `AttachmentInfo`, `MimeResourceInfo`, `list_attachments`,
  `list_mime_resources`, `read_attachment`.
- Zone compacte dans le panneau de lecture Slint, visible uniquement lorsqu'il
  existe au moins une pièce téléchargeable.
- `Ouvrir` extrait vers un répertoire temporaire propre au processus puis
  délègue l'ouverture à l'application associée via la crate légère `open`
  (`that_detached`) ; aucune commande shell ni exécution directe du contenu
  n'est utilisée. Sous KDE, le test local a effectivement lancé Gwenview pour
  une image PNG ; l'association PDF configurée localement est
  `cachy-browser.desktop`.
- `Enregistrer sous…` utilise le dialogue natif existant et écrit les octets
  décodés seulement après choix explicite de destination.
- Les noms MIME sont réduits à un nom de fichier sûr : séparateurs, `..`,
  contrôles, caractères invalides et noms réservés Windows sont neutralisés.
- Les fichiers temporaires sont sous `memoria-attachments-<pid>` et supprimés
  à la fin du processus. Une fermeture brutale peut laisser un répertoire
  résiduel ; aucun nettoyage global de `/tmp` n'est effectué.

## Tests

Fixture MIME automatisée couvrant : PDF base64, image inline CID, pièce sans
nom, nom Unicode, quoted-printable, nom `../`, plusieurs feuilles et RAW
malformable via les erreurs du parseur. Elle vérifie que la ressource CID est
retrouvable, que la pièce est décodable et que `read_archived_raw` renvoie
exactement le RAW initial.

Validation locale sur l'archive Gmail réelle, hors ligne et sans contenu
persisté :

```text
messages_with_downloadable_attachments=16
attachments=25
decoded_bytes=41977800
attachment_reads_size_verified=25
raw_reads_completed=true
```

Cette vérification a relu les 3 012 messages et leurs pièces à la demande ;
elle n'a modifié ni archive, ni catalogue, ni index. Elle confirme l'ordre de
grandeur du rapport Gmail réel : 25 pièces jointes strictes pour cet ensemble.
Les ressources CID n'ont pas été comptées comme pièces ordinaires.

Memoria release a été compilé et lancé sur l'archive réelle sous KDE Wayland.
La vérification automatisée complète de clic `Ouvrir`/dialogue `Enregistrer`
reste limitée par l'absence de `ydotoold` dans la session ; les chemins Rust
sont couverts par fixtures et l'application démarre sans erreur.

## Limites

- Aucune prévisualisation intégrée.
- Aucun antivirus ni extraction permanente.
- Les erreurs MIME, décodage, fichier temporaire, écriture et application
  associée sont signalées séparément au niveau du statut UI, mais la richesse
  du diagnostic reste volontairement minimale.
- Les images CID sont exposées par l'API mais pas encore injectées dans le
  renderer texte/HTML.
- La lecture d'une grosse pièce alloue temporairement ses octets en mémoire ;
  il n'y a pas encore de streaming. Cette limite est acceptable pour cette
  première passe et doit être mesurée avant tout support de très grosses
  pièces.
