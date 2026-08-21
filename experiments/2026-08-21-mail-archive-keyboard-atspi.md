# Validation clavier et AT-SPI — première UI mail

## Périmètre

Validation locale de l'application hors ligne, sans accès Gmail. Les sorties
et rapports ne contiennent aucun sujet, adresse, extrait ou corps de message.

## AT-SPI réel

Après activation temporaire de `org.a11y.Status.IsEnabled` dans la session,
`python-atspi` a trouvé l'application Wayland et un arbre de 15 nœuds dans
l'état initial. Les rôles/labels génériques observés sont :

- fenêtre `frame` « Memoria — Archive » ;
- entrée `entry` de recherche ;
- bouton d'effacement ;
- `list box` des résultats ;
- région de lecture ;
- trois boutons de zoom.

Les labels de contrôle sont présents et non vides. Les rôles Slint sont
convertis par AccessKit vers AT-SPI. La tentative de modifier le texte de
l'entrée via `pyatspi.EditableText` retourne `NotImplementedError` dans cette
version de la passerelle ; elle ne permet donc pas de simuler une frappe par
AT-SPI.

## Clavier

Le scénario reproductible sous Xvfb/X11, avec le même binaire et le même
layout logique, couvre :

- Ctrl+F : focus recherche ;
- frappe : recherche réactive et liste de résultats ;
- flèche bas : sélection du premier résultat ;
- Entrée : lecture du résultat ;
- Ctrl+plus / Ctrl+moins / Ctrl+0 : zoom du corps, borné entre 10 et 28 px ;
- Échap depuis un message : retour du focus vers la liste sans effacer la
  recherche ;
- Ctrl+Retour arrière depuis la liste : effacement du champ, des résultats et
  du message courant.

Les commandes de zoom sont aussi activables par boutons AT-SPI et portent des
labels explicites. Home/End et PageUp/PageDown déplacent la sélection par
borne ou par pas de dix.

`ydotoold` démarre avec un socket privé mais ne crée pas son périphérique
virtuel : `/dev/uinput` appartient au groupe `input`, auquel l'utilisateur de
session n'appartient pas, et aucun `sudo` non interactif n'est disponible.
L'injection clavier Wayland directe par ydotool n'a donc pas pu être validée.
Cette limite est environnementale et n'a pas été contournée en modifiant les
permissions système.

## Corrections

- retour clavier vers le `FocusScope` des résultats ;
- navigation haut/bas, Home/End et PageUp/PageDown ;
- raccourcis de zoom et commandes accessibles visibles dans le chrome fixe ;
- effacement direct du `LineEdit` dans tous les chemins d'effacement, après
  découverte d'un état où les résultats disparaissaient mais le texte restait
  affiché ;
- région de lecture et liste nommées dans l'arbre accessible ;
- labels de lignes combinant correspondant, sujet et date, sans les écrire
  dans les rapports.

## Décisions UX

Échap est contextuel : depuis un message il revient à la liste ; depuis l'état
sans message il efface la recherche. Ctrl+F revient toujours à la recherche.
Le zoom concerne le corps dérivé uniquement ; le chrome reste fixe. Aucun
menu contextuel n'est ajouté tant qu'aucune action produit ne le justifie.

**Fait vérifié :** l'arbre AT-SPI initial est publié et expose les contrôles
principaux avec des rôles cohérents.

**Fait vérifié :** le scénario clavier complet et le zoom fonctionnent dans
le test automatisé X11 ; le même code de raccourcis est utilisé par le
backend Wayland.

**Limite :** injection physique Wayland par ydotool et modification de texte
par l'API EditableText ne sont pas validées dans l'environnement courant.
