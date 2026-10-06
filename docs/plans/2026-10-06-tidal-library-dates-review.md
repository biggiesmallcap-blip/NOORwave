# Adversarial review: TIDAL catalogue aliases and library dates

Prepared 6 October 2026. Review of commit `8f52e3be487a929b4c2e444820fac1d652d232dc`, [PR #288](https://github.com/biggiesmallcap-blip/NOORwave/pull/288), and the subsequent date-reset investigation. This review made no application-code changes, live database repairs, process changes, or TIDAL requests. It added documentation and disposable offline probes.

**Verdict: BLOCK.** Revise the PR before merging. The [action plan](2026-10-06-tidal-library-dates-action-plan.md) defines the required changes and recovery scope.

## Scope and method

The working tree was clean, so the explicitly requested [adversarial-reviewer skill](C:/Users/Felix/.codex/skills/adversarial-reviewer/SKILL.md) selected the last commit: 20 changed files, 2,008 insertions and 288 deletions. The review followed the Saboteur, New Hire, and Security Auditor perspectives sequentially, then deduplicated findings. The skill promotes a finding caught by multiple perspectives one severity level; promotions are identified below.

Reviewed the changed code and its relevant callers, SQL, migration, tests, queue handling, favorite handlers, frontend date consumers, recovery CLI, request limiter, and server authentication boundary. The new modules and recovery scripts were read in full. Context in the very large existing routes, queries, and frontend files was reviewed around the affected paths; this is not an exhaustive audit of those entire files or the application.

Offline reproductions are in `target/catalogue-recovery/review_probes.py` and `target/catalogue-recovery/review-cc8fd3ca311845699c31943085b4b825/results.json`. The probes extract and compile the actual pure Rust matching functions and execute the actual favorite/reconciliation SQL against in-memory fixtures. Their simulated provider state is explicit; they make no network calls. Additional fixtures exercise the recovery audit/apply and the availability candidate query. These are review evidence, not permanent regression tests.

**User clarification incorporated:** An intentional unlike/re-like is expected to move a song to the top by renewing its displayed Date Added. The initial finding that this behavior itself was a bug is withdrawn. The required guard concerns automatic resync and recovery overriding a date the user deliberately chose or accepted. The older date should remain historical evidence, rather than preventing the deliberate re-like.

## Critical findings

### R1. Automatic merge or historical recovery can undo an intentional re-like

**Locations:** `noor-server/src/db/catalogue.rs:350`; `scripts/tidal_catalogue_recovery.py:127`, `scripts/tidal_catalogue_recovery.py:136`, `scripts/tidal_catalogue_recovery.py:148`; related guard in `noor-server/src/db/catalogue.rs:95`.

The explicit favorite handler intentionally renews the displayed date on a false-to-true toggle. Its `user` provenance guard protects that value from ordinary provider resync; this behavior is consistent with the user's clarified requirement. Recovery apply does not honor that guard. It revalidates ID and recording metadata, but not the audited date/provenance or any intervening user action, and unconditionally restores an earlier historical value.

The automatic merge path has a related gap: `retain_merge()` always chooses the chronological earliest date across the two rows, regardless of `user` or `recovered` provenance. Keeping the intentionally re-liked row's local ID does not keep its chosen date when the losing row carries older history. This follows directly from the unconditional `earliest()` selection and subsequent update; the full merge case was reviewed statically, while the recovery case below was reproduced.

**Reproduction:** Generate a manifest against a row dated 5 October. Before applying it, record an intentional re-like dated 6 October with provenance `user`. Applying the stale manifest succeeds, replaces that chosen date with `2020-06-10 07:00:00`, and sets provenance `recovered`. This was reproduced with the actual recovery implementation and migration on a disposable fixture.

**Required change:** Validate the current saved date, provenance, and latest local intent against the reviewed manifest before any visible-date repair. An intervening re-like invalidates that proposal. Recovery may replace a deliberate user date only when a fresh, explicit review accepts the older date. Merge must honor the current date choice and its intent evidence before considering earliest legacy/provider history. Preserve historical first-save evidence separately. Test that automatic resync/merge keeps either a deliberate newer date or an accepted recovered older date, while an explicit re-like can still renew the visible date.

**Perspectives:** Saboteur and New Hire. Base severity WARNING, promoted to CRITICAL by the skill. The demonstrated failure loses an intervening user choice during local repair; it does not make the intended re-like behavior a bug.

### R2. Version matching still collapses distinct performances and masters

**Locations:** `noor-server/src/library/duplicates.rs:306`, `noor-server/src/library/duplicates.rs:432`, `noor-server/src/library/duplicates.rs:467`, `noor-server/src/library/duplicates.rs:1610`; `noor-server/src/db/catalogue.rs:169`.

The guard compares lists of coarse keywords, then accepts equal base titles. It discards the descriptors that distinguish two versions with the same keyword. With the same artist, ISRC, and duration, the actual matcher returns `LinkAlias` for each of these fixtures:

| Existing title | Incoming title | Current decision |
| --- | --- | --- |
| `Song (2009 Remaster)` | `Song (2024 Remaster)` | Link alias |
| `Song (Live at Wembley)` | `Song (Live at Glastonbury)` | Link alias |
| `Song (Explicit)` | `Song (Clean)` | Link alias |

Provider `version` and `explicit` metadata in `TidalTrack.extra` are also absent from the matching inputs. The same coarse comparisons authorize automatic merges that delete the losing local row, combine its references, and retain only one visible recording. Keeping provider aliases does not preserve two independent library versions.

**Required change:** Compare meaningful version descriptors and available provider version/explicit metadata, rather than keyword presence alone. Known differences prohibit automatic linking, merging, or replacement. Insufficient evidence remains separate for review. Apply the same identity contract to import, merge, metadata refresh, playback selection, recovery, and album fingerprints.

**Perspectives:** Saboteur and Security Auditor. The trust boundary is provider metadata being accepted as sufficient proof for a destructive local identity operation. The fixtures prove the matcher behavior, not that these particular metadata collisions exist in the user's library. Independent recording identity and associated state are at risk when such input occurs.

### R3. An explicit unlike can be reversed by the next full sync

**Locations:** `noor-server/src/server/routes.rs:3942`, `noor-server/src/server/routes.rs:3972`, `noor-server/src/db/catalogue.rs:388`, `noor-server/src/server/routes.rs:4128`.

The track handler sets every local alias's observed favorite flag to the user's requested value, but sends a remote mutation only for the selected `tracks.tidal_id`. If aliases 101 and 202 are favorites and 202 is selected, an unlike removes only 202 remotely. A successful full snapshot still contains 101; `reconcile_favorites()` restores `tracks.is_favorite=1`. The offline SQL probe reproduced this exact state transition. Albums have the same selected-ID-only mutation problem after alias consolidation.

This also means the alias table claims all provider IDs were favorited or unfavorited even before the single request succeeds. Observed remote state and pending user intent are conflated. A sync snapshot fetched before a user action can overwrite that action; request failures have no durable intent to retry.

**Required change:** Keep observed per-alias favorite state separate from durable user intent. Explicitly unliking a logical recording must address its known remotely favorited aliases. Protect newer user actions from stale snapshots, retain failed/pending work, and apply the same contract to albums. Favorite toggles must be transactional locally. A genuine explicit re-like can renew the visible date; an automatic retry, alias reconciliation, unlike, or no-op like cannot.

**Perspectives:** Saboteur and New Hire. Base severity WARNING, promoted to CRITICAL by the skill's multiple-perspective rule. The impact is loss of the user's requested favorite state; this is not a claimed authentication vulnerability.

## Warnings

### R4. Preserved timestamps can still sort in the wrong order

**Locations:** `noor-server/src/db/queries.rs:568`, `frontend/src/routes/library/+page.svelte:1141`, `frontend/src/routes/library/+page.svelte:1366`.

`earliest()` compares instants correctly, but it preserves mixed raw string formats. The library query, client search sorting, and recent-album grouping still compare those strings. `2020-06-10T08:00:00+1000` is 22:00 UTC on 9 June, earlier than `2020-06-09 23:00:00`, yet descending string order places it first. SQL-space versus ISO-`T` formats can also misorder times on the same day.

**Required change:** Use one canonical UTC representation or an explicit chronological sort value across server and client consumers. Preserve raw historical/provider evidence separately. Cover offsets and mixed legacy formats at the displayed/sorted library boundary, not only in a helper test.

**Perspective:** Saboteur. Existing sorting behavior becomes part of the promised saved-date correctness and remains unresolved by this PR.

### R5. Availability scheduling depends on follow-up timing for fairness

**Locations:** `noor-server/src/services/tidal/catalogue.rs:49`, `noor-server/src/services/tidal/catalogue.rs:59`, `noor-server/src/main.rs:1125`.

The query puts every stale selected ID ahead of every alternative and limits the batch to 24. With 24 recordings, two aliases each, and one completed run per day, the disposable query fixture checked only the 24 selected IDs in three consecutive daily runs; it never checked an alternative. Extra sync events or successful self-triggering can advance the alternatives, but they are not a fairness guarantee. The self-emitted `LibrarySynced` event can reach the listener while the job's running flag is still held, suppressing that follow-up.

The client also uses `with_http()`'s default Interactive priority instead of the existing `for_background_work()` contract. Resolving `LOW` audio quality does not establish background request priority.

**Required change:** Give stale alternatives a fair share of the bounded budget, and schedule continuation explicitly when work remains. Prefer checking a selected release and a plausible alternative together when replacement is needed. Use background request priority and keep failures/authentication/rate limits from authorizing switches.

**Perspective:** Saboteur. The reproduction proves candidate selection under the stated daily-only schedule; it does not claim every running instance starves alternatives.

### R6. Recovery audit reports contradictory identity evidence as verified

**Locations:** `scripts/tidal_catalogue_recovery.py:76`, `scripts/tidal_catalogue_recovery.py:81`, `scripts/tidal_catalogue_recovery.py:133`.

The same-ID shortcut replaces the ISRC-filtered candidate list with a title/artist/duration match without comparing the ISRC. A fixture with one unchanged TIDAL ID but conflicting nonempty old/current ISRCs was reported as `verified`, with the reason “ISRC, artist, title and duration agree.” Apply correctly rejects that row, so this is not a demonstrated unsafe write. It is misleading review evidence and can abort an otherwise valid multi-song recovery batch.

**Required change:** Use consistent evidence checks for audit and apply. A conflicting ISRC must require review. Distinguish a unique automated match from an explicitly reviewed repair manifest, and include the actual evidence and conflicts in the report.

**Perspectives:** New Hire and Security Auditor. Base severity NOTE for a misleading audit classification with a safe apply guard; promoted to WARNING by the skill.

### R7. Frontend CI fails on the new badge styling

**Location:** `frontend/src/routes/library/+page.svelte:3596`.

The new `font-size: 0.7rem` violates the required typography-token rule in `frontend/STYLING.md`. Local `pnpm lint` reproduces the failure. [The frontend CI job](https://github.com/biggiesmallcap-blip/NOORwave/actions/runs/37387912337/job/112025782290) stops at lint, before its typecheck, test, and build steps. Earlier local check/build success therefore did not establish that all required frontend checks passed.

**Required change:** Use the existing typography token and complete the actual CI checks. Do not suppress or weaken the rule.

**Perspective:** New Hire.

### R8. A copied development library is not isolated from the user's TIDAL account

**Locations:** Operational context; `noor-server/src/services/tidal/mutations.rs:6`; startup work in `noor-server/src/main.rs:1122`.

The earlier read-only investigation established that another development copy retained a working session for the same TIDAL account. Disabling daily favorite sync did not disable startup dedupe's legacy favorite/unfavorite writes. The seven provider favorite timestamps, the development launch window, and removed alternate rows strongly implicate that process; there is no per-ID request audit proving the exact mutation sequence.

The PR removes automatic dedupe's remote mutations in this checkout. That does not constrain another already running older checkout, nor does a separate database constrain authenticated outbound writes. The isolation assumption can invalidate testing and change the real account.

**Required change:** Test with a mock provider or a dedicated test account. If production credentials are deliberately retained for read-only metadata/stream checks, enforce a default-deny guard for provider library mutations in that development run. Audit instance/account/operation/IDs and outcomes without logging credentials. Prevent competing legacy mutators during later recovery.

**Perspective:** Security Auditor. This is an existing operational risk highlighted by the investigation, not a newly introduced remote exploit or a requirement for the date fix to work.

## Notes

- Automatic local dedupe no longer needs provider favorite transfer because aliases preserve the relationship. The unused `AutoMergeStats.favorite_transfers` field and its “caller must reconcile on TIDAL” documentation should be removed or rewritten to avoid reintroducing the old behavior.
- No new SQL injection or authentication bypass was identified in the reviewed paths. The new status route inherits the existing protected router. SQL identifiers in reconciliation come from fixed internal choices; values use parameters. Recovery apply revalidates rows/alias ownership and uses a consistent SQLite backup and transactional rollback.
- Migration 069 is not yet in the installed database. Another development branch already uses that number for unrelated work. Select the next unused migration number against the eventual merge base; do not treat an unrelated development schema as an upgraded copy of this PR's schema.

## Evidence and validation

The prior implementation run passed 1,707 Rust tests with 5 existing ignored tests, 4 Python recovery tests, local frontend typecheck, and local frontend build. Rust format/test/clippy CI jobs also passed for the reviewed head. This final pass did not rerun the full suite; it ran targeted offline counterexamples and local frontend lint. Lint failed as described above; `git diff --check` passed.

The historical investigation still supports two different date-reset mechanisms: Smoko and Warrior changed TIDAL IDs; seven later songs retained the same IDs but received new provider favorite dates. These nine examples have earlier historical timestamps suitable for validating reviewed recovery. They are not the entire affected set. Spanish Flowers has no established earlier saved timestamp and is excluded from historical-date recovery.

A broader read-only inventory compared the saved pre-repair validation snapshot with both historical sources, recording favorite, playlist, and legacy library-flag evidence separately. Its report is `target/catalogue-recovery/2026-10-06-full-library-review-inventory.json`; its script is `target/catalogue-recovery/review_library_inventory.py`. Among 4,327 distinct historically favorited IDs:

- 224 historical IDs are absent as primary IDs or aliases in the comparison copy. Of these, 219 have a unique plausible current match by ISRC/title/artist/duration and 5 have no current candidate under the inventory's matching rules.
- 115 have a matched current row with a later date than its historical date.
- 27 match current rows whose track-level library and favorite flags are both off. Actual visibility also depends on album/playlist/UI rules and needs verification.
- 204 historical favorites match rows that are currently unliked. This can be intentional and is not authorization to restore likes.

These counts overlap. A missing provider ID can represent a safely consolidated duplicate; a later date can represent an intentional re-like. The wider legacy-library inventory is also affected by changes in curation classification. None of these counts is a count of confirmed losses or automatically approved repairs. The five historically liked entries without a current candidate are Lowdown (Boz Scaggs), Nobody Speak (DJ Shadow), WHATS POPPIN Remix (Jack Harlow), It Runs Through Me (Tom Misch), and i like girls (i like boys) (Conrad Taylor); their current provider availability and removal intent remain unverified.

The earlier 245 unique audit matches used a narrower audit contract and remain candidates. The revised full-library audit must classify both date regressions and missing/hidden entries, and must distinguish deliberate user actions before producing repair batches.

## Summary

The alias and date foundation is useful, but automatic merge/stale recovery can overwrite a deliberate user date, identity guards still accept distinct versions, and alias reconciliation can reverse an unlike. Preserve the intentional re-like experience while fixing automatic paths. Audit the full historical library; use the nine known examples as recovery validation, not as a cap on the repair scope.


## Implementation review after execution

The original BLOCK verdict above applies to the earlier implementation. The revised pass reviewed date ownership, alias identity, explicit favorite delivery, recovery, development isolation and patch packaging sequentially with the same three personas.

- **Saboteur:** A stale manifest originally created a full database backup on every startup/sync before rejecting its changed date. Preflight now rejects it before backup, with transaction-time revalidation still protecting concurrent changes. A late full response could also erase newer alias observations; the complete-snapshot watermark now rejects it. Tests cover stale recovery, replay after a later re-like, transactional rollback, stale snapshots, failed/reversed operations and fair availability progress.
- **New Hire:** Earlier descriptions incorrectly treated `library_added_at` as immutable first-save history and implied provider timestamps expressed deliberate intent. The implementation and plan now define it as the effective chosen date, retain prior values in audit evidence, and describe recovery candidates as unapproved. The complete audit separates five absent same-ID favorites from 88 identity-review rows that are still present.
- **Security Auditor:** A private recovery build option could have remained in the packaging shell environment after the build. The release scripts now restore the prior environment even on failure; public builds embed null. Recovery uses parameterized SQL, ownership/identity/date/revision guards and a size limit; test delivery uses no real credentials. Development mutation helpers deny writes before requests.

All earlier findings R1–R8 have corresponding code, tests or explicitly documented rollout constraints. No critical findings remain in the revised implementation. The two remaining operational notes are that older already-running binaries need their normal update/restart to acquire the guard, and that broad historical evidence cannot distinguish a deliberate external re-like/deletion from an automated change. The implementation keeps uncertain rows for review and leaves the installed library unchanged until the normal patch lifecycle.

**Revised verdict: CLEAN (operational notes retained).** Rust workspace, Python recovery and frontend checks passed; the nine-song whole-library recovery batch passed its disposable-copy test. Deployment and review of the remaining historical candidates remain separate from implementation.
