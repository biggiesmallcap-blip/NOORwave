# Action plan: stable library dates across TIDAL resync and replacements

Prepared 6 October 2026 after the [final adversarial review](2026-10-06-tidal-library-dates-review.md). This supersedes the behavioral choices and live-rollout scope in [the initial plan](2026-10-06-tidal-catalogue-replacements.md). PR #288 remains open. Implementation and validation are recorded below; the installed library has not been repaired directly.

## Decision

**Date Added follows the user's save intent.** First saving a recording establishes its displayed date. An intentional unlike/re-like renews that date and moves the song to the top. Automatic resync, reimport, duplicate handling, and provider-ID replacement retain the chosen date. If the user accepts an older recovered date, that accepted value is protected in the same way.

Retain historical first-save evidence separately from the current displayed date. A provider favorite timestamp describes a relationship with a particular TIDAL ID; a newer one is not evidence that the user deliberately re-added the song in NOORwave. Do not gate the user's intentional re-like behind the historical first-save date.

An explicit action in NOORwave supplies known intent. A changed timestamp observed only from TIDAL cannot, by itself, distinguish a deliberate action in another client from an automated favorite transfer. Preserve the known NOORwave date in that uncertain case; an explicitly accepted recovery/date choice supplies the missing decision.

This addresses both observed mechanisms: a recording appears under a new TIDAL ID, or the same ID is re-favorited and returns a newer provider timestamp. Turning off automatic provider favorite cleanup is not a prerequisite for date correctness. Keep the current automatic dedupe local because aliases make remote favorite transfers unnecessary; defer any optional repair of the official TIDAL collection until its behavior is explicitly defined.

| Event | Saved date | Favorite/playback behavior |
| --- | --- | --- |
| Existing ID appears in another sync | Retain | Refresh observed provider state/metadata |
| Verified equivalent new ID appears | Retain | Retain alias; choose playback only with adequate availability evidence |
| Unavailable song returns | Retain | Refresh availability; retain library identity and references |
| User unlikes a library song | Retain | Honor unlike; library entry remains |
| User intentionally re-likes that existing library song | Renew to the explicit action time | Move to the top; retain the prior date as historical evidence |
| Discovery-only row is saved for the first time | Initialize | Promote into the library; discovery import time is not the saved date |
| User accepts recovery of an evidenced older date | Restore with audit and protect from resync | Keep current favorite state and selected playable ID |
| A different version, performance, or recording appears | Separate identity/date | Do not consolidate it automatically |

An unlike alone and an idempotent already-liked request keep the current displayed date. Background favorite retries reuse the original action time and cannot generate another addition date. An intentional later save can renew the visible date without erasing historical evidence.

## 1. Correct saved-date ownership first

Addresses review R1 and R4.

- Use an explicit source/intent for date changes. Keep the favorite handler's genuine false-to-true user toggle renewing the visible saved date. Unlike, no-op like, retry, automatic alias reconciliation, and sync do not renew it. Preserve provenance on unchanged dates rather than stamping every request as a new user addition. Keep local date/favorite/intent changes in one transaction.
- Keep per-alias provider favorite timestamps separately. A later provider timestamp never advances a recorded displayed date. Retain the prior supported save date and provenance in historical evidence before an intentional re-like, so it remains available for later review without preventing the re-like.
- Retain `date_added` as the API/UI compatibility value for curated tracks, derived from the protected current saved date. `library_added_at` must be documented as that effective date if kept under its existing name; it must not be described as an immutable first-save timestamp. Use the existing audit/history mechanism for older evidence unless a separate first-save field is demonstrably needed. Discovery-only rows may retain an import timestamp internally without turning it into a saved date.
- Before repairing a displayed date, revalidate the audited date/provenance/latest user intent. Reject a stale proposal if an explicit re-like happened since the audit. A fresh, reviewed choice to accept an older date can replace the displayed date and becomes protected recovered provenance. Keep ordinary resync from overriding both deliberate newer dates and accepted older dates.
- Make automatic merge honor the current date choice, including deliberate re-like and accepted recovery. Do not select the oldest date across aliases blindly. Carry the relevant date-choice revision/time so competing user choices are resolved by intent, not by whichever timestamp is numerically earlier. Retain older evidence in the merge audit. Only use earliest supported history when no later explicit date choice governs the logical recording.
- Use a canonical UTC representation for curated dates, with raw evidence retained separately. Update server ordering and client date sorting/recent-album consumers to compare instants consistently, including legacy SQL timestamps interpreted as UTC. Preserve timestamp precision. Invalid/missing dates need an explicit fallback rather than becoming newer on every sync.
- Choose the migration sequence against the current merge base. Do not edit already-applied migrations on databases being reused for development; an unrelated branch's migration 069 is not equivalent to this PR's migration.

