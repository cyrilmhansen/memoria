# Memoria — ouverture HTML dans le navigateur système

Date : 2026-08-21

## Résultat

Memoria peut ouvrir une partie `text/html` dérivée du RAW dans le navigateur
système, via un serveur HTTP éphémère lié exclusivement à `127.0.0.1` et à un
port attribué par l'OS. Aucun WebView, QtWebEngine, WebKitGTK ou moteur HTML
n'est ajouté au binaire.

Le bouton `Ouvrir HTML` n'est activé que lorsqu'une partie HTML non considérée
comme pièce jointe est détectée. Le lecteur texte reste inchangé et reste le
fallback principal.

## Sécurité et cycle de vie

Chaque ouverture reçoit un token aléatoire de 192 bits obtenu par
`getrandom`. La session contient le HTML nettoyé et les ressources CID en
mémoire ; elle ne contient aucun chemin de fichier arbitraire. Les routes sont
uniquement :

```text
/<session>/html
/<session>/cid/<token>
```

Les autres chemins, tokens et méthodes HTTP sont refusés. Les réponses
utilisent `Cache-Control: no-store`, `nosniff` et une CSP équivalente à :

```text
default-src 'none'; img-src 'self'; style-src 'unsafe-inline';
script-src 'none'; connect-src 'none'; object-src 'none';
frame-src 'none'; form-action 'none'; base-uri 'none'
```

`ammonia 4.1.4` retire les scripts, formulaires et attributs d'événements. Les
références `cid:` sont remplacées par des routes locales opaques. Les images
HTTP/HTTPS ne sont pas autorisées par la CSP ; les liens externes restent
présentés comme liens et ne sont suivis qu'après action de l'utilisateur.

Le serveur est arrêté à la fin de la session Memoria et son espace de sessions
est alors détruit. Il n'exécute aucune commande shell et ne sert jamais le
filesystem.

## Validation

Fixtures automatisées :

- HTML simple et multipart/alternative ;
- multipart/related avec plusieurs CID ;
- CID absent/malformé ;
- script, `onclick`, formulaire et URL distante ;
- HTML malformé ;
- accès à un CID valide et refus d'une session/route étrangère.

Les tests vérifient notamment que le HTML nettoyé conserve le lien externe,
retire le contenu actif et que la ressource CID renvoie les bytes décodés.

Un smoke test local a ensuite parcouru l'archive Gmail réelle hors ligne,
ouvert dans le navigateur système un message HTML réel, puis un message HTML
réel possédant une ressource embarquée. Aucun sujet, adresse, HTML, CID ou
contenu n'a été imprimé ou conservé dans le dépôt. Le test a seulement vérifié
que l'ouverture système et le serveur local aboutissent sans synchronisation
Gmail.

## Coût

La nouvelle dépendance directe est `ammonia 4.1.4`, avec `html5ever 0.39.0`,
`markup5ever` et les petites dépendances de parsing HTML. `getrandom 0.3.4`
était déjà présent transitivement et sert ici à garantir des tokens de session
non devinables. Le binaire release passe de 29 327 488 à 30 378 656 octets
dans le profil courant, soit environ 1,0 MiB supplémentaire. `ldd` ne révèle
aucune bibliothèque Qt, WebKit, WebEngine ou Chromium.

Le démarrage du serveur est immédiat ; la latence supplémentaire mesurée dans
le smoke test est dominée par le lancement du navigateur système, pas par la
construction de la réponse HTML. Le HTML et les CID sont gardés en mémoire
pendant la session du navigateur ; un streaming de très grosses ressources
reste hors périmètre.

## Limites et décision

**Fait vérifié.** Cette architecture permet de profiter du moteur HTML fidèle
du navigateur et de rendre des CID locaux sans intégrer de moteur dans
Memoria.

**Décision de projet.** Le navigateur système est utilisé uniquement après une
action explicite sur `Ouvrir HTML`. Aucun téléchargement d'image distante n'est
autorisé automatiquement.

**Limites.** Le sanitizer ne promet pas une fidélité parfaite aux contenus
actifs ou aux intégrations externes ; scripts, formulaires, iframes, objets et
ressources distantes sont volontairement neutralisés. Le rendu et la sécurité
finale dépendent aussi du navigateur utilisé ; la CSP reste donc une barrière
de défense obligatoire.

## Correction CID — validation réelle

**Cause vérifiée.** La première réécriture recherchait des chaînes littérales
`cid:<id>` et une variante HTML échappée. Elle ne normalisait pas les références
CID URL-encodées (`%40`, `%3C`, `%3E`) et ne traitait pas uniformément les formes
entourées d'angles. Ammonia supprimait ensuite les `src` CID restés inconnus.
La route, la CSP et `nosniff` n'étaient pas la cause du défaut.

La correction réécrit avant sanitisation chaque référence dont le schéma est
`cid:` après une normalisation stricte : décodage percent-encoding valide,
retrait éventuel des angles, puis égalité exacte avec le `Content-ID` MIME
normalisé. Une référence inconnue reste bloquée par la sanitisation ; aucune
correspondance approximative n'est tentée.

La réponse CID conserve le type MIME de la partie MIME, par exemple
`image/png`, et non un type générique. La CSP `img-src 'self'`, le header
`X-Content-Type-Options: nosniff`, les tokens opaques et le bind localhost
n'ont pas changé.

Le diagnostic hors ligne anonymisé sur l'archive réelle a observé :

```text
html_messages=3002
img_total=36162
cid_src=78
cid_mime_resources=96
cid_matched=78
cid_rewritten=78
cid_get_requests=78
cid_http_200=78
cid_content_type_image=78
```

Un smoke test a ouvert le premier message correspondant dans le navigateur
système via l'URL locale opaque ; le serveur a servi toutes les ressources CID
correspondantes en `image/*`. Aucun contenu, identifiant réel ou octet de
l'archive n'a été journalisé. Les tests couvrent désormais les CID simples,
avec `@`, percent-encodés, plusieurs ressources, CID absent, ressource
non-image et image HTTPS bloquée.
