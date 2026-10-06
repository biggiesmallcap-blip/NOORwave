use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DjProfile {
    /// Tempo of the phase grid. Independent clip tempo must not relabel this grid.
    pub bpm: Option<f32>,
    /// Independently analysed DSP tempo, useful when phase evidence is limited.
    #[serde(default)]
    pub tempo_bpm: Option<f32>,
    #[serde(default)]
    pub tempo_confidence: Option<f32>,
    /// Uniform, zero-anchored grids estimate periodicity without measured phase.
    #[serde(default)]
    pub grid_is_synthetic: bool,
    /// Explicit detector provenance; older/imported profiles default to unknown.
    #[serde(default)]
    pub grid_is_measured: bool,
    pub camelot_key: Option<String>,
    pub energy: Option<f32>,
    pub beat_grid_seconds: Vec<f32>,
    pub downbeat_seconds: Vec<f32>,
    pub phrase_bar_indices: Vec<u32>,
    pub mix_in_seconds: Vec<f32>,
    pub mix_out_seconds: Vec<f32>,
    pub intro_end_seconds: Option<f32>,
    pub outro_start_seconds: Option<f32>,
    pub breakdown_seconds: Vec<f32>,
    pub drop_seconds: Vec<f32>,
    #[serde(default)]
    pub manual_drop_seconds: Vec<f32>,
    pub safe_transition_windows: Vec<TransitionWindow>,
    pub vocal_presence_by_bar: Vec<f32>,
    pub vocal_density_by_bar: Vec<f32>,
    pub lufs_loud_body: Option<f32>,
    pub true_peak_dbtp: Option<f32>,
    pub profile_confidence: f32,
    #[serde(default)]
    pub beat_confidence: Option<f32>,
    #[serde(default)]
    pub energy_contour: Vec<f32>,
    #[serde(default)]
    pub analysis_scope_seconds: Option<f32>,
    #[serde(default)]
    pub vocals_known: bool,
    pub safe_crossfade_only: bool,
    pub profile_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionWindow {
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub confidence: f32,
}

impl DjProfile {
    pub fn has_full_dj_profile(&self) -> bool {
        !self.beat_grid_seconds.is_empty()
            && !self.downbeat_seconds.is_empty()
            && !self.phrase_bar_indices.is_empty()
            && !self.mix_in_seconds.is_empty()
            && !self.mix_out_seconds.is_empty()
            && !self.safe_transition_windows.is_empty()
            && !self.profile_version.is_empty()
    }
}
