use crate::AppEvent;
use crate::SharedState;
use crate::db::queries;
use futures::StreamExt;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::info;

use super::AnalysisConfig;

/// Run a preview scan: resolve TIDAL stream URLs at LOW quality, download the
/// first ~30 s of each track, decode, and run DSP analysis.
///
/// Uses "LOW" quality (AAC 96 kbps) to minimise bandwidth — ~360 KB per 30 s.
/// Downloads are capped at 2 MB so we never pull a full lossless album.
pub async fn run_preview_scan(
    state: SharedState,
    _analysis_tx: mpsc::UnboundedSender<super::AnalysisJob>,
    cancel: Arc<AtomicBool>,
) {
    info!("Starting preview audio analysis scan via TIDAL streams (LOW quality).");

    // Grab auth tokens and HTTP client up-front.
    let (tokens, http_client) = {
        let s = state.read().await;
        (s.tidal_tokens.clone(), s.http_client.clone())
    };

    let Some(tokens) = tokens else {
        tracing::warn!("No TIDAL tokens available — cannot run preview scan");
        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::AudioAnalysisComplete { analyzed: 0 });
        return;
    };

    let tracks = state
        .read()
        .await
        .db
        .with_conn(|conn| queries::get_tracks_missing_dsp_features(conn, i64::MAX))
        .unwrap_or_default();

    let total = tracks.len() as u32;
    let mut analyzed: u32 = 0;
    let mut skipped: u32 = 0;

    info!("Preview scan: {} tracks queued", total);

    for track in tracks {
        if cancel.load(Ordering::Relaxed) {
            info!("Preview scan cancelled at {}/{}", analyzed, total);
            break;
        }

        // Only TIDAL tracks have a resolvable stream URL.
        let Some(tidal_id) = track.tidal_id else {
            skipped += 1;
            continue;
        };

        // ── 1. Resolve stream URL ─────────────────────────────────────────────
        let stream_info = match crate::services::tidal::stream::get_stream_url(
            &http_client,
            &tokens.access_token,
            tidal_id,
            "LOW", // 96 kbps AAC — sufficient for BPM/key/energy analysis
        )
        .await
        {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(
                    track_id = track.id,
                    tidal_id,
                    "Could not resolve stream: {}",
                    e
                );
                if e.is_session_expired() {
                    tracing::error!("TIDAL session expired — aborting preview scan");
                    break;
                }
                skipped += 1;
                continue;
            }
        };

        tracing::info!(
            track_id = track.id,
            tidal_id,
            codec = %stream_info.codec,
            quality = %stream_info.audio_quality,
            segments = stream_info.segment_urls.len(),
            url_prefix = %stream_info.url.chars().take(60).collect::<String>(),
            "stream resolved"
        );

        // ── 2. Download audio bytes (cap at 8 MB) ────────────────────────────
        // 8 MB ≈ 11 min of 96 kbps AAC — covers full-length songs at LOW
        // quality. We need the WHOLE file because TIDAL's MP4s typically
        // place the `moov` atom (track metadata symphonia needs) at the END
        // of the file. A smaller cap truncates moov and triggers EOF errors.
        //
        // For DASH SegmentTemplate manifests, `stream_info.url` is the init
        // segment and the audio is in `segment_urls`; chain them. For BTS /
        // JSON manifests `segment_urls` is empty and `url` points at the
        // full file directly.
        const MAX_BYTES: usize = 8 * 1024 * 1024;
        let mut buf: Vec<u8> = Vec::with_capacity(512 * 1024);
        let mut fetch_failed = false;

        'segments: for seg_url in std::iter::once(&stream_info.url)
            .chain(stream_info.segment_urls.iter())
        {
            if buf.len() >= MAX_BYTES { break; }

            let resp = match http_client.get(seg_url).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(track_id = track.id, "Failed to fetch segment: {}", e);
                    fetch_failed = true;
                    break;
                }
            };

            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(c) => {
                        let remaining = MAX_BYTES.saturating_sub(buf.len());
                        if c.len() <= remaining {
                            buf.extend_from_slice(&c);
                        } else {
                            buf.extend_from_slice(&c[..remaining]);
                            break 'segments;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(track_id = track.id, "Stream read error: {}", e);
                        fetch_failed = true;
                        break;
                    }
                }
            }
            if fetch_failed { break; }
        }

        let audio_bytes = buf;
        let magic: String = audio_bytes
            .iter()
            .take(16)
            .map(|b| format!("{:02x}", b))
            .collect();
        tracing::info!(
            track_id = track.id,
            bytes = audio_bytes.len(),
            fetch_failed,
            magic = %magic,
            "audio bytes accumulated"
        );

        if audio_bytes.is_empty() || (fetch_failed && audio_bytes.len() < 32 * 1024) {
            skipped += 1;
            continue;
        }

        // ── 3. Decode to mono f32 (max 30 s) ─────────────────────────────────
        let cursor = std::io::Cursor::new(audio_bytes);
        let decode_result =
            tokio::task::spawn_blocking(move || decode_source_to_mono_f32(Box::new(cursor), 30))
                .await;

        let (samples, sample_rate) = match decode_result {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                tracing::warn!(track_id = track.id, "Decode failed: {}", e);
                skipped += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(track_id = track.id, "Decode task panicked: {}", e);
                skipped += 1;
                continue;
            }
        };

        // Diagnostic: report decoded buffer characteristics so we can tell
        // silent-from-source from silent-from-mixdown-bug.
        let n = samples.len();
        let nonzero = samples.iter().filter(|s| s.abs() > 1e-6).count();
        let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let rms = if n > 0 {
            (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / n as f64).sqrt()
        } else {
            0.0
        };
        tracing::info!(
            track_id = track.id,
            samples = n,
            sample_rate,
            nonzero_count = nonzero,
            peak_abs = peak,
            rms = rms,
            "decoded audio stats"
        );

        // ── 4. Skip the first 10 s of the preview clip ───────────────────────
        // Track intros (fades, single-instrument opens, sustained pads) have
        // different rhythmic content and contaminate BPM/key detection.
        // Only skip when there are at least 4 s of audio remaining after the
        // offset so the analyser has enough signal to work with.
        const PREVIEW_OFFSET_SEC: usize = 10;
        let offset_samples = sample_rate as usize * PREVIEW_OFFSET_SEC;
        let (samples, applied_offset_ms) = if samples.len() > offset_samples + sample_rate as usize * 4 {
            (samples[offset_samples..].to_vec(), (PREVIEW_OFFSET_SEC * 1000) as i64)
        } else {
            (samples, 0i64)
        };

        // ── 5. Analyze and save ───────────────────────────────────────────────
        let db = state.read().await.db.clone();
        let tid = track.id;
        let saved = tokio::task::spawn_blocking(move || {
            super::engine::analyze_and_save(&db, &samples, sample_rate, "preview", tid, applied_offset_ms)
        })
        .await
        .ok()
        .flatten();

        match &saved {
            Some(f) => {
                analyzed += 1;
                info!(
                    "[{}/{}] ✓ {} — BPM: {}, key: {}, energy: {:.2}, beat_strength: {:.2}",
                    analyzed,
                    total,
                    track.title,
                    f.bpm.map(|b| format!("{:.1}", b)).unwrap_or_else(|| "?".into()),
                    f.key_signature.as_deref().unwrap_or("?"),
                    f.energy.unwrap_or(0.0),
                    f.beat_strength.unwrap_or(0.0),
                );
            }
            None => {
                skipped += 1;
                info!("[{}/{}] ✗ {} — no features extracted", analyzed, total, track.title);
            }
        }

        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::AudioAnalysisProgress {
                analyzed,
                total,
                mode: "preview".to_string(),
            });

        // Rate-limit: avoid hammering TIDAL's API.
        sleep(Duration::from_millis(200)).await;
    }

    let _ = state
        .read()
        .await
        .event_tx
        .send(AppEvent::AudioAnalysisComplete { analyzed });

    // Post-scan: let SQLite reoptimise the fingerprint_hashes index.
    let db_for_opt = state.read().await.db.clone();
    tokio::task::spawn_blocking(move || super::fingerprint::optimize_after_bulk_scan(&db_for_opt))
        .await
        .ok();

    info!(
        "Preview scan complete. Analyzed {}/{} tracks ({} skipped).",
        analyzed, total, skipped
    );
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
    info!("Starting local audio analysis scan of {:?}", folder_path);

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
    let sample_rate = codec_params
        .sample_rate
        .ok_or_else(|| anyhow::anyhow!("unknown sample rate"))?;
    // codec_params.channels can be None for MP4/AAC where channel count is
    // only available in the AAC codec-specific config (esds → AudioSpecificConfig)
    // and the demuxer doesn't surface it on the track. Default to stereo for
    // the upper bound; the real channel count comes from the decoded buffer below.
    let metadata_channels = codec_params.channels.map(|c| c.count()).unwrap_or(0);
    let channel_upper_bound = metadata_channels.max(2);
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &DecoderOptions::default())
        .map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;

    let max_samples = (sample_rate * max_secs) as usize * channel_upper_bound;
    let mut samples: Vec<f32> = Vec::with_capacity(max_samples);
    let mut decoded_channels: usize = 0;
    let mut packet_err: Option<String> = None;
    let mut decode_err_count: u32 = 0;
    let mut last_decode_err: Option<String> = None;
    let mut packets_seen: u32 = 0;

    while samples.len() < max_samples {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(e) => {
                packet_err = Some(e.to_string());
                break;
            }
        };
        packets_seen += 1;

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(e) => {
                decode_err_count += 1;
                last_decode_err = Some(e.to_string());
                continue;
            }
        };

        if decoded_channels == 0 {
            decoded_channels = decoded.spec().channels.count().max(1);
        }

        // Copy to an interleaved SampleBuffer — this matches the playback
        // runtime's pattern and gives us LRLR... ordering so the chunked
        // downmix below is correct. The decoded AudioBuffer is planar
        // (LLLL...RRRR...) and the previous code conflated the two.
        let mut sb = symphonia::core::audio::SampleBuffer::<f32>::new(
            decoded.capacity() as u64,
            *decoded.spec(),
        );
        sb.copy_interleaved_ref(decoded);
        let interleaved = sb.samples();
        let remaining = max_samples.saturating_sub(samples.len());
        let take = interleaved.len().min(remaining);
        samples.extend_from_slice(&interleaved[..take]);
    }

    // Downmix to mono using the channel count we observed from the actual
    // decoded buffers. Falls back to 1 when no packets ever decoded.
    let downmix_channels = decoded_channels.max(1);
    let mono = if downmix_channels <= 1 {
        samples
    } else {
        samples
            .chunks(downmix_channels)
            .map(|chunk| chunk.iter().sum::<f32>() / downmix_channels as f32)
            .collect()
    };

    if mono.is_empty() {
        tracing::warn!(
            packets_seen,
            decode_err_count,
            packet_err = packet_err.as_deref().unwrap_or(""),
            last_decode_err = last_decode_err.as_deref().unwrap_or(""),
            "decode produced 0 samples"
        );
    }

    Ok((mono, sample_rate))
}
