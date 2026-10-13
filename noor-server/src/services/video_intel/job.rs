//! Background builder: on boot (after a delay) and whenever the catalog has
//! grown enough, rebuild the derived tables on an isolated connection. Never
//! blocks boot; progress is visible in the census.
//!
//! Scaffold from Atlas and Air; the catalog worker owns this file.

use crate::SharedState;

/// Start the background builder. Stub: does nothing yet.
pub fn spawn(_state: SharedState) {}
