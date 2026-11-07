#!/usr/bin/env bash
# Build the Flatpak from this checkout and install (or update) it for the
# current user.
#
#   packaging/flatpak/build.sh            build + install / update
#   packaging/flatpak/build.sh --run      … then start it
#   packaging/flatpak/build.sh --clean    drop build caches first
#   packaging/flatpak/build.sh --bundle   also write mtzd-vector.flatpak (single-file bundle)
#
# Uses flatpak-builder if installed, else the org.flatpak.Builder Flatpak
# (installed automatically). Rust crates are vendored into cargo-sources.json,
# regenerated whenever Cargo.lock changes (in a private Python venv).
set -euo pipefail

APP_ID=pro.mtzd.MtzdVector
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
manifest="$here/$APP_ID.yml"
state="$here/.flatpak-builder"
build="$here/build"
repo="$here/repo"

run=0 clean=0 bundle=0
for a in "$@"; do
    case "$a" in
        --run) run=1 ;;
        --clean) clean=1 ;;
        --bundle) bundle=1 ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "unknown option: $a" >&2; exit 2 ;;
    esac
done

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

command -v flatpak >/dev/null || { echo "flatpak is not installed" >&2; exit 1; }

# Flathub for the runtime, SDK and Rust extension (user installation).
if ! flatpak --user remotes --columns=name | grep -qx flathub; then
    say "Adding the Flathub remote (user)"
    flatpak --user remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
fi

# flatpak-builder: native, else the Flatpak build of it.
if command -v flatpak-builder >/dev/null; then
    builder=(flatpak-builder)
else
    if ! flatpak info org.flatpak.Builder >/dev/null 2>&1; then
        say "flatpak-builder not found; installing org.flatpak.Builder (user)"
        flatpak --user install -y flathub org.flatpak.Builder
    fi
    builder=(flatpak run --filesystem="$root" org.flatpak.Builder)
fi

# Vendored crates, regenerated when Cargo.lock changes.
sources="$here/cargo-sources.json"
stamp="$here/.cargo-lock.sha256"
lock_sum=$(sha256sum "$root/Cargo.lock" | cut -d' ' -f1)
if [[ ! -f "$sources" || ! -f "$stamp" || "$(cat "$stamp")" != "$lock_sum" ]]; then
    say "Cargo.lock changed: regenerating cargo-sources.json"
    venv="$here/.venv"
    if [[ ! -x "$venv/bin/python" ]]; then
        python3 -m venv "$venv"
        "$venv/bin/pip" install --quiet aiohttp tomlkit
    fi
    gen="$here/.flatpak-cargo-generator.py"
    [[ -f "$gen" ]] || curl -fsSL -o "$gen" https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
    "$venv/bin/python" "$gen" "$root/Cargo.lock" -o "$sources"
    echo "$lock_sum" > "$stamp"
fi

if (( clean )); then
    say "Removing build caches"
    rm -rf "$state" "$build" "$repo"
fi

say "Building and installing $APP_ID"
args=(--user --install --install-deps-from=flathub --force-clean --ccache --state-dir="$state")
(( bundle )) && args+=(--repo="$repo")
"${builder[@]}" "${args[@]}" "$build" "$manifest"

if (( bundle )); then
    say "Writing $root/mtzd-vector.flatpak"
    flatpak build-bundle "$repo" "$root/mtzd-vector.flatpak" "$APP_ID" --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
fi

version=$(flatpak --user info "$APP_ID" 2>/dev/null | sed -n 's/^ *Version: //p' | head -1)
say "Installed $APP_ID ${version:+$version }— run with: flatpak run $APP_ID"
(( run )) && exec flatpak run "$APP_ID"
exit 0
