# Preserve library entries across TIDAL catalogue replacements

**Superseded behavior and rollout scope:** The [final adversarial review](2026-10-06-tidal-library-dates-review.md) found additional recovery, identity, and favorite-intent issues. Use the [revised action plan](2026-10-06-tidal-library-dates-action-plan.md) for further work. An intentional unlike/re-like should renew the displayed date and move the song to the top; automatic resync must retain the chosen or accepted recovered date. Recovery covers a full-library audit, using nine known examples to validate behavior. The implementation/validation record below describes the original committed PR, not approval to merge or repair the live library.

Prepared 6 October 2026. Prevention and recovery tooling are implemented in this checkout. The two-song recovery has been tested on a consistent copy of the installed library. The installed application and live library have not been changed.

**Problem and evidence**

NOORwave currently conflates recording identity with a preferred TIDAL catalogue ID. Import dedupe can discard an incoming ID without checking whether the existing ID is playable. Automatic merges delete losing rows and do not retain provider aliases. Full sync reconciles favorites against individual provider IDs. Although current merges preserve the earliest date, the favorite upsert can subsequently replace that date with the surviving ID's provider timestamp.

Read-only comparison of the installed database, the older portable database, and the May backup confirmed:

| Recording | Original track ID | Current track ID | Original date added | Current date added |
| --- | --- | --- | --- | --- |
| Smoko — The Chats | 122611523 | 556255961 | 10 June 2020 | 18 September 2026 |
| Warrior — Xavier Rudd | 42089470 | 544138444 | 10 June 2020 | 18 September 2026 |

Both pairs have the same ISRC, duration, and album title, with changed album IDs. Both current rows belong to resolved duplicate groups and are currently library members without a local favorite flag. Logs establish successful stream resolution for the new IDs in September. They do not establish that the old IDs were unavailable, or exactly when and why the favorite flags changed. Do not present these unknowns as confirmed TIDAL behavior.

The broader comparison found 220 old liked IDs missing locally with same-ISRC recordings under other IDs: 184 have a survivor already present in the older snapshot; 36 have only IDs absent from that snapshot. These are audit candidates, not 220 confirmed replacement failures. A current-source SQL reproduction in an in-memory database confirmed that resync can overwrite an earlier preserved date.

**Required behavior**

A catalogue replacement must retain the local song identity, original curated date, library membership, history, and playlists while allowing playback to use a verified available provider ID. Old IDs must remain resolvable as aliases. Distinct recordings and versions must remain separate. An unavailable saved entry must remain visible with its status. Ordinary sync must not silently change remote favorites to a different release.

**1. Capture regressions and contain destructive handling**

Add fixtures for the two confirmed ID pairs, the existing date-overwrite reproduction, and duplicate copies that are both playable. Cover a new ID encountered through favorites, playlist refresh, and favorite-album enrichment. First prevent cross-ID dedupe from discarding the only recorded route to the incoming release, and decouple normal dedupe from automatic TIDAL favorite/unfavorite calls. Temporary extra visible copies are preferable to losing provider identity until alias support is in place.

Main code: `library/duplicates.rs`, `services/library_dedupe.rs`, `server/routes/tidal_sync_routes.rs`, and `server/routes.rs`.

**2. Persist catalogue aliases and availability**

Add a schema migration at the next unused migration number. Persist track-provider aliases with a unique TIDAL ID, local track reference, release metadata, availability state, last-check time, and evidence/source. Backfill existing IDs as aliases. Keep `tracks.id` as the stable local identity and `tracks.tidal_id` as the selected playback ID for compatibility. Retain a merge audit record including old IDs, dates, and favorite state before deleting a redundant row.

Track alias states must distinguish unknown, available, unavailable, and transient/authentication failures. A failed request, expired session, rate limit, or empty response is not evidence that a release was withdrawn. Cache availability observations and use the existing TIDAL request limits/backoff. Inspect metadata for candidate conflicts; perform a bounded stream-resolution check when metadata is insufficient. Never stream audio merely to check identity.

**3. Reconcile recordings before choosing playback IDs**

Replace the binary insert/skip result with outcomes that retain incoming provider information: existing alias refresh, verified same-recording alias, distinct recording, and uncertain match requiring review. Apply artist, duration, and version guards even on the shared-ISRC path. Shared ISRC alone must not swallow live, remix, acoustic, edited, or incompatible-duration recordings. Title-only matches may nominate candidates but must not authorize destructive consolidation automatically.

If the retained ID is unavailable and an equivalent incoming ID is verified available, update the selected playback ID and release metadata in place. If both are available, retain the selected release unless a documented selection rule or user choice requires a switch. If availability is unknown, retain both aliases and defer switching. Keep unavailable recordings visible when no safe replacement exists.

Use one provider-ID lookup helper across sync, imports, catalog playback, and favorite handling so old aliases resolve to the same local row. Review queue provider hints, pending playback jobs, DJ media references, and caches when the selected ID changes. Preserve history and playlist references by local ID; only migrate reusable analysis when recording/version compatibility supports it.

**4. Preserve original dates and reconcile favorites carefully**

Separate the original curated library date from per-alias provider favorite timestamps and discovery import timestamps. Record date provenance. Preserve the original date during resync, provider replacement, and merge; a discovery-fill timestamp must not become the first saved date. Compare normalized timestamps chronologically, accounting for offsets and mixed legacy formats. Keep the existing Date Added label and sort backed by the protected value. Preserve explicit remove/re-add behavior as a separate user action.

