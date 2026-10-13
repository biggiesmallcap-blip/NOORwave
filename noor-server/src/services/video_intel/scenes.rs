//! Scene finder: Louvain communities over the weighted artist graph,
//! giants split, small ones dissolved, stable ids across rebuilds, exemplar
//! names, hue. Writes video_scenes and video_artist_facets.scene_id.
//!
//! Stub from the Atlas and Air scaffold; the atlas worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
