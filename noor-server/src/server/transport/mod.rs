//! Transport: owns what is playing and how playback moves between queue items.
//! See CONTEXT.md "Transport". It alone bumps and checks the playback
//! generation; HTTP handlers, the phone remote and runtime events go through it.

pub(crate) mod generation;
pub(crate) mod runtime;
pub(crate) mod stream;
