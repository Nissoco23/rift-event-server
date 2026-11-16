#!/bin/sh
# Starts rift-server in the server folder of a hosting panel (the Pterodactyl egg: packaging/rift/).
# The programs sit in ./rift/ (rift-server and rift-engine side by side), the prepared game files in
# ./resources/. The first start writes server.toml from the panel's HORIZON_* variables (rift-server init:
# HORIZON_EULA, WORLD, SEED, WHITELIST, PLAYERS, OP, ONLINE); the port always follows the panel's allocation
# (SERVER_PORT). Later starts use server.toml as it is. The voice chat's UDP port and public address
# (VOICECHAT_PORT, VOICECHAT_HOST) are written to config/voicechat.toml at each start.
cd "$(dirname "$0")" || exit 1
port="${SERVER_PORT:-25565}"
if [ ! -f server.toml ]; then
  ./rift/rift-server init --yes --port "$port" || exit 1
fi
sed "s/^port = .*/port = $port/" server.toml > server.toml.new && mv server.toml.new server.toml
# Sets `key = value` in a config file, adding it when the file has no such line.
set_key() {
  if grep -q "^$2 = " "$1" 2>/dev/null; then
    sed "s|^$2 = .*|$2 = $3|" "$1" > "$1.new" && mv "$1.new" "$1"
  else
    echo "$2 = $3" >> "$1"
  fi
}
mkdir -p config
set_key config/voicechat.toml port "${VOICECHAT_PORT:-24454}"
if [ -n "${VOICECHAT_HOST:-}" ]; then
  set_key config/voicechat.toml voice-host "\"$VOICECHAT_HOST\""
fi
exec ./rift/rift-server run
