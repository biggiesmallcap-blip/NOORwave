//! Sportify (anonymous Spotify metadata proxy) discovery layer.
//!
//! Sportify is the discovery brain — search, browse, playlist/album/artist
//! detail pages, top tracks, relationships. TIDAL remains the speakers:
//! every playable item is resolved against the existing TIDAL client and
//! actual audio comes from `services::tidal`.
//!
//! Phase 1 (this commit): client + meta cache + DB plumbing. No HTTP routes
//! yet — those land in phase 3.
//!
//! World playcounts and monthly listeners extracted from Sportify responses
//! are written into the existing `spotify_track_stats` /
//! `spotify_artist_stats` tables (MIGRATION_029) so the legacy artist page
//! benefits without parallel state.

pub mod cache;
pub mod client;
pub mod models;
pub mod normalize;
pub mod recommend;
pub mod resolver;
pub mod stats;

pub use client::{SportifyClient, SportifyClientConfig};
