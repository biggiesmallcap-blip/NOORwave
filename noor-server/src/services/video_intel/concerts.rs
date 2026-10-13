//! Concert batching: live cuts by one artist in one upload year with ids
//! close together become one concert; long-form live videos are concerts of
//! one. Writes video_concerts and video_facets.concert_id.
//!
//! Stub from the Atlas and Air scaffold; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

/// Rebuild this stage's rows. Returns the number of rows written.
pub fn build(_conn: &Connection) -> Result<usize> {
    Ok(0)
}
