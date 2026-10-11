# Release checklist

Tags drive releases. CI is [.github/workflows/release.yml](../.github/workflows/release.yml); its header comments cover build cache and packaging order.

## Cut the release

1. On a clean `master`, run `scripts\release-bump.ps1 -Tag vX.Y.Z`. The first run scaffolds `docs/releases/vX.Y.Z.md` from the commits since the last tag and stops.
2. Rewrite that file as listener-facing changes under `## What's new in vX.Y.Z` (no downloads, install steps or checksums; CI adds those). It feeds both the updater's `latest.json` and the release page, and is frozen once the tag is pushed.
3. Run `scripts\release-bump.ps1 -Tag vX.Y.Z -Ship`. It bumps the two Cargo manifests, `tauri.conf.json` and the two `Cargo.lock` entries (no cargo invocation), commits, pushes `master`, runs `release-preflight.ps1`, and pushes the tag. Without `-Ship` it only commits and prints the remaining commands.

Preflight checks versions, notes, `gh` auth, signing secret names, tag availability, and a green master `PR check`. A commit that only touches release files rides on its parent's green run, so there is no PR or CI wait for a version-only release. If the parent's run is still going, wait for it.

Never run bare `cargo update` or `cargo generate-lockfile` for a release: both re-resolve transitive deps and have broken Tauri twice (v0.1.35 zoom, v0.9.39 `Send + Sync`). Run extra local tests only when product code or packaging changed.

## Verify and publish the draft

CI builds every platform into a **draft** release whose body already leads with the notes. Don't edit the body: the in-app patch dialog reads it from `## What's new in` up to `Desktop hi-fi player`, so a second copy or a reflowed body shows up there. Before publishing:

1. Every platform job and `publish-checksums` succeeded.
2. Assets present: setup exe, `.sig`, `latest.json`, portable zip, macOS/Linux archives, `sha256sums.txt`.
3. The installer signature verifies against the public key in `noor-app/tauri.conf.json`, and `latest.json` `notes` matches the notes file.
4. Smoke-test the setup payload in an isolated fixture (isolated app data and WebView2 storage) so the existing install is untouched. Report the wizard, registry and uninstall flow as not exercised.

```powershell
gh release edit vX.Y.Z --draft=false --latest
```

Then confirm the latest-release endpoint and updater manifest show the new version.

## If CI fails after the tag

Fix on `master`, then move the tag (only while no artifacts have shipped):

```powershell
git tag -f -m vX.Y.Z vX.Y.Z <sha>
git push origin -f vX.Y.Z
```
