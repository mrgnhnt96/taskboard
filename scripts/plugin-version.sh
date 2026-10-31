#!/bin/bash
# Set the Claude Code plugin's version from what it carries.
#
#   ./scripts/plugin-version.sh           write it into plugin.json
#   ./scripts/plugin-version.sh --check   fail unless plugin.json already has it
#
# The version is a hash of the plugin's tracked files (all but plugin.json), so it
# only moves when the skill, the /start command or the bin/ scripts change: a
# release that leaves them alone leaves the plugin's version alone, and Claude Code
# has nothing new to pick up. scripts/release.sh writes it before it tags, and the
# release workflow runs --check against the tag (plugin/README.md, "The version").
#
# It's semver with the hash as a prerelease tag; the h keeps an all-digit hash
# from reading as a number.
set -euo pipefail
cd "$(dirname "$0")/../plugin/task-board"

manifest=.claude-plugin/plugin.json

hash=$(git ls-files -z -- . ":!$manifest" | xargs -0 shasum -a 256 | shasum -a 256 | cut -c1-12)
version="1.0.0-h$hash"
current=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$manifest" | head -1)

if [ "${1:-}" = "--check" ]; then
  if [ "$current" != "$version" ]; then
    echo "plugin.json says $current; its files make $version. Run: ./scripts/plugin-version.sh" >&2
    exit 1
  fi
  echo "plugin $version"
  exit 0
fi

sed -i '' "s/^\( *\"version\": *\"\)[^\"]*\"/\1$version\"/" "$manifest"
echo "plugin $version"