Reconcile remote favorites across all aliases after a complete successful sync. Seeing a different favorite alias must not clear the recording's favorite state. An old ID disappearing from the response is insufficient on its own to distinguish an intentional unfavorite from catalogue loss. Retain the saved library entry; represent uncertain remote favorite state explicitly and honor confirmed user actions. Do not automatically re-like every historically liked recording or remove a losing favorite ID remotely. Any optional TIDAL write-back should be a separate explicit action that adds/verifies the replacement before attempting removal, with failures recorded.

**5. Handle album replacements**

Retain old/new album ID mappings and saved-album state when equivalent releases can be established. Require matching artist and a compatible ordered track/recording set; album title alone is insufficient. Keep deluxe editions, materially changed tracklists, remasters, and distinct releases separate when appropriate. Ensure album detail lookup and enrichment resolve aliases and do not repeatedly import replacements as new discovery fill. This includes reviewing album favorite reconciliation and `enrich_completed_at` behavior.

**6. Prepare historical recovery**

Build a read-only audit that emits a reviewable repair manifest: old/new IDs, recording evidence, date provenance, current state, proposed changes, and uncertainty. Start with Smoko and Warrior, whose exact 2020 timestamps survive in both older sources. Audit the 220 candidates individually; do not blanket-restore their favorite flags.

Before applying any future repair, back up the live database consistently, account for WAL state, and test the manifest on a disposable copy. The repair should be transactional, repeatable, and logged. Recover dates and aliases where evidence is strong; leave uncertain matches for review. Historical recovery must not send TIDAL mutations unless separately requested.

**7. Validate and roll out**

Required checks: original dates survive repeated full/incremental syncs; same-recording aliases create one visible library entry; old aliases still resolve; unavailable-old/available-new chooses the available ID; transient failures never cause deletion or switching; different versions remain separate; both-provider-favorite cases reconcile correctly; intentional unlikes remain honored; album replacement does not duplicate the shelf or reset enrichment incorrectly; queued playback, history, and playlist references remain valid. Exercise cancellation/restart, merge failure rollback, and repeated recovery.

Run focused Rust tests and frontend checks for any changed status/date UI, plus migration and integration tests on disposable databases. Validate both confirmed songs and a sample of broader candidates. Add concise reconciliation diagnostics with decision evidence and IDs, without logging credentials or signed stream URLs. Complete prevention first, validate the recovery manifest second, then apply a separately authorized historical repair.

**Implementation and validation**

Migration 069 adds track and album aliases, availability evidence, a protected curated date with provenance, remote favorite state, and a merge/recovery audit. The new migration and its completion marker commit atomically. Existing IDs backfill without changing favorite flags. Owner guards prevent an old alias from being inserted again under another local identity.

Import matching retains verified ISRC/artist/title/duration matches as aliases, with version and master guards. Title-only matches remain for review. Automatic dedupe no longer writes provider favorites. Playback changes only when the selected release is freshly confirmed unavailable and an equivalent alias is freshly available; unknown, timeout, authentication, and rate-limit failures do not authorize replacement. Background checks are bounded to 24 stale aliases per run. Queue imports and pending resolution recognize old aliases.

Original curated dates survive sync, alias refresh, and transactional merge. Discovery import dates remain separate. Explicit user re-adds and recovered timestamps are protected. Favorites reconcile across aliases; uncertain catalogue loss retains the local entry and state. Library rows show Unavailable or Saved locally when applicable.

Album consolidation requires a complete ordered recording fingerprint and compatible artist, title, and release type. The existing local identity and saved/enrichment state survive; the verified incoming release supplies the future provider lookup. Incomplete sets and changed tracklists remain separate.

The recovery CLI is `scripts/tidal_catalogue_recovery.py`; its audit is read-only, while apply requires an explicit verified manifest and a new SQLite snapshot backup. Apply is transactional, records before-state, handles committed WAL data, changes no favorite flags, and sends no TIDAL requests. The broader audit contains 254 candidates, 245 with unique compatible recording matches; these are still review candidates, not proof of 245 catalogue replacement failures.

The reviewed manifest is `target/catalogue-recovery/smoko-warrior.json`. On `target/catalogue-recovery/validation.db`, applying it repaired two records and repeating it repaired zero. The repaired copy retains current playable IDs and existing favorite flags while restoring:

| Recording | Restored original timestamp | Historical alias retained |
| --- | --- | --- |
| Smoko — The Chats | `2020-06-10T07:10:44.221+0000` | 122611523 |
| Warrior — Xavier Rudd | `2020-06-10T07:10:42.323+0000` | 42089470 |

Recovery unit tests cover idempotence, ambiguity rejection, all-or-nothing rollback, unchanged favorite flags, and committed WAL backup contents. Rust regressions cover both songs, mixed timestamp formats, alias ownership and changed identity, availability switching, queue/history preservation, failed merge rollback, strict album matching, migration restart, discovery dates, and confirmed versus unresolved unfavorites. Final validation: `cargo test -p noor-server --offline -- --test-threads=4` passed 1,707 tests with 5 existing ignored tests; the 4 Python recovery tests passed; `pnpm check` reported 0 errors and 0 warnings; `pnpm build` passed; `git diff --check` passed. No live TIDAL availability or mutation requests were made during validation.

**Live rollout remains**

Use the normal application build/install workflow to run this checkout's code and migration. Before a live recovery, create a consistent pre-upgrade backup, stop competing sync/repair work, re-audit the current library, review the two-song manifest against current IDs, and run the tested recovery with a fresh backup path. The installed application must use the new schema-aware code before repair. Applying the historical recovery to live data remains the separate reviewed action in step 6; no broad favorite restoration or provider mutation is part of that repair.
