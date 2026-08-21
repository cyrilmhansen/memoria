# system-thumbnail-probe

Probe isolé d’un service de miniatures sans Slint et sans moteur PDF/Office/
vidéo embarqué.

Linux utilise le cache freedesktop (`$XDG_CACHE_HOME/thumbnails` ou
`$HOME/.cache/thumbnails`) puis les fichiers `.thumbnailer` installés dans
`/usr/share/thumbnailers` et `/usr/local/share/thumbnailers`. Les providers
sont lancés comme processus enfants, sans shell, avec un timeout de 10 s.

Windows utilise `IShellItemImageFactory::GetImage` avec
`SIIGBF_THUMBNAILONLY`; l’implémentation de conversion bitmap reste un point
à valider sur Windows natif dans cette expérience.

```text
cargo run --release -- providers
cargo run --release -- thumbnail /path/to/file 256
```

Le probe ne considère jamais une icône de fichier comme une miniature valide.
`unavailable` signifie qu’aucun provider ne correspond ; `error` signifie
qu’un provider a échoué ou a produit une sortie invalide.
