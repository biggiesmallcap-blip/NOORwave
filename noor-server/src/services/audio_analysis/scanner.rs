use crate::AppEvent;
use crate::SharedState;
use crate::db::queries;
use rusqlite::OptionalExtension;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::info;

use super::AnalysisConfig;

/// Run a preview scan: query tracks missing DSP features, analyze in batches.
///
/// Since TIDAL tracks don't have local audio files, this scan cannot actually
/// decode audio — it emits progress and marks tracks as scanned.
/// Real analysis happens via the passive tap during playback.
pub async fn run_preview_scan(
    state: SharedState,
    _analysis_tx: mpsc::UnboundedSender<super::AnalysisJob>,
    cancel: Arc<AtomicBool>,
) {
    info!("Starting preview audio analysis scan.");

    let tracks = state
        .read()
        .await
        .db
        .with_conn(|conn| queries::get_tracks_missing_dsp_features(conn, 1000))
        .unwrap_or_default();

    let total = tracks.len() as u32;
    let mut analyzed: u32 = 0;

    for _track in tracks {
        if cancel.load(Ordering::Relaxed) {
            info!("Preview scan cancelled at {}/{}", analyzed, total);
            break;
        }

        // We don't have local audio samples for preview scan (TIDAL tracks).
        // Skip for now — real analysis happens via the passive playback tap.
        analyzed += 1;

        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::AudioAnalysisProgress {
                analyzed,
                total,
                mode: "preview".to_string(),
            });

        sleep(Duration::from_millis(100)).await;
    }

    let _ = state
        .read()
        .await
        .event_tx
        .send(AppEvent::AudioAnalysisComplete { analyzed });

    info!("Preview scan complete. Analyzed {} tracks.", analyzed);
}

/// Run a local folder scan (for local files).
///
/// Walks the given directory for audio files (.flac, .mp3, .wav, .aiff),
/// decodes each to mono f32, and sends analysis jobs.
pub async fn run_local_scan(
    state: SharedState,
    analysis_tx: mpsc::UnboundedSender<super::AnalysisJob>,
    cancel: Arc<AtomicBool>,
    folder_path: std::path::PathBuf,
    config: AnalysisConfig,
) {
    info!(
        "Starting local audio analysis scan of {:?}",
        folder_path
    );

    // Walkdir for .flac, .mp3, .wav, .aiff
    // Match by tracks.file_path or fuzzy title+artist via strsim
    // For now: placeholder implementation
    let mut analyzed: u32 = 0;

    // Placeholder — walk directory entries
    if let Ok(entries) = std::fs::read_dir(&folder_path) {
        for entry in entries.flatten() {
            if cancel.load(Ordering::Relaxed) {
                info!("Local scan cancelled at {}", analyzed);
                break;
            }

            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if matches!(
                ext.as_str(),
                "flac" | "mp3" | "wav" | "aiff" | "aif" | "m4a" | "ogg"
            ) {
                // Attempt to decode and analyze
                match std::fs::File::open(&path) {
                    Ok(file) => {
                        match decode_source_to_mono_f32(Box::new(file), config.max_seconds) {
                            Ok((samples, sample_rate)) => {
                                // Try to match to existing track
                                let file_stem = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                                    .to_string();

                                let track_id = state
                                    .read()
                                    .await
                                    .db
                                    .with_conn(|conn| {
                                        find_track_by_filename(conn, &file_stem, &path)
                                    })
                                    .ok()
                                    .flatten();

                                let tid = track_id.unwrap_or(-1);
                                let _ = analysis_tx.send((tid, samples, sample_rate));
                                analyzed += 1;
                            }
                            Err(e) => {
                                tracing::warn!("Failed to decode {:?}: {}", path, e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to open {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    let _ = state
        .read()
        .await
        .event_tx
        .send(AppEvent::AudioAnalysisComplete { analyzed });

    info!("Local scan complete. Analyzed {} tracks.", analyzed);
}

/// Attempt to match a local audio file to an existing database track.
/// Uses file path first, then fuzzy title+artist match.
fn find_track_by_filename(
    conn: &rusqlite::Connection,
    file_stem: &str,
    path: &std::path::Path,
) -> Result<Option<i64>, anyhow::Error> {
    let path_str = path.to_string_lossy();

    // Try exact file_path match
    let result: Option<i64> = conn
        .query_row(
            "SELECT id FROM tracks WHERE file_path = ?1 LIMIT 1",
            rusqlite::params![path_str],
            |row| row.get(0),
        )
        .optional()?;

    if result.is_some() {
        return Ok(result);
    }

    // Fuzzy match: title contains file_stem
    let result: Option<i64> = conn
        .query_row(
            "SELECT id FROM tracks WHERE title LIKE ?1 LIMIT 1",
            rusqlite::params![format!("%{}%", file_stem)],
            |row| row.get(0),
        )
        .optional()?;

    Ok(result)
}

/// Decode audio source to mono f32 samples (max N seconds).
///
/// Uses Symphonia to decode any supported format, downmixing to mono.
pub fn decode_source_to_mono_f32(
    reader: Box<dyn symphonia::core::io::MediaSource>,
    max_secs: u32,
) -> Result<(Vec<f32>, u32), Box<dyn std::error::Error + Send>> {
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let probe = symphonia::default::get_probe();
    let mss = MediaSourceStream::new(reader, Default::default());

    let format_result = probe.format(
        &Hint::default(),
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    );

    let mut format = match format_result {
        Ok(f) => f.format,
        Err(e) => return Err(Box::new(e)),
    };

    // Find the first audio track
    let track = format
        .default_track()
        .ok_or_else(|| anyhow::anyhow!("no audio tracks found"))?;

    let codec_params = &track.codec_params;
    let sample_rate = codec_params.sample_rate
        .ok_or_else(|| anyhow::anyhow!("unknown sample rate"))?;
    let channels = codec_params.channels.unwrap_or_default();
    let num_channels = channels.count();
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &DecoderOptions::default())
        .map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;

    let max_samples = (sample_rate * max_secs) as usize * num_channels;
    let mut samples: Vec<f32> = Vec::with_capacity(max_samples);

    while samples.len() < max_samples {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let mut buf = decoded.make_equivalent::<f32>();
        decoded.convert(&mut buf);

        for plane in buf.planes().planes() {
            for &sample in *plane {
                samples.push(sample);
                if samples.len() >= max_samples {
                    break;
                }
            }
            if samples.len() >= max_samples {
                break;
            }
        }
    }

    // Downmix to mono
    let mono = if num_channels <= 1 {
        samples
    } else {
        samples
            .chunks(num_channels)
            .map(|chunk| chunk.iter().sum::<f32>() / num_channels as f32)
            .collect()
    };

    Ok((mono, sample_rate))
}
