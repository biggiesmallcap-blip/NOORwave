//! The census: every coverage number the design's "done when" checks, in
//! one read. Served at GET /api/videos/census.
//!
//! Stub from the Atlas and Air scaffold; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

pub fn read(_conn: &Connection) -> Result<Value> {
    Ok(json!({ "building": true }))
}
