#!/bin/sh
# Without the app: builds the daemon and the agent CLI, installs them into ~/.local/bin (or
# $PREFIX/bin), and installs the board's Claude Code hooks and task-board plugin. For the Mac app,
# use packaging/build-app.sh instead.
set -eu
cd "$(dirname "$0")"
cargo build --release -p taskboardd -p taskboard-cli
# Board terminals share a target folder through CARGO_TARGET_DIR; cargo builds there when it's set.
target="${CARGO_TARGET_DIR:-target}"
dest="${PREFIX:-$HOME/.local}/bin"
mkdir -p "$dest"
install -m 755 "$target/release/taskboardd" "$dest/taskboardd"
install -m 755 "$target/release/tb" "$dest/tb"
echo "Installed taskboardd and tb into $dest"
"$dest/taskboardd" init
"$dest/tb" install-hooks --plugin "$PWD/plugin"
