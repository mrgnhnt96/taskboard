# Releasing Taskboard

Taskboard ships the way Midna does: a version tag builds, signs and notarizes the app in CI,
packs it in a DMG and publishes a GitHub release. The website (`site/`, taskboard.mrgnhnt.com)
points its Download button at the newest release with a DMG, betas included.

## Cut a release

```sh
./scripts/release.sh 1.1.0          # public release, from main
./scripts/release.sh 1.2.0-beta.1   # beta (prerelease), from any branch
```

`release.sh` checks the version is newer than the last one (`scripts/newest-version.py`), writes
it into `Cargo.toml`/`Cargo.lock` (`scripts/set-version.sh`), commits, tags `vX.Y.Z` and pushes.
The tag runs `.github/workflows/release.yml` on a `macos-15` runner, which:

1. checks the files' version matches the tag
2. imports the Developer ID certificate into a throwaway keychain
3. `packaging/build-app.sh` (Developer ID signed, hardened runtime)
4. `packaging/notarize.sh` (App Store Connect API key), staples
5. packs the stapled app as `Taskboard-<version>.app.tar.gz`, for the app's updater
6. `scripts/build-dmg.sh` (branded window, `packaging/assets/dmg`), signs, notarizes and staples
   the DMG
7. publishes the GitHub release with `Taskboard-<version>-macos-arm64.dmg`, the `.app.tar.gz`
   and generated notes (betas as prereleases)

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

```sh
packaging/build-app.sh --version 1.1.0 --out dist/1.1.0   # Developer ID signed when the cert is in the keychain
packaging/notarize.sh dist/1.1.0/Taskboard.app            # OPT-IN: uploads to Apple, then staples
./scripts/build-dmg.sh dist/1.1.0/Taskboard.app Taskboard-1.1.0-macos-arm64.dmg
```

`notarize.sh` uses the keychain profile `taskboard-notary` locally
(`xcrun notarytool store-credentials taskboard-notary --apple-id … --team-id U2G2XV3688`).

## DMG window

`scripts/dmg-settings.py` places the icons; `packaging/assets/dmg/background.svg` is the
background. After changing it, run `scripts/render-dmg-background.sh` (ImageMagick) and commit
`background.tiff`.

## Website

`site/` is one static page, deployed to GitHub Pages by `.github/workflows/site.yml` on pushes
to `main` that touch it. `site/CNAME` holds the domain.
