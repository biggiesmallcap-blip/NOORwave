# Release checklist

Tags drive releases. CI is in [.github/workflows/release.yml](../.github/workflows/release.yml).

## Fast path for a release-only version bump

The release commit changes two Cargo manifest versions, the Tauri config version, two `Cargo.lock` package versions, and one curated notes file. The release build normally takes about 10-12 minutes after the tag is pushed. Do the short preparation promptly so the build can start.

1. Confirm the requested tag first. Check `gh auth status` and `gh secret list --app actions` for the two `TAURI_SIGNING_*` names before editing. Secret values must never be printed. Fetch and fast-forward to current `origin/master`; preserve untracked files.
2. Make the version and notes edits below. Check that `git diff Cargo.lock` contains only the two workspace version lines and run `git diff --check`. For version-only edits, use the existing PR CI instead of repeating a full local Rust suite or reinstalling frontend dependencies. Run additional local tests when product code or packaging behavior changed.
3. Push the release-preparation PR. A merged PR is not proof of green CI: this repository may merge before checks finish. Wait for the `PR check` run on the **exact master merge commit** to complete successfully.
4. Update the checkout to that master commit and run `scripts\release-preflight.ps1 -Tag vX.Y.Z`. It checks versions, notes, signing secret presence, authentication, the exact master CI result, and tag availability. Then create and push the tag. Do not spend release time trying to repair a stalled local `pnpm install` when the same locked frontend build passed in CI.
5. Watch the release workflow. The Windows job must pass NSIS signing and signature packaging; then verify the setup exe, `.sig`, `latest.json`, portable zip, other platform archives, and `sha256sums.txt` on the release. The Tauri updater signature is separate from a Windows CA code-signing certificate, so retain the SmartScreen note.

If local signing credentials are unavailable, GitHub Actions can still produce and verify the signed installer. Report the local installation and mutable-data smoke test as unverified; do not confuse its absence with missing CI signing secrets or delay an explicitly requested tag for a version-only release.

## Before tagging `vX.Y.Z`

1. Bump only these: `noor-server/Cargo.toml`, `noor-app/Cargo.toml`, `noor-app/tauri.conf.json`, and the matching `noor-app` / `noor-server` entries in `Cargo.lock`.
2. Create `docs/releases/vX.Y.Z.md` **before pushing the tag**. This is the updater's source of truth; the Windows packaging job embeds it in `latest.json` before uploading the installer.

   Start the file with the exact tag and include only user-facing changes:

   ```markdown
   ## What's new in vX.Y.Z

   ### Feature or area

   - What changed and why it matters to the listener.
   ```

   Keep download tables, installation instructions, checksums, and troubleshooting out of this file. CI fails if the file is missing, the heading does not match the tag, or the section is empty.
3. **Do not run bare `cargo update`, and never run `cargo generate-lockfile`.** Both re-resolve transitive deps and can pull a pinned crate's dependency into an incompatible range. In v0.9.39, `generate-lockfile` upgraded `tauri-runtime` to 2.11.3 under the pinned `tauri =2.10.3` and broke CI with a `Send` / `Send + Sync` mismatch. In v0.1.35, a bare `cargo update` dragged Tauri 2.10.3 to 2.11.1 along for the ride and silently killed Ctrl+wheel UI zoom.

   Sync the lock with:

   ```powershell
   cargo update -p noor-server --offline
   cargo update -p noor-app --offline
   ```

   Or hand-edit the two version fields. Confirm `git diff Cargo.lock` is exactly those two lines before committing.
4. Commit the version bump, lockfile change, and `docs/releases/vX.Y.Z.md` together. Let the PR check pass before creating and pushing the tag.
5. Keep both Windows artifacts: the portable zip and the NSIS setup exe.
6. Keep `installMode: "currentUser"` in the NSIS config.
7. Keep the Windows SmartScreen / Smart App Control note in the release copy.
8. Read `release.yml` before changing any release behavior.

## Build cache and Windows packaging

Release builds, PR checks, and cache warmers use the compiler pinned in
`rust-toolchain.toml`. When changing that pin or Rust dependencies, let a
Windows cache warmer on `master` complete before tagging; a cache saved after
the release's restore step cannot speed up that release. Manual dispatch of
`Warm build cache` remains available for release-only commits.

Windows compiles the server once and the app once through the pinned Tauri CLI
with `--no-bundle`. The portable ZIP copies those outputs before `tauri bundle`
creates the NSIS installer and updater signature. Keep that order: the bundler
patches the application with installer-specific bundle metadata. The Windows
warmer must use the same Tauri compile command and configuration as the release.

## After CI publishes

`latest.json` already contains the prepared notes at this point. Do not wait until this step to write or revise the changelog: changes made after tagging are not included in the updater manifest for that release.

Publish the same prepared notes on the GitHub release page. `--notes` replaces the whole body, so prepend the file to CI's generated body:

```powershell
$tag = "vX.Y.Z"
$curated = Get-Content "docs/releases/$tag.md" -Raw
$generated = gh release view $tag --json body --jq .body
$bodyPath = Join-Path $env:TEMP "NOORwave-$tag-release.md"
$utf8 = New-Object System.Text.UTF8Encoding $false
[System.IO.File]::WriteAllText($bodyPath, "$($curated.Trim())`n`n$($generated.Trim())`n", $utf8)
gh release edit $tag --notes-file $bodyPath
Remove-Item -LiteralPath $bodyPath
```

Finally, download `latest.json` from the release and confirm its `notes` field contains the same user-facing sections before announcing the release.

## Installed-Windows release-ready means

A signed local `cargo tauri build --bundles nsis` has been tested, the `.sig` exists, and mutable data still lives under `%LOCALAPPDATA%\NOORwave`.

## If CI fails after the tag is pushed

Fix on `master`, then force-move the tag to the fix commit so the release rebuilds from corrected code:

```powershell
git tag -f vX.Y.Z <sha>
git push origin -f vX.Y.Z
```

Only safe while the failed build produced no shipped artifacts.
