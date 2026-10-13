//! Genre spread: label propagation from tagged artists over the graph with
//! an inverse-frequency prior and hop decay. Writes video_artist_facets.genres_json.
//!
//! Stub from the Atlas and Air scaffold; the atlas worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
