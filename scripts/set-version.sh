#!/bin/bash
# Set taskboard's version everywhere it's written down.
#
#   ./scripts/set-version.sh 1.1.0          write 1.1.0 into every file
#   ./scripts/set-version.sh 1.2.0-beta.1   a beta works the same way
#   ./scripts/set-version.sh --check [v]    fail unless every file has the same version (and it's v)
#
# Git tags are the source of truth: scripts/release.sh calls this before it
# tags, and the release workflow runs --check against the tag.
set -euo pipefail
cd "$(dirname "$0")/.."

crates=(taskboardd taskboard-cli taskboard-app)

versions() {
  echo "Cargo.toml $(awk '/^\[workspace.package\]/{p=1} p && /^version *=/{gsub(/[" ]/,""); sub(/version=/,""); print; exit}' Cargo.toml)"
  for crate in "${crates[@]}"; do
    echo "Cargo.lock:$crate $(grep -A1 "^name = \"$crate\"$" Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')"
  done
}

if [ "${1:-}" = "--check" ]; then
  expected="${2:-}"
  list=$(versions)
  echo "$list"
  unique=$(echo "$list" | awk '{print $2}' | sort -u)
  if [ "$(echo "$unique" | wc -l | tr -d ' ')" != 1 ]; then
    echo "Versions don't match. Run: ./scripts/set-version.sh <version>" >&2
    exit 1
  fi
  if [ -n "$expected" ] && [ "$unique" != "$expected" ]; then
    echo "Files say $unique, expected $expected." >&2
    exit 1
  fi
  exit 0
fi

version="${1:-}"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]]; then
  echo "Usage: $0 <major.minor.patch[-beta.N]> | --check [version]" >&2
  exit 1
fi

# Only the [workspace.package] version; dependency versions stay as they are.
sed -i '' "/^\[workspace.package\]/,/^\[/s/^version = \".*\"/version = \"$version\"/" Cargo.toml
for crate in "${crates[@]}"; do
  sed -i '' "/^name = \"$crate\"$/{n;s/^version = \".*\"/version = \"$version\"/;}" Cargo.lock
done

versions
