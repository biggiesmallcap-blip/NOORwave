use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionTemplate {
    SafeCrossfade,
    SlamCut,
    BassSwap16,
    BassSwap32,
    LongHarmonicBlend,
    FilterSweep,
    DropTease16,
}
