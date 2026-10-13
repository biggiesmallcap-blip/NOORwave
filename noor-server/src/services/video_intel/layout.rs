//! Map layout: scene disks from a force layout of the scene graph, artists
//! inside their disk by rank and hashed angle, uncharted artists on an outer
//! ring, warm-started so nothing jumps between builds.
//!
//! Stub from the Atlas and Air scaffold; the atlas worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
