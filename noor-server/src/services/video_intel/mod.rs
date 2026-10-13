//! Video catalog intelligence (Atlas and Air). Turns raw `video_catalog`
//! rows, the artist graph and the music library into the derived tables of
//! migration 077: cuts, songs, concerts, scenes, artist facets, era and map
//! layout. Pure local work: nothing here calls TIDAL.
//!
//! Build order (see `build::build_all`): cuts -> songs -> concerts -> scenes
//! -> spread -> layout -> era -> link. Each stage is idempotent and rebuilds
//! its own rows, so a shipped install heals itself on the next run.
//!
//! Ownership while Atlas and Air is built in parallel:
//! - catalog worker: cuts, songs, concerts, era, link, census, job, build
//! - atlas worker: scenes, louvain, spread, layout, light
//! - atlas looks: pictures (Picture's thumbnail colours, its own background
//!   job), you (footprints, trails, frontier)

pub mod archive;
pub mod build;
pub mod census;
pub mod concerts;
pub mod cuts;
pub mod era;
pub mod job;
pub mod layout;
pub mod light;
pub mod link;
pub mod louvain;
pub mod pictures;
pub mod scenes;
pub mod songs;
pub mod spread;
pub mod you;
