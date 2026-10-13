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
    let admission = state.read().await.playback_generation_admission.clone();
    let _admission = admission.lock().await;
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

pub(crate) async fn observe(state: &SharedState) -> u64 {
    let state_guard = state.read().await;
    current(&state_guard)
}

/// Bump to a new generation only if it is still `observed`: work that was not
/// itself a user command (a quality re-issue) claims its generation once its
/// result is ready, and loses to any transport command that arrived meanwhile.
pub(crate) async fn claim_if_current(state: &SharedState, observed: u64) -> Option<u64> {
    let admission = state.read().await.playback_generation_admission.clone();
    let _admission = admission.lock().await;
    let state_guard = state.read().await;
    state_guard
        .playback_generation
        .compare_exchange(
            observed,
            observed + 1,
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        )
        .ok()
        .map(|previous| previous + 1)
}

/// A runtime-owned flow: the runtime that emitted its event and the audible
/// decoder identity it was emitted for.
pub(crate) type RuntimeOwner<'a> = (
    &'a crate::playback::runtime::PlaybackRuntimeHandle,
    &'a std::sync::Arc<std::sync::atomic::AtomicU64>,
);

/// True when `generation` is current and, for an owned flow, the same runtime
/// still publishes the same audible decoder. `published_source` is the value
/// read under `lock_playback_source_identity`, held through the mutation.
pub(crate) fn owner_is_current(
    state: &crate::AppState,
    generation: u64,
    expected_owner: Option<RuntimeOwner<'_>>,
    published_source: Option<&std::sync::Arc<std::sync::atomic::AtomicU64>>,
) -> bool {
    current(state) == generation
        && expected_owner.is_none_or(|(runtime, expected_source)| {
            state
                .playback_runtime
                .as_ref()
                .is_some_and(|current| current.handle.is_same_runtime(runtime))
                && published_source
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, expected_source))
        })
}

/// Run a queue/transport write only while its generation (and, for an owned
/// flow, its runtime and decoder) are current. Admission keeps a newer
/// generation from being admitted mid-write; the source lock keeps a decoder
/// restart from replacing the owner. Returns `None` when superseded.
pub(crate) async fn mutate_if_owner_current<T>(
    state: &SharedState,
    generation: u64,
    expected_owner: Option<RuntimeOwner<'_>>,
    mutate: impl FnOnce(&crate::AppState) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    let admission = state.read().await.playback_generation_admission.clone();
    let _admission = admission.lock().await;
    let state_guard = state.read().await;
    let source_guard = expected_owner.map(|(runtime, _)| runtime.lock_playback_source_identity());
    if !owner_is_current(
        &state_guard,
        generation,
        expected_owner,
        source_guard.as_deref(),
    ) {
        return Ok(None);
    }
    mutate(&state_guard).map(Some)
}
