//! Windows exclusive-mode output wrapper.

use std::sync::{Arc, Mutex};

pub(crate) use crate::playback::wasapi_exclusive::{
    ExclusiveRenderRole, ExclusiveRenderSource, build_exclusive_stream,
};
use crate::playback::wasapi_exclusive::{ExclusiveRenderSourceBank, ExclusiveStream};

pub(crate) struct ExclusiveRuntimeSink {
    pub(crate) source_bank: Arc<ExclusiveRenderSourceBank>,
    pub(crate) stream: Option<ExclusiveStream>,
    event_identity: Arc<Mutex<Arc<()>>>,
}

impl ExclusiveRuntimeSink {
    pub(crate) fn new(event_identity: Arc<Mutex<Arc<()>>>) -> Self {
        Self {
            source_bank: Arc::new(ExclusiveRenderSourceBank::new()),
            stream: None,
            event_identity,
        }
    }

    fn begin_attempt(&self) -> Arc<()> {
        let identity = Arc::new(());
        *self
            .event_identity
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Arc::clone(&identity);
        identity
    }

    pub(crate) fn is_current_event_identity(&self, identity: &Arc<()>) -> bool {
        Arc::ptr_eq(
            &self
                .event_identity
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
            identity,
        )
    }

    pub(crate) fn retire_stream(&mut self) -> Arc<()> {
        let identity = self.begin_attempt();
        self.stream = None;
        identity
    }

    pub(crate) fn clear(&mut self) {
        self.retire_stream();
        self.source_bank.clear();
    }

    /// A failed acquisition still owns the latest diagnostic. Clearing its
    /// source bank for shared fallback must not retire that failure event.
    pub(crate) fn clear_after_failed_start(&mut self) {
        if self.stream.is_some() {
            self.retire_stream();
        }
        self.source_bank.clear();
    }

    pub(crate) fn needs_rebuild(&self) -> bool {
        self.stream
            .as_ref()
            .map(|stream| stream.is_released())
            .unwrap_or(true)
    }

    /// Ask the live exclusive stream (if any) to drop the device now instead of
    /// waiting out the idle grace, so another app can take it in shared mode.
    /// No-op when no stream is active or it already released.
    pub(crate) fn request_release(&self) {
        if let Some(stream) = self.stream.as_ref() {
            stream.request_release();
        }
    }
}

impl Drop for ExclusiveRuntimeSink {
    fn drop(&mut self) {
        self.retire_stream();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_sink_retires_event_identity_before_renderer_join() {
        let old_identity = Arc::new(());
        let reader = Arc::new(Mutex::new(Arc::clone(&old_identity)));
        let mut sink = ExclusiveRuntimeSink::new(Arc::clone(&reader));
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        sink.stream = Some(ExclusiveStream::test_with_teardown_check(move || {
            let retired = reader
                .try_lock()
                .is_ok_and(|current| !Arc::ptr_eq(&current, &old_identity));
            observed_tx.send(retired).unwrap();
        }));
        let identity = sink.retire_stream();
        assert!(
            observed_rx.try_recv().unwrap(),
            "identity must retire and its mutex unlock before renderer join"
        );
        assert!(Arc::ptr_eq(&sink.event_identity.lock().unwrap(), &identity));
        assert!(sink.stream.is_none());
    }

    #[test]
    fn exclusive_sink_fallback_preserves_failed_attempt_but_clear_retires_it() {
        let reader = Arc::new(Mutex::new(Arc::new(())));
        let mut sink = ExclusiveRuntimeSink::new(reader.clone());
        let initial = Arc::clone(&reader.lock().unwrap());
        let failed_attempt = sink.retire_stream();
        assert!(!Arc::ptr_eq(&initial, &failed_attempt));
        assert!(sink.is_current_event_identity(&failed_attempt));
        assert!(!sink.is_current_event_identity(&initial));
        sink.clear_after_failed_start();
        assert!(Arc::ptr_eq(&reader.lock().unwrap(), &failed_attempt));
        sink.clear();
        assert!(!Arc::ptr_eq(&reader.lock().unwrap(), &failed_attempt));
    }

    #[test]
    fn exclusive_sink_implicit_drop_retires_identity_before_renderer_join() {
        let old_identity = Arc::new(());
        let reader = Arc::new(Mutex::new(Arc::clone(&old_identity)));
        let mut sink = ExclusiveRuntimeSink::new(Arc::clone(&reader));
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        sink.stream = Some(ExclusiveStream::test_with_teardown_check(move || {
            observed_tx
                .send(
                    reader
                        .try_lock()
                        .is_ok_and(|current| !Arc::ptr_eq(&current, &old_identity)),
                )
                .unwrap();
        }));
        drop(sink);
        assert!(
            observed_rx.try_recv().unwrap(),
            "implicit sink teardown must invalidate output events before join"
        );
    }
}
