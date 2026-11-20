#!/usr/bin/env bash
# Writes dist/THIRD-PARTY-NOTICES.html: the licenses of every library built
# into Moonglass on Windows and Linux (from cargo-about, configured in
# about.toml with the template about.hbs), plus the bundled fonts'.
#
#   cargo install cargo-about --locked
#   scripts/third-party-notices.sh
#
# The packaging scripts put the file in the zip, tarball and AppImage, and
# the release workflow attaches it to the release.
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p dist
out=dist/THIRD-PARTY-NOTICES.html
generated=$(mktemp)
fonts=$(mktemp)
trap 'rm -f "$generated" "$fonts"' EXIT

# --fail: a dependency whose license is not accepted in about.toml stops it.
cargo about generate --fail --workspace about.hbs -o "$generated"

# The fonts are not crates, so their licenses are added here.
escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' "$1"; }
for f in crates/moonglass-launcher/assets/fonts/OFL-*.txt; do
    name=$(basename "$f" .txt)
    name=${name#OFL-}
    printf '<h3>%s</h3>\n<pre>' "$name"
    escape "$f"
    printf '</pre>\n'
done > "$fonts"

awk -v fonts="$fonts" '
    /<!--FONTS-->/ { while ((getline line < fonts) > 0) print line; next }
    { print }
' "$generated" > "$out"
echo "$out"
