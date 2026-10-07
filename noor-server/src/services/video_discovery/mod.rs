//! Scored frontier crawler that discovers which TIDAL music videos exist.
//!
//! TIDAL has no "every music video" endpoint, so discovery fans out over an
//! artist graph. This module owns that fan-out: one ledger per artist, a
//! weighted graph, a harvest layer that learns from every video payload, a
//! scheduler that ranks jobs by expected value, and the crawler that spends a
//! governed call budget on them. Radio and the related row only read.

pub mod artist_state;
pub mod crawler;
pub mod expand;
pub mod governor;
pub mod graph;
pub mod harvest;
pub mod names;
pub mod roots;
pub mod scheduler;
pub mod setting;
pub mod source;
