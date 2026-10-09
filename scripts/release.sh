#!/bin/bash
# Cut a release: set the version, commit, tag and push.
#
#   ./scripts/release.sh 1.1.0          a public release, from main
#   ./scripts/release.sh 1.2.0-beta.1   a beta, from any branch
#
# Pushing the v1.1.0 tag runs .github/workflows/release.yml, which builds,
# signs and notarizes Taskboard.app and publishes the GitHub release with the
# DMG attached. The website's Download button reads that release.
#
# A beta is published as a prerelease, which the website skips.
#
# set-version.sh also writes the version into the Claude Code plugin's
# plugin.json, so a plugin change (hooks, skill) reaches installed copies with
# the release: Claude Code keeps running its cached copy until that version
# changes (plugin/README.md, "The version").
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:-}"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]]; then
  echo "Usage: $0 <major.minor.patch[-beta.N]>" >&2
  exit 1
fi
tag="v$version"

branch=$(git rev-parse --abbrev-ref HEAD)
if [[ "$version" != *-beta.* && "$branch" != main ]]; then
  echo "Release from main (on $branch). Betas can go out from any branch." >&2
  exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
  echo "Commit or stash your changes first." >&2
  exit 1
fi
git fetch --tags origin
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "$tag already exists." >&2
  exit 1
fi
# A public release only has to be newer than the last public one, so a
# hotfix can go out while a newer beta is out. A beta has to be newer than
# everything.
tags=$(git tag --list 'v[0-9]*' | sed 's/^v//' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$' || true)
[[ "$version" == *-beta.* ]] || tags=$(echo "$tags" | grep -v -- '-beta\.' || true)
latest=
[ -z "$tags" ] || latest="v$(./scripts/newest-version.py $tags)"
if [ -n "$latest" ] && [ "$(./scripts/newest-version.py "${latest#v}" "$version")" != "$version" ]; then
  echo "$tag isn't newer than $latest." >&2
  exit 1
fi

./scripts/set-version.sh "$version"
git add Cargo.toml Cargo.lock plugin/task-board/.claude-plugin/plugin.json
git commit -m "chore: release $tag"
git tag -a "$tag" -m "Taskboard $version"
git push origin "$branch" "$tag"

echo
echo "Pushed $tag. The release workflow builds the app and publishes the release:"
echo "  https://github.com/mrgnhnt96/taskboard/actions/workflows/release.yml"
