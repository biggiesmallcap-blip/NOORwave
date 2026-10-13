//! Runs every stage in order and records each in video_intel_runs.
//!
//! Scaffold from Atlas and Air; the catalog worker owns this file.

use anyhow::Result;
use rusqlite::Connection;

use super::{concerts, cuts, era, layout, link, scenes, songs, spread};

/// One stage's outcome.
#[derive(Debug, Clone)]
pub struct StageReport {
    pub stage: &'static str,
    pub rows: usize,
}

/// Rebuild everything. Order matters: songs read cuts, scenes are needed by
/// layout and link, era reads songs.
pub fn build_all(conn: &Connection) -> Result<Vec<StageReport>> {
    let stages: [(&'static str, fn(&Connection) -> Result<usize>); 8] = [
        ("cuts", cuts::build),
        ("songs", songs::build),
        ("concerts", concerts::build),
        ("scenes", scenes::build),
        ("spread", spread::build),
        ("layout", layout::build),
        ("era", era::build),
        ("link", link::build),
    ];
    let mut reports = Vec::with_capacity(stages.len());
    for (stage, run) in stages {
        let rows = run(conn)?;
        reports.push(StageReport { stage, rows });
    }
    Ok(reports)
}
