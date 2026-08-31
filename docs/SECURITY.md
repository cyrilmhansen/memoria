# Memoria — Politique de sécurité

Version 0.1 — 31 août 2026

Ce document définit le modèle de sécurité de Memoria. Il est distinct de
[`ASSURANCE.md`](ASSURANCE.md), qui traite de conservation, fidélité, cohérence
et niveau de contrôle du code.

Le profil de sécurité par défaut de Memoria est :

> **Archive locale de confiance / contenu externe non fiable**

Memoria protège une archive personnelle locale contre les effets non désirés de
contenus, sources et intégrations externes sans prétendre constituer une
frontière de sécurité contre un système d'exploitation ou un compte utilisateur
déjà compromis.

## 1. Hypothèses de confiance

Le modèle par défaut suppose de confiance :

- l'utilisateur local ;
- le système d'exploitation et le compte sous lequel Memoria s'exécute ;
- l'installation de Memoria volontairement acceptée par l'utilisateur ;
- les primitives système ordinaires utilisées explicitement par Memoria.

Sont considérés comme potentiellement non fiables :

- les messages MIME et leurs en-têtes ;
- HTML, pièces jointes, images, calendriers, vCards, MDN/DSN et autres contenus ;
- fichiers EML, MBOX ou autres corpus importés ;
- métadonnées fournies par un service distant ou une source intermédiaire ;
- noms de fichiers, types MIME déclarés et contenus décodés ;
- résultats de parseurs, extracteurs, moteurs de preview ou outils externes ;
- contenu obtenu par le réseau.

Une donnée non fiable peut fournir une information au produit ; elle ne reçoit
jamais, par son seul contenu, une autorité sur l'archive ou sur les comptes
externes.

## 2. Actifs principalement protégés

Memoria protège en priorité :

- les RAW et les autres données Tier A contre les mutations non autorisées ;
- les identités et preuves de provenance Tier A ;
- les secrets OAuth, tokens et autres credentials ;
- les fichiers privés hors archive qui n'ont pas été explicitement sélectionnés ;
- les comptes distants contre les mutations inattendues ;
- la confidentialité de lecture contre le chargement automatique de ressources
  distantes et les mécanismes de tracking ;
- l'utilisateur contre l'exécution implicite de contenu actif provenant d'un
  message ou d'une pièce jointe ;
- la séparation entre contenus non fiables et opérations d'autorité Tier A.

## 3. Non-objectifs

Le profil par défaut ne cherche pas à défendre contre :

- un noyau ou un système d'exploitation compromis ;
- un malware déjà exécuté avec l'identité Unix/Windows de l'utilisateur ;
- un utilisateur local volontairement malveillant ;
- un binaire Memoria malveillant volontairement installé ;
- une compromission physique de la machine ;
- une isolation équivalente à une VM ou à un environnement multi-tenant hostile ;
- toutes les formes de side-channel ;
- un attaquant capable de modifier arbitrairement à la fois l'application,
  l'archive et les secrets de l'utilisateur.

Si un profil renforcé devient nécessaire pour un usage partagé, automatisé ou
hostile, il devra être défini séparément plutôt que d'alourdir silencieusement
le profil local.

## 4. Autorité

Le contenu d'un email n'est jamais une autorité de sécurité.

En particulier, les éléments suivants ne peuvent pas, par leur simple présence,
autoriser une mutation ou établir une provenance Tier A :

- `Message-ID` MIME ;
- `From`, `To`, `Date`, `Subject` ou autres en-têtes ;
- contenu HTML ou texte ;
- nom ou type MIME d'une pièce jointe ;
- URI ou liens ;
- métadonnées déclarées à l'intérieur d'un format importé.

L'autorité opérationnelle vient du code et de la politique Memoria, des actions
explicites de l'utilisateur et, pour certaines opérations de source, d'une
configuration/credential validé séparément.

Une source ou un module d'acquisition peut attester uniquement les faits qu'il
est effectivement en mesure d'observer ou de vérifier. Les catégories de
provenance sont décrites dans [`ARCHITECTURE.md`](ARCHITECTURE.md).

## 5. Réseau et comptes externes

L'accès réseau et l'autorité de mutation d'un compte externe sont deux
capacités distinctes.

Le connecteur Gmail actuel utilise uniquement `gmail.readonly`. Il ne doit pas
obtenir implicitement la capacité de supprimer, modifier, labelliser, envoyer,
insérer ou réimporter des messages.

Les chemins IMAP actuels sont également read-only pour l'acquisition et le
recovery décrits par le produit.

Un futur connecteur d'écriture, de restauration ou de migration devra être une
capacité explicitement séparée :

- configuration distincte ;
- UI/action explicite ;
- tests et audit propres ;
- credentials/scopes adaptés ;
- absence de réutilisation implicite du contrat read-only existant.

L'activation du réseau ne doit pas, à elle seule, rendre accessibles des secrets
sans rapport avec la source concernée.

## 6. Credentials et données privées

Les credentials doivent rester hors de l'archive et du dépôt source.

Memoria ne doit pas :

- inclure volontairement tokens ou secrets dans les RAW, exports, index ou logs ;
- imprimer les secrets dans les diagnostics ;
- conserver des credentials dans une représentation dérivée destinée à la
  recherche ;
- copier des secrets dans un corpus de test ;
- traiter un chemin fourni par un message comme une autorisation de lire un
  fichier local.

L'accès à un fichier local hors archive doit provenir d'une action utilisateur
ou d'un contrat explicitement configuré.

## 7. HTML et contenu actif

Le rendu d'un email ne transforme jamais le contenu HTML archivé en contenu de
confiance.

Le modèle de sécurité du rendu HTML impose par défaut :