**Acceptance:** Same-ID resync, new-ID alias import, unavailable/return, and repeated recovery retain the chosen displayed date. An explicit unlike/re-like renews it once and moves the song up; later sync/retry does not undo or renew it again. Unlike alone and no-op like leave it alone. First save of a discovery-only track initializes a curated date. Merge respects current user intent rather than blindly choosing an older historical date. Stale recovery cannot undo an intervening user action. UI labels and ordering agree across timestamp formats and offsets.

## 2. Strengthen recording identity before automatic consolidation

Addresses R2.

- Build one small identity comparison contract used by import, local auto-merge, alias refresh, playback replacement, album fingerprints, and recovery. Share meaningful rules; the Python recovery check must have matching fixtures if it cannot share Rust code directly.
- Include full meaningful version descriptors, remaster year, named live performance/remix/cut, and available provider `version`/`explicit` metadata. Compare both track and relevant release evidence. An edition title alone neither proves a different recording nor proves equivalence.
- Keep ISRC, artist, title, duration, and version agreement as evidence. A shared ISRC never overrides a known version conflict. Distinguish missing metadata from an observed absence of a version marker.
- Treat conflicting or insufficient evidence as a separate entry/review candidate. Keep uncertain provider IDs and metadata without attaching them as playback-equivalent aliases. Do not discard incoming IDs merely because a plausible candidate exists.
- Recheck identity at replacement selection as well as at alias creation/refresh. A changed selected release must not redefine the local recording and make a previously valid alias unsafe.
- Add the three counterexamples from the review as permanent behavioral tests. Check actual import and merge outcomes, not only the keyword helper. Verify no independent version row or its references is deleted. Apply the same version evidence to complete album fingerprints.

**Acceptance:** Smoko and Warrior's established pairs still reconcile; distinct remasters, clean/explicit cuts, and named performances remain separate despite shared ISRC/duration. Unknown or changed identity cannot authorize a destructive merge or playback switch.

## 3. Honor explicit favorite intent across aliases

Addresses R3. This is required for correctness once several provider IDs represent one logical library entry.

- Store the latest requested favorite value and a revision/time independently of observed per-alias favorite flags. Persist pending targets and outcomes using a narrow data model suited to these operations; no general job framework is required.
- On a like, target the selected suitable release and update the logical favorite state immediately. Record remote observations only after requests/snapshots establish them; never mark every alias remotely favorited merely because one local button was pressed.
- On an unlike, target the recording's known remotely favorited aliases, rather than only the selected playback ID. Include the selected ID when its observed state is uncertain and the action requires it. This is a user-requested unlike, distinct from automatic duplicate cleanup. Apply the same policy to album aliases.
- Preserve local user intent across a request failure, lost connectivity, restart, or stale full-sync response. Track the snapshot's start/revision so a snapshot fetched before the action cannot undo it. Coalesce repeated/reversed actions so a delayed unlike cannot remove a later like.
- Retry within existing auth/backoff limits, with clear pending/failed outcomes and no endless tight loops. Complete reconciliation after a post-action successful snapshot or adequate request evidence. Later independent changes made in TIDAL can update observed state once they are demonstrably newer; uncertainty must not silently override the local action.
- Keep library membership independent of favorite state. Bind any renewed visible date to the genuine explicit re-like action, not to its remote confirmation or retry. If no remote session exists, retain the local action/time and represent its unsynchronized state.

**Acceptance:** With both aliases favorited, unliking removes the relevant remote favorites and remains unliked after full sync. A selected nonfavorite alias with another favorite alias also works. Test albums, failure/retry, restart, stale snapshot arrival, and rapid unlike/re-like with a mocked provider. Intentional re-like moves the song to the top once; automatic processing retains that action time.

## 4. Finish availability behavior and required frontend checks

Addresses R5 and R7.

- Retain the bounded availability budget, but allocate it fairly across selected releases and alternatives. Make continuation explicit rather than relying on a self-emitted event arriving after the running guard drops. Use the existing background-priority transport for metadata and an appropriate background transport for optional stream resolution.
- Preserve conservative switching: freshly verified unavailable selected release plus freshly verified available equivalent alias. Unknown, expired auth, timeout, rate limit, or transient asset failure cannot authorize switching. Single-alias unavailable entries should not require a duplicate to remain visible; avoid adding an unrestricted catalogue crawl.
- Retain old provider IDs, history/playlist references, and queue hints. Both available releases keep the current selection unless the user chooses a change. Use structured decision evidence without credentials or signed stream URLs.
- Replace the badge's raw font size with an existing typography token. Run `pnpm lint`, `pnpm check`, `pnpm test`, and `pnpm build`, including the CI checks for unused CSS and oversized chunks. Do not change the styling rule to accommodate the new badge.

