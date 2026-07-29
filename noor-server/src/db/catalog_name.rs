//! Catalogue name folding for cross-provider matching.
//!
//! Last.fm, TIDAL and the local library spell the same artist or album three
//! different ways: accents survive in one and not another, `&` and `and` swap,
//! punctuation and edition suffixes come and go. Resolution used to be a plain
//! `LOWER(name) = LOWER(?)`, so "Sigur Ros", "Beyonce" and "Tyler, The Creator"
//! simply missed and came back unresolved and unplayable.
//!
//! This is a straight port of `normalizeCatalogName` in
//! `frontend/src/lib/components/home/recommendation_navigation.ts`. The two must
//! agree: the TS copy runs at click time to pick a search result, this one runs
//! at resolve time to match a local row. If they disagree, a name resolves on
//! one side and not the other and the mismatch is invisible until a user clicks.
//! `NORMALIZE_PARITY_CASES` below is the shared table; the same inputs are
//! asserted in the TS test.

use anyhow::Result;
use rusqlite::Connection;
use unicode_normalization::UnicodeNormalization;

/// Fold a catalogue name to its comparable form.
///
/// NFKD, drop combining marks, lowercase, `&` to `and`, then collapse every
/// remaining non-alphanumeric run to a single space and trim.
///
/// Note NFKD only decomposes what has a canonical decomposition: `ö` folds to
/// `o`, but `ø`, `ß` and `æ` do not and fall through to the punctuation pass as
/// separators. That is a real limitation, not an oversight - it is also exactly
/// what the JS does, and matching behaviour matters more here than being right
/// in isolation.
pub fn normalize_catalog_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;

    for ch in value.nfkd() {
        // U+0300..=U+036F: the combining diacritical marks NFKD just split off.
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }

        if ch == '&' {
            // Substituted in place, exactly as the JS `.replace(/&/g, 'and')`
            // does, with no spacing of its own: "Simon & Garfunkel" already has
            // the spaces, and "AC&DC" deliberately folds to "acanddc" on both
            // sides rather than to "ac and dc" on one and "acanddc" on the
            // other. Parity beats prettiness here.
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push_str("and");
            continue;
        }

        let lowered = ch.to_lowercase();
        for lc in lowered {
            if lc.is_ascii_alphanumeric() {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(lc);
            } else if !out.is_empty() {
                // Any other character is a separator. Defer the space so a run
                // of punctuation collapses and a trailing run leaves nothing.
                pending_space = true;
            }
        }
    }

    out
}

/// Rows folded per pass. The work is pure CPU, but each batch takes the write
/// lock, so keep it small enough that a sweep never blocks a request path for
/// long. A cold library of ~100k tracks clears in a handful of passes.
const BACKFILL_BATCH: usize = 2_000;

/// One table's worth of backfill: which table, which source column, and where
/// the folded value goes.
const BACKFILL_TARGETS: &[(&str, &str, &str)] = &[
    ("artists", "name", "name_normalized"),
    ("albums", "title", "title_normalized"),
    ("tracks", "title", "title_normalized"),
];

