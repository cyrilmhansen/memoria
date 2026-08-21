# Probe Slint + WebView système

Ce crate est volontairement indépendant de Memoria.

Sous Linux, le build utilise les bibliothèques système GTK/WebKitGTK :

```text
PKG_CONFIG_PATH=/usr/lib/pkgconfig:/usr/share/pkgconfig \
  cargo run --release --manifest-path experiments/slint-wry-probe/Cargo.toml
```

Le `PKG_CONFIG_PATH` explicite évite que le `pkg-config` Homebrew local ne
masque les fichiers `.pc` système. Sous X11, le probe peut être lancé avec
`GDK_BACKEND=x11`. Sous Wayland, il conserve le fallback Slint et rapporte
l'impossibilité d'attacher la WebView enfant.
