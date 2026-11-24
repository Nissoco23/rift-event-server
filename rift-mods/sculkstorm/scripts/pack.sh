#!/bin/sh
# Zips the mod's resourcepack/ into dist/sculkstorm-resourcepack.zip (the repository's dist/) and
# prints the pack's SHA-1.
#
# The zip is the same bytes for the same files (fixed dates, sorted entries), so the SHA-1 only
# changes when the pack does. Players' clients check the download against it.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
repo=$(cd "$root/../.." && pwd)
out="$repo/dist/sculkstorm-resourcepack.zip"

if ! command -v zip >/dev/null 2>&1; then
    echo "pack.sh: zip is needed (it is in the macOS and most Linux base installs)." >&2
    exit 1
fi

if command -v shasum >/dev/null 2>&1; then
    sha1() { shasum -a 1 "$1" | cut -d ' ' -f 1; }
elif command -v sha1sum >/dev/null 2>&1; then
    sha1() { sha1sum "$1" | cut -d ' ' -f 1; }
else
    echo "pack.sh: neither shasum nor sha1sum was found." >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -R "$root/resourcepack/." "$work/"
find "$work" -name .DS_Store -delete
find "$work" -exec touch -t 202601010000 {} +

mkdir -p "$repo/dist"
rm -f "$out"
(cd "$work" && find . -type f | sed 's|^\./||' | LC_ALL=C sort | zip -X -q "$out" -@)

echo "Wrote $out"
echo "SHA-1: $(sha1 "$out")"
echo
echo "Host the zip where players can download it (HTTPS), then put its address and the SHA-1"
echo "in config/sculkstorm.toml, under [resource-pack]."