**Acceptance:** More than 24 stale candidates make measurable progress on alternatives without interactive request contention; a busy listener cannot lose required follow-up work. Transient failures preserve the entry and its date. All required frontend CI steps pass.

## 5. Isolate development provider writes before integration testing

Addresses the existing operational risk in R8; it is independent of the date guarantee.

- Use mocked TIDAL responses for date, merge, and favorite-mutation tests. A copied database must not be treated as a copied cloud account.
- For a development run using real production credentials only to read metadata/resolve streams, default-deny provider library mutations. Enforce this at the favorite-mutation boundary, including background callers and album operations, rather than relying on a daily-sync setting or database path.
- Require deliberate configuration for a development run to perform provider writes; keep ordinary installed-app explicit favorite actions working. Do not create a new user-facing product flow for development plumbing.
- Identify competing legacy development processes before any live integration or recovery. They are not constrained by a guard added to another checkout. Isolate their provider session or arrange their stop before proceeding with work that assumes the cloud favorite state will stay stable.
- Record instance, operation, provider IDs, reason, and result so future attribution does not depend on timestamp inference. Never log access/refresh tokens, master keys, or signed stream URLs.

**Acceptance:** Startup dedupe and ordinary read-only testing issue zero favorite/unfavorite requests under the development guard. A separate test proves deliberate installed-app favorite actions still work. Test data and account isolation are recorded in the validation evidence.

## 6. Audit the full historical library and build reviewed recovery batches

Addresses R1, R6, and both date regressions and missing/hidden entries. Nine known examples are a validation set, not a limit on recovery. Apply remains a later concrete rollout action, after prevention is tested and the current rows and user intent are revalidated.

Fix audit/apply consistency first: same-ID matches must still reject conflicting recording evidence. Label automatically unique matches as candidates for explicit review, and reserve the apply manifest's reviewed status for inspected entries. Include old/current IDs, historical timestamps, identity/version evidence, provenance/source, current date/intent/flags/selected ID, proposed changes, and any conflict. A date difference alone cannot distinguish an accidental resync reset from an intentional re-like.

The read-only inventory in `target/catalogue-recovery/2026-10-06-full-library-review-inventory.json` already shows the broader scope. Comparing the saved pre-repair snapshot with both historical sources found 4,327 historically favorited IDs, with 224 historical IDs absent, 115 matched rows with later dates, and 27 matched rows with both track-level curation flags off. Of the 224 absent IDs, 219 have plausible unique recording counterparts and 5 have no current candidate under the inventory rules. These overlapping differences are investigation candidates, not confirmed losses. The older library flags also reflect different curation rules and must not authorize blanket restoration.

Expand the audit as follows:

1. Compare all available historical sources and trustworthy favorite, playlist, and library evidence against a fresh consistent snapshot. Use provider IDs/retained aliases first, then strict recording/version evidence. Keep uncertain title/artist candidates for review. Record historical duplicate resolutions and available user-action evidence.
2. Classify date changes on unchanged IDs, replacement/merged-away IDs, existing rows at risk of being hidden, and recordings with no current counterpart. Also record historical favorites now unliked, without treating that as an error. Verify actual library visibility rather than relying only on track flags.
3. Distinguish deliberate unlike/re-like/delete actions from automatic processing where evidence permits. Protect newer user-chosen dates and accepted older dates. If legacy data cannot establish intent, flag uncertainty for a reviewed choice rather than guessing.
4. Investigate the five previously liked songs currently lacking candidates and other disappearance candidates. Use bounded read-only provider metadata/availability checks when required, with version and country/availability evidence. Database absence does not prove withdrawal from TIDAL.
5. Build reviewed batches of date/alias repairs wherever evidence supports them. For genuinely lost or wrongly hidden saved entries, propose restoration of the local library identity/membership or a visible unavailable entry separately from favorite-state restoration. Do not replay historical likes or reimport intentionally deleted entries automatically.
6. Report the entire classified set, accepted repairs, conflicts, and remaining unknowns. The scope is the full library; the supported subset can be repaired in batches while uncertain cases remain visible for review.

Use these nine known examples to validate historical dates and the initial recovery behavior:

