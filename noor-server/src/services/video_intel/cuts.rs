//! Cut classification: the kind of every catalog video (official, live,
//! session, lyric, visualizer, remix, dance, interview, trailer, clip,
//! longform) from multilingual title markers, TIDAL type and duration, plus
//! the one shared `programmable` rule every surface obeys. Writes video_facets.
//!
//! Stub from the Atlas and Air scaffold; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
