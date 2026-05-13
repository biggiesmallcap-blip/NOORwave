//! Cross-platform beat + downbeat detection core.

mod config;
mod dbn;
mod features;
mod io;
mod model;
mod types;

pub use config::{
    validate_core_config, ConfigValidationIssue, CoreConfig, DbnConfig, FeatureConfig, ModelConfig,
};
use types::BeatEvent;
pub use types::{AnalysisOutput, ProgressEvent, ProgressSink, ProgressStage};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RhythmError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("model error: {0}")]
    Model(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("io error: {0}")]
    Io(String),
}

/// Analyze a full track of mono PCM samples at 44.1kHz.
///
/// Returns beat arrays.
pub fn analyze(
    samples: &[f32],
    sample_rate: u32,
    config: &CoreConfig,
) -> Result<AnalysisOutput, RhythmError> {
    analyze_with_progress(samples, sample_rate, config, None)
}

/// Analyze a full track using in-memory model data (for WASM).
pub fn analyze_with_model_data(
    samples: &[f32],
    sample_rate: u32,
    config: &CoreConfig,
    model_json: &str,
    weights_npz: &[u8],
) -> Result<AnalysisOutput, RhythmError> {
    analyze_with_progress_and_model_data(
        samples,
        sample_rate,
        config,
        None,
        model_json,
        weights_npz,
    )
}

/// Analyze with optional progress callbacks (0.0..=1.0 for each stage).
pub fn analyze_with_progress(
    samples: &[f32],
    sample_rate: u32,
    config: &CoreConfig,
    progress: Option<&mut dyn ProgressSink>,
) -> Result<AnalysisOutput, RhythmError> {
    if let Err(issue) = validate_core_config(config) {
        return Err(RhythmError::InvalidInput(format!("config {}", issue)));
    }
    if sample_rate != config.feature.sample_rate {
        return Err(RhythmError::InvalidInput(format!(
            "expected {} Hz mono samples; got {}",
            config.feature.sample_rate, sample_rate
        )));
    }

    let mut noop = NoopProgressSink;
    let sink: &mut dyn ProgressSink = match progress {
        Some(s) => s,
        None => &mut noop,
    };

    emit_stage_telemetry("features:start");
    let features = features::compute_features(samples, config, sink)
        .map_err(|e| with_stage_error("features", e))?;
    emit_stage_telemetry("features:end");

    emit_stage_telemetry("inference:start");
    let activations = model::run_inference(&features, config, sink)
        .map_err(|e| with_stage_error("inference", e))?;
    emit_stage_telemetry("inference:end");

    emit_stage_telemetry("dbn:start");
    let beats = dbn::decode(&activations, config, sink).map_err(|e| with_stage_error("dbn", e))?;
    emit_stage_telemetry("dbn:end");
    build_analysis_output(beats, config.feature.fps)
}

/// Analyze using in-memory model data and optional progress callbacks.
pub fn analyze_with_progress_and_model_data(
    samples: &[f32],
    sample_rate: u32,
    config: &CoreConfig,
    progress: Option<&mut dyn ProgressSink>,
    model_json: &str,
    weights_npz: &[u8],
) -> Result<AnalysisOutput, RhythmError> {
    if let Err(issue) = validate_core_config(config) {
        return Err(RhythmError::InvalidInput(format!("config {}", issue)));
    }
    if sample_rate != config.feature.sample_rate {
        return Err(RhythmError::InvalidInput(format!(
            "expected {} Hz mono samples; got {}",
            config.feature.sample_rate, sample_rate
        )));
    }

    let mut noop = NoopProgressSink;
    let sink: &mut dyn ProgressSink = match progress {
        Some(s) => s,
        None => &mut noop,
    };

    emit_stage_telemetry("features:start");
    let features = features::compute_features(samples, config, sink)
        .map_err(|e| with_stage_error("features", e))?;
    emit_stage_telemetry("features:end");

    emit_stage_telemetry("inference:start");
    let activations = model::run_inference_with_data(&features, model_json, weights_npz, sink)
        .map_err(|e| with_stage_error("inference", e))?;
    emit_stage_telemetry("inference:end");

    emit_stage_telemetry("dbn:start");
    let beats = dbn::decode(&activations, config, sink).map_err(|e| with_stage_error("dbn", e))?;
    emit_stage_telemetry("dbn:end");
    build_analysis_output(beats, config.feature.fps)
}

fn build_analysis_output(beats: Vec<BeatEvent>, fps: f32) -> Result<AnalysisOutput, RhythmError> {
    if !fps.is_finite() || fps <= 0.0 {
        return Err(RhythmError::InvalidInput("invalid fps".to_string()));
    }
    let fps = fps.round() as u32;

    let mut beat_times = Vec::with_capacity(beats.len());
    let mut beat_numbers = Vec::with_capacity(beats.len());
    let mut beat_confidences = Vec::with_capacity(beats.len());

    let mut last_time = f32::NEG_INFINITY;
    for event in beats {
        if !event.time_sec.is_finite() {
            return Err(RhythmError::Model(
                "non-finite beat time in decode output".to_string(),
            ));
        }
        if event.time_sec <= last_time {
            return Err(RhythmError::Model(
                "non-increasing beat time in decode output".to_string(),
            ));
        }
        if event.beat_in_bar == 0 {
            return Err(RhythmError::Model(
                "invalid beat_in_bar in decode output".to_string(),
            ));
        }
        beat_times.push(event.time_sec);
        beat_numbers.push(event.beat_in_bar as u32);
        beat_confidences.push(event.confidence.clamp(0.0, 1.0));
        last_time = event.time_sec;
    }

    Ok(AnalysisOutput {
        fps,
        beat_times,
        beat_numbers,
        beat_confidences,
    })
}

struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn on_progress(&mut self, _event: ProgressEvent) {}
}

fn with_stage_error(stage: &str, err: RhythmError) -> RhythmError {
    let (pages, bytes) = wasm_memory_usage();
    let prefix = format!(
        "{} failed (wasm_pages={}, wasm_bytes={}): ",
        stage, pages, bytes
    );
    match err {
        RhythmError::InvalidInput(message) => {
            RhythmError::InvalidInput(format!("{}{}", prefix, message))
        }
        RhythmError::Model(message) => RhythmError::Model(format!("{}{}", prefix, message)),
        RhythmError::NotImplemented(message) => {
            RhythmError::NotImplemented(format!("{}{}", prefix, message))
        }
        RhythmError::Io(message) => RhythmError::Io(format!("{}{}", prefix, message)),
    }
}

fn emit_stage_telemetry(stage: &str) {
    if !telemetry_enabled() {
        return;
    }
    let (pages, bytes) = wasm_memory_usage();
    eprintln!(
        "madmom_beats_port telemetry stage={} wasm_pages={} wasm_bytes={}",
        stage, pages, bytes
    );
}

fn telemetry_enabled() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("MADMOM_BEATS_PORT_TELEMETRY").ok().as_deref() == Some("1")
    }
}

fn wasm_memory_usage() -> (usize, usize) {
    #[cfg(target_arch = "wasm32")]
    {
        let pages = core::arch::wasm32::memory_size(0);
        return (pages, pages.saturating_mul(65_536));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        (0, 0)
    }
}
