//! Windows exclusive-mode output wrapper.

use std::sync::Arc;

use crate::playback::wasapi_exclusive::{
    ExclusiveInitFailure, ExclusiveRenderSourceBank, ExclusiveStream,
};
pub(crate) use crate::playback::wasapi_exclusive::{ExclusiveRenderRole, ExclusiveRenderSource};

pub(crate) struct ExclusiveRuntimeSink {
    pub(crate) source_bank: Arc<ExclusiveRenderSourceBank>,
    pub(crate) stream: Option<ExclusiveStream>,
}

impl ExclusiveRuntimeSink {
    pub(crate) fn new() -> Self {
        Self {
            source_bank: Arc::new(ExclusiveRenderSourceBank::new()),
            stream: None,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.source_bank.clear();
        self.stream = None;
    }

    pub(crate) fn needs_rebuild(&self) -> bool {
        self.stream
            .as_ref()
            .map(|stream| stream.is_released())
            .unwrap_or(true)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_exclusive_stream(
    device_pref: Option<&str>,
    device_label: String,
    desired_sample_rate: u32,
    channels: u16,
    grace_secs: u32,
    source_bank: Arc<ExclusiveRenderSourceBank>,
    command_tx: std::sync::mpsc::Sender<crate::playback::runtime::PlaybackRuntimeCommand>,
    event_tx: tokio::sync::broadcast::Sender<crate::playback::runtime::PlaybackRuntimeEvent>,
) -> std::result::Result<ExclusiveStream, ExclusiveInitFailure> {
    crate::playback::wasapi_exclusive::build_exclusive_stream(
        device_pref,
        device_label,
        desired_sample_rate,
        channels,
        grace_secs,
        source_bank,
        command_tx,
        event_tx,
    )
}
