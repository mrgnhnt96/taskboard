#!/bin/sh
# Cargo's rustc wrapper (set in .cargo/config.toml). Routes rustc through
# sccache when it's installed, so every worktree reuses dependencies another
# worktree already compiled instead of starting from a cold target dir.
# Without sccache it runs rustc as is, so builds work on any machine.
#
# Set TASKBOARD_NO_SCCACHE=1 to skip the cache for one build.

if [ -z "$TASKBOARD_NO_SCCACHE" ] && command -v sccache >/dev/null 2>&1; then
  # sccache hashes every CARGO_* variable into its keys, and each worktree
  # sets its own CARGO_TARGET_DIR, so with it set no worktree ever hits
  # another's entries. rustc doesn't read it; cargo already passed the paths.
  unset CARGO_TARGET_DIR
  # Size of the cache when this call starts the sccache server.
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-20G}"
  exec sccache "$@"
fi
exec "$@"
