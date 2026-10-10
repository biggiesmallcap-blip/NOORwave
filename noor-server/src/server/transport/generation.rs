//! The playback generation: bumped by every user transport command so work
//! started under an older generation (a slow stream resolve, a pending-row
//! lookup) does not apply its result. See CONTEXT.md "Playback generation".

use crate::SharedState;

pub(crate) fn current(state: &crate::AppState) -> u64 {
    state
        .playback_generation
        .load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) async fn bump(state: &SharedState) -> u64 {
    let state_guard = state.read().await;
    state_guard
        .playback_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1
}

pub(crate) async fn is_current(state: &SharedState, generation: u64) -> bool {
    let state_guard = state.read().await;
    current(&state_guard) == generation
}
