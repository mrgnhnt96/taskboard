#!/bin/sh
# Without the app: builds the daemon and the agent CLI and installs them into ~/.local/bin (or
# $PREFIX/bin). For the Mac app, use packaging/build-app.sh instead.
set -eu
cd "$(dirname "$0")"
cargo build --release -p taskboardd -p taskboard-cli
dest="${PREFIX:-$HOME/.local}/bin"
mkdir -p "$dest"
install -m 755 target/release/taskboardd "$dest/taskboardd"
install -m 755 target/release/tb "$dest/tb"
echo "Installed taskboardd and tb into $dest"
"$dest/taskboardd" init
