#!/bin/bash
# Cut a release: set the version, commit, tag and push.
#
#   ./scripts/release.sh 1.1.0          a public release, from main
#   ./scripts/release.sh 1.2.0-beta.1   a beta, from any branch
#   ./scripts/release.sh v1.2.0-beta.1  the same; one leading v is dropped
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
#
# tb release cut runs this with TB_RELEASE set and records the release on the
# board when it succeeds, so run that instead. --force runs it without the
# board (when the board is down).
#
# A release whose workflow failed is marked failed on the board (tb release fail
# R<n>); cutting the same name again sets TB_RELEASE_REPLACE, and this script
# then moves the tag to HEAD and drops any GitHub release the failed run left,
# instead of refusing the existing tag.
set -euo pipefail

force=
args=()
for a in "$@"; do
  if [ "$a" = --force ]; then force=1; else args+=("$a"); fi
done
set -- ${args[@]+"${args[@]}"}
if [ -z "${TB_RELEASE:-}" ] && [ -z "$force" ]; then
  v="${1:-<version>}"
  echo "Run tb release cut v${v#v} instead; it runs this script and records the release. (--force to run it without the board.)" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

# Take the tag name too, so a release name can be passed straight through.
version="${1:-}"
version="${version#v}"
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
# main only takes PRs. The release commit is the one direct push, made with a deploy key
# the repo's main-branch ruleset lets past. Set it up: README.md, "Developing".
deploy_key=${TB_RELEASE_DEPLOY_KEY:-$HOME/.ssh/taskboard-release}
if [ ! -f "$deploy_key" ]; then
  echo "No release deploy key at $deploy_key (see README.md, \"Developing\")." >&2
  exit 1
fi
push() {
  GIT_SSH_COMMAND="ssh -i $deploy_key -o IdentitiesOnly=yes" git push git@github.com:mrgnhnt96/taskboard.git "$@"
  git fetch -q origin
}
git fetch --tags origin
replace=
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  if [ -z "${TB_RELEASE_REPLACE:-}" ]; then
    echo "$tag already exists. If its release failed, mark it failed (tb release fail R<n>) and cut it again." >&2
    exit 1
  fi
  # Out of the way locally, so the newer-than check below doesn't count it.
  replace=1
  git tag -d "$tag" >/dev/null
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
# A replaced release usually has its version commit already.
git diff --cached --quiet || TB_RELEASE_COMMIT=1 git commit -m "chore: release $tag"
git tag -a "$tag" -m "Taskboard $version"
if [ -n "$replace" ]; then
  # The failed run may have published before it failed; the new run creates the release again.
  if gh release view "$tag" >/dev/null 2>&1; then
    echo "Deleting the failed run's GitHub release $tag."
    gh release delete "$tag" --yes
  fi
  push "$branch" "+refs/tags/$tag:refs/tags/$tag"
else
  push "$branch" "$tag"
fi

echo
echo "Pushed $tag. The release workflow builds the app and publishes the release."
