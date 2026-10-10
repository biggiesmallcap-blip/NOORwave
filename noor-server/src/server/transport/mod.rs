//! Transport: owns what is playing and how playback moves between queue items.
//! See CONTEXT.md "Transport". It alone bumps and checks the playback
//! generation; HTTP handlers, the phone remote and runtime events go through it.

pub(crate) mod command;
pub(crate) mod events;
pub(crate) mod generation;
pub(crate) mod listen;
pub(crate) mod pending;
pub(crate) mod runtime;
pub(crate) mod settings;
pub(crate) mod snapshot;
pub(crate) mod start;
pub(crate) mod stream;
pub(crate) mod toggle;

#[cfg(test)]
mod tests;
