//! Era repair: song_year from library matches, context era tags, the
//! earliest cut year and the artist active range, with source and confidence.
//!
//! Stub from the Atlas and Air scaffold; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
