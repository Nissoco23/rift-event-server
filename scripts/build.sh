#!/bin/sh
# Builds the Rift Event Server programs and copies them into dist/ under their Rift names.
#
#   scripts/build.sh             fast profile: quick to link, for development and testing
#   scripts/build.sh --release   release profile: thin LTO, slow to build, for production
#
# The Cargo packages keep their internal names (horizon-*), apart from rift-engine; only the copies in
# dist/ are renamed (docs/rift-naming.md): rift-server (the supervisor), rift-engine (the game engine
# with the mods of rift-mods/ linked in) and rift-prepare (prepares the game files from your own
# Minecraft copy).
set -eu

profile=fast
case "${1:-}" in
    "") ;;
    --release) profile=release ;;
    -h|--help)
        sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        echo "scripts/build.sh: unknown option $1 (try --help)" >&2
        exit 2
        ;;
esac

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# The engine is built apart, without default features (crates/rift-engine asks for none): no sound,
# no controllers and none of Horizon's extras, which a dedicated server never uses (docs/server.md).
# It is the engine's main with every mod of rift-mods/ linked in (docs/rift-mods.md); horizon-client
# stays the plain engine the harnesses run.
cargo build --profile "$profile" -p rift-engine
cargo build --profile "$profile" -p horizon-server -p horizon-prepare

mkdir -p dist
cp "target/$profile/rift-engine" dist/rift-engine
cp "target/$profile/horizon-server" dist/rift-server
cp "target/$profile/horizon-prepare" dist/rift-prepare

echo "Built dist/rift-server, dist/rift-engine and dist/rift-prepare ($profile profile)."
