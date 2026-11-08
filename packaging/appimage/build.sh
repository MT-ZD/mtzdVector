#!/usr/bin/env bash
# Package a release binary as an AppImage.
#
#   packaging/appimage/build.sh target/release/mtzd-vector mtzd-vector.AppImage
#
# Bundles only the app: the graphics, Wayland and X11 libraries it loads at
# run time must come from the host's drivers. Build the binary on an old
# distro, since it needs that glibc or newer.
set -euo pipefail

APP_ID=pro.mtzd.MtzdVector
[[ $# -eq 2 ]] || { sed -n '2,8p' "$0"; exit 2; }
binary=$(realpath "$1")
output=$(realpath -m "$2")
here=$(cd "$(dirname "$0")" && pwd)
packaging=$(dirname "$here")
arch=$(uname -m)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/AppDir"

install -Dm755 "$binary" "$appdir/usr/bin/mtzd-vector"
install -Dm644 "$packaging/$APP_ID.desktop" -t "$appdir/usr/share/applications"
install -Dm644 "$packaging/$APP_ID.metainfo.xml" -t "$appdir/usr/share/metainfo"
install -Dm644 "$packaging/$APP_ID.xml" -t "$appdir/usr/share/mime/packages"
"$binary" --write-icons "$appdir/usr/share/icons/hicolor"
ln -s usr/bin/mtzd-vector "$appdir/AppRun"
ln -s "usr/share/applications/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
ln -s "usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$appdir/$APP_ID.png"

# Pinned tools; APPIMAGE_TOOLS can name a directory already holding them.
base=https://github.com/AppImage
case "$arch" in
    x86_64)
        tool_sum=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
        runtime_sum=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
        ;;
    aarch64)
        tool_sum=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
        runtime_sum=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
        ;;
    *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac
fetch() { # file-name url sha256
    if [[ -n "${APPIMAGE_TOOLS:-}" ]]; then
        cp "$APPIMAGE_TOOLS/$1" "$work/$1"
    else
        curl -fsSL --retry 5 --retry-all-errors -o "$work/$1" "$2"
    fi
    echo "$3  $work/$1" | sha256sum -c --quiet
}
fetch "appimagetool-$arch.AppImage" "$base/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage" "$tool_sum"
fetch "runtime-$arch" "$base/type2-runtime/releases/download/20251108/runtime-$arch" "$runtime_sum"
tool="$work/appimagetool-$arch.AppImage"
chmod +x "$tool"
# Extract instead of mounting: containers and CI have no FUSE.
ARCH=$arch APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream --runtime-file "$work/runtime-$arch" "$appdir" "$output"
