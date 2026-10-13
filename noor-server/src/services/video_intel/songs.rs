//! Song rollup: video_facets grouped by song_key into video_songs with
//! the best cut (library_videos order: best match, then longest).
//!
//! Stub from the Atlas and Air scaffold; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
