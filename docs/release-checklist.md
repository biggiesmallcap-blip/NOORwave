# Release checklist

Tags drive releases. CI is in [.github/workflows/release.yml](../.github/workflows/release.yml).

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
