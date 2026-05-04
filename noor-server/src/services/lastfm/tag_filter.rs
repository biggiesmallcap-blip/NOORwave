use rusqlite::Connection;

/// Returns true when a Last.fm tag is exactly an artist already present in the
/// local library. Context/genre/noise routing is handled by `tags::context`;
/// this DB-backed check stays here because the pure classifier has no DB access.
pub fn is_artist_name_tag(tag: &str, conn: &Connection) -> bool {
    let lower = tag.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }

    conn.query_row(
        "SELECT COUNT(*) FROM artists WHERE lower(name) = ?1",
        [&lower],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}
