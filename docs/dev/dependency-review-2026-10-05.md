# Dependabot review — 5 October 2026

Reviewed the five open Dependabot PRs against `master` at `5538b16d`. Recommendations below are for the reviewed commits; recheck the head and CI before merging.

## PR decisions

| PR | Decision | Evidence |
| --- | --- | --- |
| [#241](https://github.com/biggiesmallcap-blip/NOORwave/pull/241) | Ready to merge: scripts Playwright 1.62.0 → 1.63.0 | All four CI checks passed at `1b4ef768`. Exact npm lockfile installs; a local headless Edge launch, page creation, and locator click passed. Node 24 satisfies the Node >=20 requirement. |
| [#248](https://github.com/biggiesmallcap-blip/NOORwave/pull/248) | Ready to merge: pnpm/action-setup 6.0.10 → 6.1.0 and softprops/action-gh-release 3.0.2 → 3.0.3 | All four CI checks passed at `f93509df`. SHA pins match official releases. pnpm adds v12 support while workflows retain pnpm 10; gh-release fixes malformed API error handling and updates dependencies. Release publishing itself runs only on tags. |
| [#256](https://github.com/biggiesmallcap-blip/NOORwave/pull/256) | Replace the grouped PR with the compatible subset below | Rust CI at `93e4c43a` fails with 20 compiler errors, including removed Rand traits, Symphonia decoder/buffer APIs, and Rubato SincFixedIn. |
| [#271](https://github.com/biggiesmallcap-blip/NOORwave/pull/271) | Replace the grouped PR with the compatible subset below | Frontend CI at `b43fea89` stops at ERR_PNPM_LOCKFILE_CONFIG_MISMATCH because the generated lockfile removes the manifest's security overrides. It never checks the TypeScript 7 or Vitest 5 migrations. |
| [#243](https://github.com/biggiesmallcap-blip/NOORwave/pull/243) | Close as obsolete | `promo/noorwave-showcase` was removed from master by `dc960d5c`; the PR now conflicts. Its old promo job also failed with the same override mismatch. Do not restore the removed app to accept this bump. |

## Compatible Rust updates

Keep existing Cargo manifest ranges and update only these locked packages to the reviewed versions. Cargo also resolves necessary transitive updates.

| Package | Before | After |
| --- | --- | --- |
| tokio | 1.52.3 | 1.53.1 |
| tower-http | 0.7.0 | 0.7.1 |
| rusqlite | 0.40.1 | 0.40.2 |
| reqwest | 0.13.3 | 0.13.5 |
| serde | 1.0.228 | 1.0.229 |
| serde_json | 1.0.150 | 1.0.151 |
| aes-gcm | 0.11.0 | 0.11.1 |
| md5 | 0.8.0 | 0.8.1 |
| uuid | 1.23.4 | 1.26.1 |
| thiserror | 2.0.18 | 2.0.21 |
| anyhow | 1.0.103 | 1.0.104 |
| async-trait | 0.1.89 | 0.1.92 |
| futures | 0.3.32 | 0.3.34 |
| rss | 2.1.0 | 2.1.2 |
| regex | 1.12.4 | 1.13.1 |
| id3 | 1.17.0 | 1.17.2 |
| mp3lame-encoder | 0.2.4 | 0.2.5 |
| tauri-plugin-dialog | 2.7.1 | 2.7.3 |
| tauri-plugin-updater | 2.10.1 | 2.12.0 |
| tauri-plugin-opener | 2.5.4 | 2.5.5 |

The direct dependency selections for Symphonia, Rubato, Rand, base64, dirs, tokio-tungstenite, and winreg remain unchanged. The existing cpal, wasapi, windows-sys, tauri, tauri-build, and wry locked versions remain unchanged, including their platform pins.

## Compatible frontend updates

Regenerate the lockfile with pnpm 10 and keep every existing security override. The current registry resolves some newer compatible versions than the original PR.

| Package | Before | Locked after |
| --- | --- | --- |
| @sveltejs/vite-plugin-svelte | 7.3.0 | 7.3.1 |
| @types/node | 26.2.0 | 26.6.4 |
| playwright | 1.62.1 | 1.63.0 |
| stylelint | 17.14.1 | 17.16.0 |
| svelte | 5.56.10 | 5.57.1 |
| vite | 8.2.2 | 8.3.2 |
| hls.js | 1.7.1 | 1.7.3 |
| devalue (transitive security fix) | 5.9.1 | 5.9.4 |

Updating devalue clears six current advisories affecting versions through 5.9.2. The remaining audit finding is [GHSA-vfj7-8cjw-p6xm](https://github.com/advisories/GHSA-vfj7-8cjw-p6xm) in braces 3.0.3 via stylelint → micromatch. It has no published patched version. This tree is used by repository lint tooling; do not pass untrusted glob patterns to it. This review does not claim a clean security audit.

## Deferred migrations

| Upgrade | Reason to keep separate | Required validation |
| --- | --- | --- |
| symphonia 0.5.5 → 0.6.1 | CI confirms removed decoder, probe, packet, and sample-buffer APIs | Migrate decoding/download/analysis callers; check format decoding, seek, and playback behavior |
| rubato 0.16.2 → 5.0.0 | CI confirms SincFixedIn removal; resampler migration affects audio output | Migrate both server and mix dependencies together; check resampling, flush, latency, and audible output |
| rand 0.9.4 → 0.10.2 | CI confirms RngCore, TryRngCore, and OsRng import failures | Migrate randomness APIs in authentication, pairing, database IDs, and shuffle; run their tests |
| base64 0.22.1 → 0.23.1 | Breaking 0.x minor update bundled into the failed PR | Verify OAuth/PKCE, tokens, remote authentication, and streaming manifests |
| dirs 6.0.0 → 7.0.0 | Major update affects download and user-directory discovery | Verify home/music directory resolution on Windows, macOS, and Linux |
| tokio-tungstenite 0.29.0 → 0.30.0 | Breaking 0.x minor update in test dependencies | Verify WebSocket pairing/authentication and message APIs |
| winreg 0.10.1 → 0.55.0 | Large breaking update; Linux PR CI does not exercise the Windows caller | Migrate and test the Windows startup registry path |
| typescript 6.0.3 → 7.0.2 | Compiler major bundled into a PR that never reached type checking | Verify SvelteKit, svelte-check, generated types, and production build compatibility |
| vitest and @vitest/coverage-v8 4.1.11 → 5.0.2 | Test-runner and coverage-provider major; no successful frontend validation in the original PR | Upgrade both together; run the full suite and coverage command |

## Future Dependabot grouping

Group frontend and scripts patch/minor updates; keep TypeScript majors separate. Group Vitest majors with its coverage provider so their versions stay aligned. Exclude the known breaking Cargo 0.x families from the broad Rust group, and keep major updates outside that group. Exclusions affect grouping, not whether Dependabot can propose updates; security updates remain available.

## Validation

- Frozen frontend installation succeeds with the security overrides intact.
- Frontend lint and type checking pass; svelte-check reports zero errors and warnings.
- All 157 frontend test files and 971 tests pass after correcting four existing elapsed-date fixtures that crossed Sydney's daylight-saving transition. Runtime formatter behavior is unchanged.
- The production build passes without unused-CSS or oversized-chunk warnings.
- The audit now reports one high advisory in unpatched braces; the six devalue findings are cleared.
- cargo fmt --all -- --check and git diff --check pass.
- cargo test --workspace --locked passes on Windows: 1,836 passed, zero failed, six ignored. A test-only analytics fixture now shifts local calendar time before converting to UTC, avoiding the DST mismatch; runtime analytics code is unchanged.
- cargo clippy --workspace --all-targets --locked succeeds with warnings.
- Require the replacement PR's complete Rust workspace test run and CI checks to pass before merging.

## Upstream references

- [pnpm/action-setup 6.1.0](https://github.com/pnpm/action-setup/releases/tag/v6.1.0)
- [softprops/action-gh-release 3.0.3](https://github.com/softprops/action-gh-release/releases/tag/v3.0.3)
- [Playwright 1.63.0](https://github.com/microsoft/playwright/releases/tag/v1.63.0)
- [Vite 8.3.2](https://github.com/vitejs/vite/releases/tag/v8.3.2)
- [Stylelint 17.16.0](https://github.com/stylelint/stylelint/releases/tag/17.16.0)
- [devalue 5.9.4](https://github.com/sveltejs/devalue/releases/tag/v5.9.4)
- [Dependabot grouping options](https://docs.github.com/en/code-security/reference/supply-chain-security/dependabot-options-reference#groups)
