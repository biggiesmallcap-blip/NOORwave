use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub(crate) struct ActivationOutput {
    pub beat: Vec<f32>,
    pub downbeat: Vec<f32>,
}

#[derive(Debug, Clone)]
pub(crate) struct BeatEvent {
    pub time_sec: f32,
    pub beat_in_bar: usize,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisOutput {
    pub fps: u32,
    pub beat_times: Vec<f32>,
    pub beat_numbers: Vec<u32>,
    pub beat_confidences: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStage {
    Features = 0,
    Inference = 1,
    Dbn = 2,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgressEvent {
    pub stage: ProgressStage,
    pub progress: f32,
}

pub trait ProgressSink {
    fn on_progress(&mut self, event: ProgressEvent);
}

impl<F> ProgressSink for F
where
    F: FnMut(ProgressEvent),
{
    fn on_progress(&mut self, event: ProgressEvent) {
        self(event);
    }
}
