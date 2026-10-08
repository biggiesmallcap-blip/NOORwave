//! Artist name keys and the Last.fm -> TIDAL name resolution ledger.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Failed name lookups wait this long before another TIDAL search.
pub const RESOLUTION_RETRY_DAYS: i64 = 7;

/// Fold an artist name into a comparison key: accents stripped, case folded,
/// `&` and `+` read as "and", punctuation dropped, a leading "the" ignored.
pub fn name_key(name: &str) -> String {
    let mut folded = String::with_capacity(name.len());
    for c in name
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
    {
        match c {
            '&' | '+' => folded.push_str(" and "),
            c if c.is_alphanumeric() => folded.push(c),
            _ => folded.push(' '),
        }
    }
    let mut words: Vec<&str> = folded.split_whitespace().collect();
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    words.join(" ")
}

/// The candidate whose name folds to the same key, if any.
pub fn best_match(name: &str, candidates: &[(i64, String)]) -> Option<i64> {
    let key = name_key(name);
    if key.is_empty() {
        return None;
    }
    candidates
        .iter()
        .find(|(id, candidate)| *id > 0 && name_key(candidate) == key)
        .map(|(id, _)| *id)
}

/// A TIDAL id for this name from data already on disk: the discovery ledger
/// first (it holds every harvested artist), then the library.
pub fn find_local(conn: &Connection, name: &str) -> Result<Option<i64>> {
    let key = name_key(name);
    if key.is_empty() {
        return Ok(None);
    }
    let ledger = conn
        .query_row(
            "SELECT artist_tidal_id FROM video_artist_state
              WHERE name_key = ?1 AND artist_tidal_id > 0
              ORDER BY seen_main DESC, popularity DESC, artist_tidal_id LIMIT 1",
            [&key],
            |row| row.get(0),
        )
        .optional()?;
    if ledger.is_some() {
        return Ok(ledger);
    }
    Ok(conn
        .query_row(
            "SELECT tidal_id FROM artists WHERE name = ?1 COLLATE NOCASE AND tidal_id > 0 LIMIT 1",
            [name],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn resolution_due(conn: &Connection, name: &str) -> Result<bool> {
    Ok(!conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM video_artist_resolution_failures
          WHERE name = ?1 AND attempted_at > datetime('now', ?2))",
        params![name, format!("-{RESOLUTION_RETRY_DAYS} days")],
        |row| row.get::<_, bool>(0),
    )?)
}

pub fn mark_resolution_failed(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO video_artist_resolution_failures (name) VALUES (?1)
         ON CONFLICT(name) DO UPDATE SET attempted_at = datetime('now')",
        [name],
    )?;
    Ok(())
}

/// Give every library artist with a TIDAL id a ledger row and a name key, and
/// key any harvested row that arrived without one. Idempotent.
pub fn bootstrap_library_names(conn: &Connection) -> Result<usize> {
    let collect = |sql: &str| -> Result<Vec<(i64, String)>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    };
    let library = collect(
        "SELECT tidal_id, MIN(name) FROM artists
          WHERE tidal_id > 0 AND name <> '' GROUP BY tidal_id",
    )?;
    let unkeyed = collect(
        "SELECT artist_tidal_id, name FROM video_artist_state
          WHERE name_key = '' AND name <> ''",
    )?;
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0;
    for (id, name) in library.iter().chain(unkeyed.iter()) {
        changed += tx.execute(
            "INSERT INTO video_artist_state (artist_tidal_id, name, name_key) VALUES (?1, ?2, ?3)
             ON CONFLICT(artist_tidal_id) DO UPDATE SET
               name = CASE WHEN video_artist_state.name = '' THEN excluded.name
                           ELSE video_artist_state.name END,
               name_key = CASE WHEN video_artist_state.name_key = '' THEN excluded.name_key
                               ELSE video_artist_state.name_key END
             WHERE video_artist_state.name = '' OR video_artist_state.name_key = ''",
            params![id, name, name_key(name)],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn name_keys_fold_accents_ampersands_and_a_leading_the() {
        assert_eq!(name_key("Beyonc\u{e9}"), "beyonce");
        assert_eq!(
            name_key("Simon & Garfunkel"),
            name_key("Simon and Garfunkel")
        );
        assert_eq!(name_key("The Beatles"), "beatles");
        assert_eq!(name_key("The The"), "the");
        assert_eq!(name_key("AC/DC"), "ac dc");
        assert_eq!(name_key("  Sigur R\u{f3}s!! "), "sigur ros");
    }

    #[test]
    fn best_match_compares_folded_names() {
        let found = [
            (1, "Beyonc\u{e9} Tribute".to_string()),
            (2, "Beyonc\u{e9}".to_string()),
        ];
        assert_eq!(best_match("Beyonce", &found), Some(2));
        assert_eq!(best_match("Nobody", &[(1, "Somebody".into())]), None);
    }

    #[test]
    fn find_local_prefers_the_ledger_key_then_the_library() {
        let conn = conn();
        conn.execute(
            "INSERT INTO video_artist_state (artist_tidal_id, name, name_key, seen_main)
             VALUES (7, 'Beyonce', 'beyonce', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO artists (name, tidal_id) VALUES ('Mazzy Star', 55)",
            [],
        )
        .unwrap();
        assert_eq!(find_local(&conn, "BEYONC\u{c9}").unwrap(), Some(7));
        assert_eq!(find_local(&conn, "mazzy star").unwrap(), Some(55));
        assert_eq!(find_local(&conn, "Unknown").unwrap(), None);
    }

    #[test]
    fn failed_resolutions_wait_seven_days() {
        let conn = conn();
        mark_resolution_failed(&conn, "Ghost").unwrap();
        assert!(!resolution_due(&conn, "Ghost").unwrap());
        conn.execute(
            "UPDATE video_artist_resolution_failures SET attempted_at = datetime('now', '-6 days')",
            [],
        )
        .unwrap();
        assert!(!resolution_due(&conn, "Ghost").unwrap());
        conn.execute(
            "UPDATE video_artist_resolution_failures SET attempted_at = datetime('now', '-8 days')",
            [],
        )
        .unwrap();
        assert!(resolution_due(&conn, "Ghost").unwrap());
    }

    #[test]
    fn bootstrap_keys_library_artists_once() {
        let conn = conn();
        conn.execute(
            "INSERT INTO artists (name, tidal_id) VALUES ('Sigur R\u{f3}s', 90)",
            [],
        )
        .unwrap();
        assert_eq!(bootstrap_library_names(&conn).unwrap(), 1);
        let key: String = conn
            .query_row(
                "SELECT name_key FROM video_artist_state WHERE artist_tidal_id = 90",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(key, "sigur ros");
        assert_eq!(bootstrap_library_names(&conn).unwrap(), 0);
    }
}
