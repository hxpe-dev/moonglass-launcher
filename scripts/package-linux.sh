#!/usr/bin/env bash
# Packages a release build of Moonglass for Linux (spec 4.5): a tarball and
# an AppImage, in dist/.
#
#   cargo build --release -p moonglass-launcher
#   scripts/third-party-notices.sh     (needs cargo-about)
#   scripts/package-linux.sh
#
# Both carry the license and the third-party notices. Without the notices
# they are made anyway, with a warning; on CI (CI set) that is an error.
#
# Needs rsvg-convert (librsvg2-bin) for the icon and appimagetool for the
# AppImage; APPIMAGETOOL may point at it. Without appimagetool only the
# tarball is made.
set -euo pipefail

cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
name="moonglass-${version}-linux-x86_64"
bin=target/release/moonglass
[ -x "$bin" ] || { echo "build first: cargo build --release -p moonglass-launcher" >&2; exit 1; }
notices=dist/THIRD-PARTY-NOTICES.html
docs=(README.md LICENSE)
if [ -f "$notices" ]; then
    docs+=("$notices")
elif [ -n "${CI:-}" ]; then
    echo "third-party notices missing: run scripts/third-party-notices.sh" >&2
    exit 1
else
    echo "warning: third-party notices missing: run scripts/third-party-notices.sh" >&2
fi

rm -rf dist/staging
mkdir -p dist/staging
icon=dist/staging/moonglass.png
rsvg-convert -w 256 -h 256 assets/logo.svg -o "$icon"

# Tarball: the program, its desktop entry and icon, and the readme.
tar_dir="dist/staging/$name"
mkdir -p "$tar_dir"
install -m 755 "$bin" "$tar_dir/moonglass"
install -m 644 packaging/linux/moonglass.desktop "${docs[@]}" "$icon" "$tar_dir/"
tar -C dist/staging -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"

# AppImage.
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [ -z "$tool" ]; then
    echo "appimagetool not found: AppImage skipped" >&2
    exit 0
fi
app=dist/staging/Moonglass.AppDir
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/256x256/apps"
install -m 755 "$bin" "$app/usr/bin/moonglass"
install -m 644 packaging/linux/moonglass.desktop "$app/usr/share/applications/"
install -m 644 "$icon" "$app/usr/share/icons/hicolor/256x256/apps/moonglass.png"
install -D -m 644 -t "$app/usr/share/doc/moonglass" "${docs[@]}"
# appimagetool wants the entry and icon at the top as well.
cp packaging/linux/moonglass.desktop "$app/"
cp "$icon" "$app/moonglass.png"
cat > "$app/AppRun" <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/moonglass" "$@"
EOF
chmod 755 "$app/AppRun"
# Runs without FUSE (CI containers).
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" "$app" "dist/$name.AppImage"
echo "dist/$name.AppImage"
rm -rf dist/staging
