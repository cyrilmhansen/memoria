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
