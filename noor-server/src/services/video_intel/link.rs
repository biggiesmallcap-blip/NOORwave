//! Final link pass: copy scene ids onto songs, artist playable counts
//! and hours, channel numbers for scenes that newly qualify.
//!
//! Stub from the Atlas and Air scaffold; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