| Recording | Current TIDAL ID at investigation | Supported original timestamp |
| --- | --- | --- |
| Smoko — The Chats | 556255961 | `2020-06-10T07:10:44.221+0000` |
| Warrior — Xavier Rudd | 544138444 | `2020-06-10T07:10:42.323+0000` |
| Bitter Sweet Symphony — The Verve | 77892326 | `2020-06-10T07:10:45.095+0000` |
| Sittin' Here — Verb T | 40243728 | `2020-06-10T07:10:51.018+0000` |
| Demain, c'est loin — IAM | 138833162 | `2020-06-10T07:10:44.834+0000` |
| La donna è mobile — Plácido Domingo | 4413114 | `2020-06-10T07:10:50.934+0000` |
| Speed of Soul — Ghost Rider | 81501252 | `2024-07-22T03:55:51.884+0000` |
| Help Me — Sonny Boy Williamson II | 633221 | `2020-06-10T07:10:41.406+0000` |
| Gorgeous — slowthai | 106321251 | `2020-06-10T07:10:47.670+0000` |

Smoko and Warrior need their verified historical IDs retained as aliases as well as date restoration. The seven later entries need date restoration on the unchanged current IDs; their newer TIDAL favorite timestamps must remain provider evidence. Do not restore other removed IDs automatically without reviewing recording/version compatibility.

Exclude Spanish Flowers from historical-date repair unless earlier evidence is found. Its present history does not establish an older save. The earlier 245 unique matches and the wider inventory are inputs to the full audit, not blanket-approved changes. Dates without evidence cannot be reconstructed reliably from the newer TIDAL favorite timestamp.

Recovery procedure:

1. Regenerate a read-only audit against a consistent current snapshot and historical sources. Verify each proposed row still matches its evidence and audited date/provenance/user intent. Review favorite/library flags and selected IDs as baseline state, not values to restore indiscriminately from the older backup.
2. Build each explicit reviewed manifest, beginning with the known examples and adding other supported repairs from the full audit. Validate it on a disposable copy with the final schema-aware code. Preserve the accepted instant when normalizing its storage representation.
3. Take a consistent SQLite snapshot backup including committed WAL data. Apply locally in one transaction with before-state audit and alias ownership checks. Send no TIDAL requests.
4. Confirm the first run changes exactly the intended records and the repeat changes zero. Date/alias-only batches must leave current favorite/library flags, selected playable IDs, history, playlist membership, and queue references unchanged. Separately reviewed restoration of a missing/hidden local entry may change only the explicitly proposed membership/reference state; it must not restore remote likes by implication.
5. Run repeated sync on the repaired disposable copy with mocked provider favorites retaining the newer timestamps. Confirm accepted recovered dates survive. Then intentionally unlike/re-like a recovered song and confirm it moves to the top and stays there after sync. Test an intervening re-like after manifest creation and a late identity conflict to verify rejection/rollback and a recoverable backup.

## 7. Update the PR, validate, and roll out

The next implementation pass should revise PR #288 around the final contracts above. Resolve R1–R3 before merge; complete the other required checks and record the development isolation constraint. Remove stale favorite-transfer documentation and clearly distinguish intentional re-like dates from automatic resync dates. Update the PR description with actual passing validation, rather than the earlier partial frontend checks.

Run focused behavioral regressions first, then the required Rust workspace tests/format/lint and Python recovery tests. Complete frontend CI and migration/restart tests. Validate the known songs and each expanded recovery batch on a fresh consistent copy; do not reuse a development database carrying unrelated migration numbering. Avoid live credentials in mutation tests.

For a later live rollout, use the normal build/install path with a consistent pre-upgrade backup, ensure competing legacy mutators are isolated, run the verified final migration, re-audit and review current rows, and apply the already-tested manifest with a fresh recovery backup. Confirm dates in the library UI and after a normal sync. Date/alias recovery changes no provider favorites and does not replay historical likes.

Optional automatic repair of the official TIDAL collection remains separate. If pursued later, define when it may add a verified playable replacement, how it confirms success, and whether an old favorite should ever be removed. Its success or failure must have no effect on the recording's chosen Date Added; an explicit user re-like remains free to renew that date.


## Implementation and patch delivery

Implemented in PR #288 on `bsc/tidal-catalogue-replacements`:

