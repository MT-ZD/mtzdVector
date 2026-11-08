# Packaging

App id: `pro.mtzd.MtzdVector` · MIME type: `application/x-mtzdvector` (`*.mvec`)

| File | Purpose |
|---|---|
| `pro.mtzd.MtzdVector.desktop` | Launcher; handles `.mvec` (`MimeType=`) and opens every file passed (`%F`) |
| `pro.mtzd.MtzdVector.xml` | shared-mime-info definition of `.mvec` (glob + `{"artboards"` magic) |
| `pro.mtzd.MtzdVector.metainfo.xml` | AppStream metadata |
| `pro.mtzd.MtzdVector.thumbnailer` | freedesktop thumbnailer template (Dolphin, GNOME Files, Nemo, Thunar…); installed as `mtzdVector.thumbnailer` |
| `flatpak/` | Flatpak manifest (preferred packaging) |

Icons are generated from `logo.svg`: `mtzd-vector --write-icons DIR` writes
`DIR/NxN/apps/pro.mtzd.MtzdVector.png`.

Thumbnails come from the app itself (CPU render of the first artboard):

    mtzd-vector --thumbnail INPUT|- OUTPUT.png|- [SIZE]

## Flatpak

    packaging/flatpak/build.sh [--run] [--clean] [--bundle]

The script vendors the crates (`cargo-sources.json`, regenerated whenever
`Cargo.lock` changes), then builds and installs or updates the app for the
current user.

Flatpak exports the desktop entry, icons, MIME type and metainfo, so `.mvec`
files open in mtzdVector. It can't export thumbnailers, so the app writes
`~/.local/share/thumbnailers/mtzdVector.thumbnailer` on the host when it
starts (`Exec=flatpak run --command=mtzd-vector pro.mtzd.MtzdVector
--thumbnail %i %o %s`), unless a native build's working entry is there. The
`/tmp` permission lets it write the PNG where the file manager asks.

CI uses `build.sh --bundle-only`, which leaves any installed copy alone.

## AppImage

    cargo build --release
    packaging/appimage/build.sh target/release/mtzd-vector mtzd-vector.AppImage

The AppImage contains the app only, so build it on an old distro: it needs
that distro's glibc or newer. On start it registers the `.AppImage` file
itself as the launcher and thumbnailer.

## Native builds

A native binary registers itself on start (and with `--install-desktop`):
desktop entry, icons, MIME type and thumbnailer in `~/.local/share`, pointing
at the running executable.

## Dolphin thumbnails (KDE)

KIO reads freedesktop `.thumbnailer` files, so Dolphin uses the same entry
as GNOME Files. If Dolphin's preview settings were saved before, tick
"mtzdVector" under Settings → Interface → Previews.
