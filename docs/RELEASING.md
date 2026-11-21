# Releasing Taskboard

Taskboard ships the way Midna does: a version tag builds, signs and notarizes the app in CI,
packs it in a DMG and publishes a GitHub release. The website (`site/`, taskboard.mrgnhnt.com)
points its Download button at the newest release with a DMG, betas included.

## Cut a release

Run it in a checkout of `main` or a `release/*` branch with nothing uncommitted:

```sh
tb release cut v1.1.0          # public release, from main
tb release cut v1.2.0-beta.1   # beta (prerelease)
```

`tb release cut` refuses any other branch, fast-forwards the checkout to origin's copy of it, then
runs the project's release script, `scripts/release.sh`, with the name. It records the release on
the board, holding every done task in Unreleased, only if the script exits 0, and prints the
release's workflow run once GitHub has started it. With no done task in Unreleased it refuses
before running the script.

You need:

- the release deploy key at `~/.ssh/taskboard-release` (or `TB_RELEASE_DEPLOY_KEY`), because
  `main` only takes PRs and the release commit is the one direct push. README.md, "Developing",
  says how to make it.
- `gh`, signed in, when a failed release is cut again (it deletes the failed run's GitHub release).

Don't run `scripts/release.sh` yourself: without `TB_RELEASE` (which `tb release cut` sets) it
stops and tells you to run `tb release cut`. `--force` runs it without the board, for when the
board is down; record the release afterwards with `tb release cut <name> --no-script`.

`release.sh` checks the version is newer than the last one (`scripts/newest-version.py`): a public
release than the last public one, a beta than every tag. It writes the version into
`Cargo.toml`/`Cargo.lock` (`scripts/set-version.sh`), sets the Claude Code plugin's version from a
hash of its files (`scripts/plugin-version.sh`), commits, tags `vX.Y.Z` and pushes the branch and
tag together.

The tag runs `.github/workflows/release.yml` on a `macos-15` runner, which:

1. checks the files' version matches the tag, and the plugin's matches its files
2. imports the Developer ID certificate into a throwaway keychain
3. `packaging/build-app.sh` (Developer ID signed, hardened runtime)
4. `packaging/notarize.sh` (App Store Connect API key), staples
5. packs the stapled app as `Taskboard-<version>.app.tar.gz`, for the app's updater
6. `scripts/build-dmg.sh` (branded window, `packaging/assets/dmg`), signs, notarizes and staples
   the DMG
7. publishes the GitHub release with `Taskboard-<version>-macos-arm64.dmg`, the `.app.tar.gz`
   and generated notes (betas as prereleases)

## When a release fails

If `release.sh` stops (wrong branch, a dirty tree, an old version, a rejected push), nothing is
recorded: fix what it says and run `tb release cut` again with the same name.

If the workflow fails after the tag is pushed, mark the release failed, then cut the same name
again:

```sh
tb release fail R12
tb release cut v1.1.0
```

`tb release fail` puts the release's tasks back in Unreleased and frees its name. The next cut of
that name sets `TB_RELEASE_REPLACE`, so `release.sh` moves the tag to `HEAD`, deletes any GitHub
release the failed run published, and force-pushes the tag, which starts the workflow again.

## Updates

The app updates itself from the release's `.app.tar.gz` (`crates/taskboard-app/src/updater.rs`):
it checks the archive against GitHub's sha256 for it, unpacks it, checks the app's signature,
notarization and team, and swaps it in. A release without the archive isn't offered as an update.
It used to install from the DMG, but mounting and unmounting a fresh DMG took about 14 seconds.

## Repository secrets and variables

The same Apple credentials as Midna's release workflow.

| Name | Kind | What |
|---|---|---|
| `DEVELOPER_ID_CERTIFICATE` | secret | base64 of the Developer ID Application `.p12` |
| `DEVELOPER_ID_CERTIFICATE_PASSWORD` | secret | its password |
| `APP_STORE_CONNECT_API_KEY` | secret | contents of the `AuthKey_<id>.p8` (notarization) |
| `DEVELOPER_ID_IDENTITY` | variable | `Developer ID Application: Morgan Hunt (U2G2XV3688)` |
| `APP_STORE_CONNECT_KEY_ID` / `APP_STORE_CONNECT_ISSUER_ID` | variable | the API key's ids |

## By hand (no CI)

The same steps as the workflow, for a build when CI can't run:

```sh
packaging/build-app.sh --version 1.1.0 --out dist/1.1.0   # Developer ID signed when the cert is in the keychain
packaging/notarize.sh dist/1.1.0/Taskboard.app            # OPT-IN: uploads to Apple, then staples
(cd dist/1.1.0 && COPYFILE_DISABLE=1 tar -czf ../../Taskboard-1.1.0.app.tar.gz Taskboard.app)
./scripts/build-dmg.sh dist/1.1.0/Taskboard.app Taskboard-1.1.0-macos-arm64.dmg
codesign --force --timestamp --sign "Developer ID Application: Morgan Hunt (U2G2XV3688)" Taskboard-1.1.0-macos-arm64.dmg
xcrun notarytool submit Taskboard-1.1.0-macos-arm64.dmg --keychain-profile taskboard-notary --wait
xcrun stapler staple Taskboard-1.1.0-macos-arm64.dmg
gh release create v1.1.0 Taskboard-1.1.0-macos-arm64.dmg Taskboard-1.1.0.app.tar.gz \
  --title "Taskboard 1.1.0" --generate-notes --latest   # a beta: --prerelease --latest=false
```

`COPYFILE_DISABLE=1` keeps AppleDouble `._` files out of the archive; they break the signature.
Build from a commit whose version files already say 1.1.0 (`scripts/set-version.sh`), or the
app reports the wrong version. Without a `v1.1.0` tag, `gh release create` tags the default
branch's latest commit. `notarize.sh` uses the
keychain profile `taskboard-notary` locally
(`xcrun notarytool store-credentials taskboard-notary --apple-id … --team-id U2G2XV3688`).

## DMG window

`scripts/dmg-settings.py` places the icons; `packaging/assets/dmg/background.svg` is the
background. After changing it, run `scripts/render-dmg-background.sh` (ImageMagick) and commit
`background.tiff`.

## Website

`site/` is one static page, deployed to GitHub Pages by `.github/workflows/site.yml` on pushes
to `main` that touch it. `site/CNAME` holds the domain.