- Migration 070 adds current date-choice time, version/explicit metadata, durable track/album favorite intents and per-ID operations, and a complete-snapshot watermark. It normalizes curated dates transactionally while preserving raw evidence in the audit. Master was checked again and currently ends at migration 068; this PR owns 069 and 070. Disposable validation databases are recreated from the schema-068 snapshot when the pending migration changes.
- A genuine unlike/re-like writes a new effective date once and audits the former value. Unlike, already-liked requests, retries, resync and recovered/merged aliases keep the chosen date. Merge preserves both the losing row and the kept row's prior date evidence and respects the latest date choice.
- Explicit favorite delivery serializes durable operations with bounded retries and auth recovery. Unlike covers known favorited aliases for tracks and albums; failure/restart and reversed in-flight actions are covered by a mock transport. Full snapshots fetched before a newer snapshot cannot erase newer alias observations; per-row intent also guards local favorite state.
- Import, local automatic merge, selected/alias metadata refresh, playback replacement, complete album fingerprints and reviewed recovery enforce meaningful version evidence. Shared Rust/Python identity examples cover remaster years, named live performances and clean/explicit conflicts. Missing selected-release metadata retains known version evidence rather than erasing it.
- Availability checks use selected/alternative pairs, stale-group scheduling, minute continuation and background request permits. UTC labels, client sorting, recent album consumers and server date sorting now compare compatible instants and handle invalid dates explicitly. The badge uses the existing font token; the layout contract tolerates Windows line endings.
- Development binaries deny TIDAL favorite mutations by default, including direct mutation helpers and worker auth refresh; an explicit development write override remains possible. Installed release actions retain normal behavior. This does not retrofit protection into an already-running older executable.
- Recovery manifests are version 2 and explicitly reviewed. They bind to identity, prior date/provenance, date choice and favorite-intent revision. The app preflights the entire batch before creating a backup, revalidates transactionally, preserves current membership/favorite flags and selected IDs, and uses a repair ledger so retries never undo a later re-like.
- The reviewed payload can be embedded in the server with `NOOR_CATALOGUE_RECOVERY_MANIFEST`, so an installer/updater patch carries it without a direct live-database repair. Local release scripts accept `-RecoveryManifest` and scope that environment variable to child builds. Portable patches also include a sidecar. Ordinary public/CI builds contain no private library payload. Startup and the next sync consume pending data automatically after migration.

The installed database was read only to create `target/catalogue-recovery/2026-10-06-current-snapshot.db` (45,478 tracks). No installed process was launched, no installed database was written, and no provider mutation request was sent during this implementation/validation pass.

The fresh full-library audit is `target/catalogue-recovery/2026-10-06-full-audit-v2.json`. It covers 34,648 historical IDs, including 4,327 historically favorited IDs. It identifies 114 matched historical favorites with a later displayed date, 204 matched historical favorites now unliked, and 27 matched historical favorites without current favorite/library/playlist membership flags. Those categories overlap and are evidence for review, not proof of an accidental reset or deletion. Of 93 historical favorites needing identity review, 88 still have the same provider ID locally and fail/omit required identity evidence; five have no same-ID row. None of those 93 should be described as 93 missing songs. Earlier library-flag classification explains the large incidental population; 26,258 entries are not a count of proven lost user saves.

The nine known date repairs form the first reviewed batch (`target/catalogue-recovery/tidal-catalogue-recovery.json`). Separate full-library subsets cover later favorite dates, now-unliked favorites, identity conflicts and membership review. These remain unapproved evidence; old flags alone cannot justify replaying likes, resurrecting deliberately removed entries or undoing intentional re-likes. Spanish Flowers remains excluded without earlier save evidence.

Application recovery validation uses a fresh disposable whole-library copy, applies the actual migrations and reviewed batch, and checks all track IDs, selected TIDAL IDs and favorite/library flags. It also repeats the batch and exercises sync with newer provider timestamps. The installed library receives changes only through the next normal patch/startup/sync. No live deployment or PR merge is part of this completed implementation pass.


Validation completed:

- Rust workspace tests: 1,875 passed, zero failed, seven ignored (including the separate whole-library recovery test).
- Frontend: all 996 tests in 161 files passed; lint passed; type checking reported zero errors/warnings; production build passed the CI dead-CSS/chunk-warning checks.
- Python recovery: nine tests passed, including the shared identity fixtures, same-ID ISRC conflicts, stale re-like rejection, repeat after a later re-like, ambiguity, transaction rollback and committed WAL backup.
- Rust format, workspace Clippy and diff whitespace checks passed. Clippy reports existing repository warnings; this PR does not make them fatal.
- PowerShell release/dev scripts parsed successfully.
- The whole-library application recovery test passed independently on a disposable copy: nine repairs, zero on repeat, selected IDs and favorite/library flags unchanged. The private embedded-payload delivery check is recorded in the same recovery log.
