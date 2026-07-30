# Release checklist

Tags drive releases. CI is in [.github/workflows/release.yml](../.github/workflows/release.yml).

## Before tagging `vX.Y.Z`

1. Bump only these: `noor-server/Cargo.toml`, `noor-app/Cargo.toml`, `noor-app/tauri.conf.json`, and the matching `noor-app` / `noor-server` entries in `Cargo.lock`.
2. **Do not run bare `cargo update`, and never run `cargo generate-lockfile`.** Both re-resolve transitive deps and can pull a pinned crate's dependency into an incompatible range. In v0.9.39, `generate-lockfile` upgraded `tauri-runtime` to 2.11.3 under the pinned `tauri =2.10.3` and broke CI with a `Send` / `Send + Sync` mismatch. In v0.1.35, a bare `cargo update` dragged Tauri 2.10.3 to 2.11.1 along for the ride and silently killed Ctrl+wheel UI zoom.

   Sync the lock with:

   ```powershell
   cargo update -p noor-server --offline
   cargo update -p noor-app --offline
   ```

   Or hand-edit the two version fields. Confirm `git diff Cargo.lock` is exactly those two lines before committing.
3. Keep both Windows artifacts: the portable zip and the NSIS setup exe.
4. Keep `installMode: "currentUser"` in the NSIS config.
5. Keep the Windows SmartScreen / Smart App Control note in the release copy.
6. Read `release.yml` before changing any release behavior.

## After CI publishes

CI only writes portable-build boilerplate, so the human changelog has to be prepended by hand. `--notes` replaces the whole body, so concatenate rather than passing only the new section:

```powershell
gh release view vX.Y.Z --json body -q .body > body.md
```

Prepend a "What's new in vX.Y.Z" section to `body.md`, then:

```powershell
gh release edit vX.Y.Z --notes-file body.md
```

## Installed-Windows release-ready means

A signed local `cargo tauri build --bundles nsis` has been tested, the `.sig` exists, and mutable data still lives under `%LOCALAPPDATA%\NOORwave`.

## If CI fails after the tag is pushed

Fix on `master`, then force-move the tag to the fix commit so the release rebuilds from corrected code:

```powershell
git tag -f vX.Y.Z <sha>
git push origin -f vX.Y.Z
```

Only safe while the failed build produced no shipped artifacts.