- neutralisation des scripts et handlers actifs ;
- neutralisation des formulaires, iframes, objets et mécanismes actifs
  équivalents ;
- CSP restrictive ;
- absence de chargement HTTP/HTTPS automatique ;
- ressources `cid:` servies uniquement à partir du message local concerné ;
- serveur local éphémère lié à `127.0.0.1`, avec routes non prédictibles et
  sessions bornées ;
- aucune autorité Tier A accordée au navigateur ou au contenu rendu.

L'ouverture explicite d'un lien externe par l'utilisateur est une action
distincte du rendu automatique du message.

## 8. Pièces jointes et outils externes

Une pièce jointe est une donnée non fiable.

L'ouverture ou l'enregistrement d'une pièce jointe doit résulter d'une action
explicite de l'utilisateur. Memoria ne doit pas exécuter implicitement une pièce
jointe du seul fait de l'affichage ou de l'indexation d'un message.

Les fichiers temporaires d'extraction doivent :

- être créés dans un espace privé approprié ;
- utiliser des noms assainis sans faire confiance au chemin MIME ;
- ne pas modifier l'archive autoritative ;
- avoir un cycle de vie explicite.

Les extracteurs de texte, systèmes de thumbnails, IFilter, `pdftotext` et futurs
providers externes sont des traitements Tier B/C. Leur défaillance ne doit pas
modifier le RAW ni devenir une preuve Tier A.

Lorsqu'un processus externe est supervisable, Memoria devrait borner sa durée,
sa concurrence et les ressources qui lui sont fournies lorsque le coût reste
proportionné au risque.

## 9. Parsing hostile et contrôle des ressources

Les parseurs Tier B doivent supposer des entrées malformées ou hostiles.

Les budgets pertinents peuvent porter sur :

- taille de l'entrée ;
- taille cumulée décodée ;
- nombre de parties ;
- profondeur d'imbrication ;
- nombre d'objets ;
- taille des résultats ;
- nombre de traitements externes simultanés.

Une limite doit être placée le plus tôt possible lorsqu'elle évite une
allocation ou un travail disproportionné contrôlé par une donnée non fiable.

La sécurité ne requiert pas de faux timeout pour un traitement in-process qui
ne peut pas être interrompu proprement ; dans ce cas le mécanisme doit être
coopératif ou le traitement isolé si le risque le justifie.

## 10. Archive, intégrité et adversaire local

Les checksums, BLAKE3, inventaires physiques, CAS catalogue et mécanismes de
recovery servent principalement à l'assurance de conservation, à la détection
de corruption et à la reproductibilité des preuves.

Ils ne constituent pas, à eux seuls, une authentification cryptographique de
l'archive contre un attaquant local capable de modifier toutes les données de
Memoria.

Une future garantie d'intégrité adversariale devra définir séparément :

- l'adversaire ;
- les clés ou racines de confiance ;
- la portée de la signature/authentification ;
- la gestion des clés et sauvegardes ;
- les coûts opérationnels.

Elle ne doit pas être déduite des mécanismes actuels de fidélité.

## 11. Recovery et opérations destructrices

Le recovery manipule des données Tier A et peut devenir destructif.

Une opération destructive ou de relink doit donc être :

- explicitement demandée ;
- fondée sur les preuves requises par [`RECOVERY.md`](RECOVERY.md) ;
- exécutée sous l'autorité single-writer lorsqu'elle modifie l'archive ;
- refusée lorsque l'ambiguïté porte sur l'identité ou l'autorité ;
- séparée des simples opérations d'inventaire et diagnostic.

Le contenu MIME, un index dérivé ou une ressemblance heuristique ne peut pas
augmenter l'autorité d'une opération de recovery.

## 12. Logs et diagnostics

Les diagnostics doivent privilégier les faits nécessaires :

- type d'erreur ;
- identités opaques ou non secrètes pertinentes ;
- coordonnées et digests nécessaires à l'intégrité ;
- versions et providers ;
- état de l'opération.

Ils doivent éviter la collecte systématique de contenu de message ou de
credentials.

Les logs bruts provenant d'outils externes peuvent contenir des données privées ;
leur conservation doit rester proportionnée à leur utilité de diagnostic.

## 13. Distribution et dépendances

Memoria est actuellement un produit en développement, sans installeur ou chaîne
de signature stabilisée.

Les mécanismes futurs de packaging devront distinguer :

- intégrité de distribution ;
- reproductibilité du build ;
- confiance dans les dépendances ;
- sécurité d'exécution des contenus archivés.

Une mesure de supply-chain ne doit pas être présentée comme une protection du
RAW si elle ne fournit pas cette propriété.

## 14. Budget de complexité

Toute mesure de sécurité significative devrait pouvoir répondre à cinq
questions :

1. quel actif protège-t-elle ?
2. contre quelle menace crédible et incluse ?
3. quelle propriété fournit-elle ?
4. quel coût ajoute-t-elle au code, au déploiement et au diagnostic ?
5. quel test démontre cette propriété ?

Un mécanisme sans menace crédible devrait normalement être simplifié, supprimé,
reclassé comme assurance/reproductibilité, ou réservé à un futur profil renforcé.

## 15. Relation avec les autres documents

- [`ARCHITECTURE.md`](ARCHITECTURE.md) définit les frontières conceptuelles et
  les classes de provenance.
- [`ASSURANCE.md`](ASSURANCE.md) définit la criticité A/B/C, les invariants de
  conservation et les exigences de preuve.
- [`RECOVERY.md`](RECOVERY.md) décrit le sous-système de recovery et ses actions
  admissibles.
- [`ROADMAP.md`](ROADMAP.md) priorise les travaux futurs.
- [`AGENTS.md`](AGENTS.md) décrit la méthode de travail ; ses instructions ne
  remplacent aucune frontière de sécurité.