/// Fold one batch of unfolded rows. Returns how many were written.
///
/// This exists because the fold is NFKD-based and SQLite cannot compute it, so
/// the column cannot be populated by the migration that adds it. A shipped app
/// cannot have its users' databases fixed by hand either, which is why this is
/// a repair pass rather than a one-off script: it runs on a schedule, chips
/// away at whatever is still NULL, and stops on its own once nothing is.
///
/// Idempotent and interruptible. A half-finished sweep just leaves NULLs, and
/// the resolvers treat NULL as "fall back to the exact match", so a partial
/// backfill is never worse than no backfill.
pub fn backfill_normalized_names(conn: &Connection, batch: usize) -> Result<usize> {
    let mut written = 0usize;

    for (table, source, target) in BACKFILL_TARGETS {
        let pending: Vec<(i64, String)> = {
            let sql = format!("SELECT id, {source} FROM {table} WHERE {target} IS NULL LIMIT ?1");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([batch as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if pending.is_empty() {
            continue;
        }

        let sql = format!("UPDATE {table} SET {target} = ?1 WHERE id = ?2");
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(&sql)?;
            for (id, value) in &pending {
                stmt.execute(rusqlite::params![normalize_catalog_name(value), id])?;
                written += 1;
            }
        }
        tx.commit()?;
    }

    Ok(written)
}

/// How many rows still need folding. Used to decide whether a sweep is worth
/// scheduling at all, and to log progress.
pub fn pending_normalized_names(conn: &Connection) -> Result<i64> {
    let mut total = 0i64;
    for (table, _, target) in BACKFILL_TARGETS {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {target} IS NULL");
        total += conn.query_row(&sql, [], |row| row.get::<_, i64>(0))?;
    }
    Ok(total)
}

/// Run passes until nothing is left to fold. Returns the total written.
///
/// Self-terminating by construction: every pass either writes rows (shrinking
/// the NULL set, since the fold of a non-null name is never null) or finds
/// none and stops. `max_passes` is belt and braces against an unforeseen row
/// that will not take a value, so a bug cannot spin forever.
pub fn run_backfill_to_completion(conn: &Connection, max_passes: usize) -> Result<usize> {
    let mut total = 0usize;
    for _ in 0..max_passes {
        let written = backfill_normalized_names(conn, BACKFILL_BATCH)?;
        if written == 0 {
            break;
        }
        total += written;
    }
    Ok(total)
}

/// Cases pinned in both languages. Keep in sync with the TS test of the same
/// name; a divergence here is the bug this module exists to prevent.
#[cfg(test)]
pub(crate) const NORMALIZE_PARITY_CASES: &[(&str, &str)] = &[
    // The failures that motivated this.
    ("Sigur Rós", "sigur ros"),
    ("Beyoncé", "beyonce"),
    ("Tyler, The Creator", "tyler the creator"),
    ("Mötley Crüe", "motley crue"),
    ("Björk", "bjork"),
    ("Sinéad O'Connor", "sinead o connor"),
    // Ampersand is substituted in place and adds no spacing of its own, so a
    // bare "AC&DC" runs together. Matches the JS.
    ("Simon & Garfunkel", "simon and garfunkel"),
    ("Kruder & Dorfmeister", "kruder and dorfmeister"),
    ("AC&DC", "acanddc"),
    ("&", "and"),
    // Punctuation runs collapse; leading and trailing ones vanish.
    ("  The   Beatles  ", "the beatles"),
    ("Godspeed You! Black Emperor", "godspeed you black emperor"),
    ("Album (Deluxe Edition)", "album deluxe edition"),
    ("!!!", ""),
    ("", ""),
    // Compatibility decomposition, which NFKD does and NFD would not.
    ("ﬁnale", "finale"),
    ("Ｍｏｏｎ", "moon"),
    // No canonical decomposition: these degrade to separators in both
    // languages. Verified against the JS, not assumed - "Agaetis" would be the
    // nicer fold but neither side produces it, and matching is what counts.
    ("Røyksopp", "r yksopp"),
    ("Ágætis byrjun", "ag tis byrjun"),
    // Digits survive.
    ("Sunn O)))", "sunn o"),
    ("2Pac", "2pac"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_frontend_normalizer() {
        for (input, expected) in NORMALIZE_PARITY_CASES {
            assert_eq!(
                normalize_catalog_name(input),
                *expected,
                "normalising {input:?}"
            );
        }
    }

    #[test]
    fn folds_names_that_differ_only_by_accent_or_ampersand() {
        assert_eq!(
            normalize_catalog_name("Sigur Rós"),
            normalize_catalog_name("Sigur Ros")
        );
        assert_eq!(
            normalize_catalog_name("Simon & Garfunkel"),
            normalize_catalog_name("Simon and Garfunkel")
        );
        assert_eq!(
            normalize_catalog_name("Tyler, The Creator"),
            normalize_catalog_name("Tyler The Creator")
        );
    }

    #[test]
    fn keeps_genuinely_different_names_apart() {
        assert_ne!(
            normalize_catalog_name("The Beatles"),
            normalize_catalog_name("The Beatles Tribute")
        );
        assert_ne!(
            normalize_catalog_name("Air"),
            normalize_catalog_name("Airs")
        );
    }

    #[test]
    fn is_idempotent() {
        for (input, _) in NORMALIZE_PARITY_CASES {
            let once = normalize_catalog_name(input);
            assert_eq!(normalize_catalog_name(&once), once, "re-folding {input:?}");
        }
    }

    fn seeded_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Sigur Rós'), (2, 'Simon & Garfunkel')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO albums (id, title, artist_id) VALUES (1, 'Ágætis byrjun', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id) VALUES (1, 'Svefn-g-englar', 1)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn backfill_folds_every_table_and_then_stops() {
        let conn = seeded_conn();
        assert_eq!(pending_normalized_names(&conn).unwrap(), 4);

        let written = run_backfill_to_completion(&conn, 8).unwrap();
        assert_eq!(written, 4);
        assert_eq!(
            pending_normalized_names(&conn).unwrap(),
            0,
            "sweep should leave nothing unfolded"
        );

        let artist: String = conn
            .query_row(
                "SELECT name_normalized FROM artists WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(artist, "sigur ros");
        let ampersand: String = conn
            .query_row(
                "SELECT name_normalized FROM artists WHERE id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ampersand, "simon and garfunkel");
        let album: String = conn
            .query_row(
                "SELECT title_normalized FROM albums WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(album, "ag tis byrjun");

        // Self-terminating: a second run finds nothing and writes nothing.
        assert_eq!(run_backfill_to_completion(&conn, 8).unwrap(), 0);
    }

    #[test]
    fn backfill_is_resumable_in_batches() {
        let conn = seeded_conn();
        // One row at a time, the way a large library gets chipped away.
        assert_eq!(backfill_normalized_names(&conn, 1).unwrap(), 3);
        assert_eq!(pending_normalized_names(&conn).unwrap(), 1);
        assert_eq!(backfill_normalized_names(&conn, 1).unwrap(), 1);
        assert_eq!(pending_normalized_names(&conn).unwrap(), 0);
    }

    #[test]
    fn backfill_only_touches_unfolded_rows() {
        let conn = seeded_conn();
        run_backfill_to_completion(&conn, 8).unwrap();
        // A name edited after the sweep is re-folded only if the column is
        // cleared, which is what the write path does.
        conn.execute(
            "UPDATE artists SET name = 'Björk', name_normalized = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        assert_eq!(backfill_normalized_names(&conn, 100).unwrap(), 1);
        let folded: String = conn
            .query_row(
                "SELECT name_normalized FROM artists WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folded, "bjork");
    }
}
