#!/bin/sh
# Starts rift-server in the server folder of a hosting panel (the Pterodactyl egg: packaging/rift/).
# The programs sit in ./rift/ (rift-server and rift-engine side by side), the prepared game files in
# ./resources/. The first start writes server.toml from the panel's HORIZON_* variables (rift-server init:
# HORIZON_EULA, WORLD, SEED, WHITELIST, PLAYERS, OP, ONLINE); the port always follows the panel's allocation
# (SERVER_PORT). Later starts use server.toml as it is.
cd "$(dirname "$0")" || exit 1
port="${SERVER_PORT:-25565}"
if [ ! -f server.toml ]; then
  ./rift/rift-server init --yes --port "$port" || exit 1
fi
sed "s/^port = .*/port = $port/" server.toml > server.toml.new && mv server.toml.new server.toml
exec ./rift/rift-server run
