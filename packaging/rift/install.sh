#!/bin/bash
# The Pterodactyl egg's install script (packaging/rift/pterodactyl-egg.json holds a copy: tools/make_egg.py
# writes it, so edit this file and run that). It runs once in an install container with the server folder at
# /mnt/server, at install and at reinstall, and puts the Linux x86-64 build of Rift Event Server there: the
# programs in rift/, start.sh at the top. It touches no world, no settings and no game files.
#
# RIFT_VERSION  the release to install: "nightly" (every push to dev) or a tag. The egg's variable.
# RIFT_REPO     the GitHub repository the releases are in.
# The rest is for testing the script away from a panel: RIFT_BASE_URL (where releases are), RIFT_ROOT (the
# server folder) and RIFT_SKIP_APT.
set -eu

version="${RIFT_VERSION:-nightly}"
repo="${RIFT_REPO:-Nissoco23/rift-event-server}"
base="${RIFT_BASE_URL:-https://github.com/$repo/releases/download}"
root="${RIFT_ROOT:-/mnt/server}"
asset="rift-server-linux-x86_64.tar.gz"

if [ -z "${RIFT_SKIP_APT:-}" ]; then
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates tar
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "Downloading Rift Event Server ($version)..."
curl -fsSL "$base/$version/$asset" -o "$work/$asset"
curl -fsSL "$base/$version/$asset.sha256" -o "$work/$asset.sha256"

# The download is checked against the checksum published beside it.
expected=$(cut -d' ' -f1 "$work/$asset.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$work/$asset" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$work/$asset" | cut -d' ' -f1)
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "The download does not match its checksum (expected '$expected', got '$actual'): nothing was installed." >&2
  exit 1
fi

mkdir -p "$root/rift" "$root/resources"
tar -xzf "$work/$asset" -C "$root"
chmod +x "$root/start.sh" "$root/rift/rift-server" "$root/rift/rift-engine"

echo "Installed. Upload the prepared game files to resources/ (docs/hosting.md, step 2), set the EULA"
echo "variable to true, and start the server."
