pub(crate) mod cdn_health;
pub mod resample;
pub mod source;

use self::resample::{StreamResampler, adapt_channels, extend_mono_from_interleaved};
use self::source::{
    StreamPipe, append_stream_bytes, build_tidal_cdn_client, dash_background_fetch_window,
    dash_initial_media_count,
};
use crate::playback::player::{PlaybackSourceRequest, PreparedPlaybackJob};
use crate::playback::runtime::PlaybackRuntimeConfig;
use crate::playback::runtime::commands::PlaybackRuntimeCommand;
use crate::playback::runtime::shared::PlaybackSharedState;
use crate::services::audio_analysis::dj_profile::DjAnalysisJob;
use anyhow::{Context, Result, anyhow};
use futures::StreamExt as _;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::runtime::Builder as TokioRuntimeBuilder;
use tracing::{debug, info, warn};

const DJ_ANALYSIS_MAX_SECONDS: usize = 90;
const DJ_ANALYSIS_DASH_PREFETCH_MEDIA_SEGMENTS: usize = 3;
const DJ_ANALYSIS_DASH_PREFETCH_ATTEMPTS: usize = 3;
const DJ_ANALYSIS_DASH_PREFETCH_RETRY_BACKOFF_MS: u64 = 150;
const PLAYBACK_DASH_PREFETCH_ATTEMPTS: usize = 2;
const PLAYBACK_DASH_PREFETCH_RETRY_BACKOFF_MS: u64 = 100;
const PLAYBACK_DASH_BACKGROUND_FETCH_ATTEMPTS: usize = 3;
const PLAYBACK_DASH_BACKGROUND_FETCH_RETRY_BACKOFF_MS: u64 = 150;
const PLAYBACK_DASH_BACKGROUND_FETCH_STOP_POLL_MS: u64 = 25;
const PLAYBACK_BUFFER_RETAIN_BEHIND_MS: i32 = 10_000;
const PLAYBACK_DECODE_HIGH_WATER_SECS: u64 = 45;
const PLAYBACK_DECODE_LOW_WATER_SECS: u64 = 30;
const PLAYBACK_DECODE_BACKPRESSURE_SLEEP_MS: u64 = 25;

#[derive(Debug)]
struct DashPrebuffer {
    bytes: Vec<u8>,
    fetched_media_segments: usize,
    stopped: bool,
    ended_after_prefix_failure: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DashBackgroundFetchSummary {
    sent_segments: usize,
    sent_bytes: usize,
    slowest_fetch_ms: u64,
    stopped: bool,
    receiver_closed: bool,
}

fn dash_prebuffer_media_count(total_segments: usize, dj_analysis_only: bool) -> usize {
    if dj_analysis_only {
        total_segments.min(DJ_ANALYSIS_DASH_PREFETCH_MEDIA_SEGMENTS)
    } else {
        dash_initial_media_count(total_segments)
    }
}

async fn fetch_dash_prebuffer<F, Fut>(
    init_url: &str,
    media_urls: &[String],
    dj_analysis_only: bool,
    stop: &std::sync::atomic::AtomicBool,
    mut fetch: F,
) -> Result<DashPrebuffer>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let media_count = dash_prebuffer_media_count(media_urls.len(), dj_analysis_only);
    if media_count == 0 {
        return Ok(DashPrebuffer {
            bytes: Vec::new(),
            fetched_media_segments: 0,
            stopped: stop.load(Ordering::Relaxed),
            ended_after_prefix_failure: false,
        });
    }

    if stop.load(Ordering::Relaxed) {
        return Ok(DashPrebuffer {
            bytes: Vec::new(),
            fetched_media_segments: 0,
            stopped: true,
            ended_after_prefix_failure: false,
        });
    }

    let mut bytes =
        fetch_dash_prebuffer_part(init_url.to_string(), 0, dj_analysis_only, &mut fetch).await?;
    if stop.load(Ordering::Relaxed) {
        return Ok(DashPrebuffer {
            bytes,
            fetched_media_segments: 0,
            stopped: true,
            ended_after_prefix_failure: false,
        });
    }

    let mut fetched_media_segments = 0usize;
    for (idx, segment_url) in media_urls.iter().take(media_count).enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Ok(DashPrebuffer {
                bytes,
                fetched_media_segments,
                stopped: true,
                ended_after_prefix_failure: false,
            });
        }

        match fetch_dash_prebuffer_part(segment_url.clone(), idx + 1, dj_analysis_only, &mut fetch)
            .await
        {
            Ok(segment) => {
                bytes.extend(segment);
                fetched_media_segments += 1;
            }
            Err(_) if dj_analysis_only && fetched_media_segments > 0 => {
                return Ok(DashPrebuffer {
                    bytes,
                    fetched_media_segments,
                    stopped: false,
                    ended_after_prefix_failure: true,
                });
            }
            Err(error) => return Err(error),
        }
    }

    Ok(DashPrebuffer {
        bytes,
        fetched_media_segments,
        stopped: false,
        ended_after_prefix_failure: false,
    })
}

async fn fetch_dash_prebuffer_part<F, Fut>(
    url: String,
    segment_index: usize,
    dj_analysis_only: bool,
    fetch: &mut F,
) -> Result<Vec<u8>>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let (attempts, retry_backoff_ms) = if dj_analysis_only {
        (
            DJ_ANALYSIS_DASH_PREFETCH_ATTEMPTS,
            DJ_ANALYSIS_DASH_PREFETCH_RETRY_BACKOFF_MS,
        )
    } else {
        (
            PLAYBACK_DASH_PREFETCH_ATTEMPTS,
            PLAYBACK_DASH_PREFETCH_RETRY_BACKOFF_MS,
        )
    };
    let mut last_error = None;
    for attempt in 0..attempts {
        match fetch(url.clone(), segment_index).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < attempts {
                    tokio::time::sleep(Duration::from_millis(retry_backoff_ms)).await;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("DASH prebuffer fetch failed")))
}

async fn fetch_dash_segment_with_retries<F, Fut>(
    url: String,
    segment_index: usize,
    attempts: usize,
    retry_backoff_ms: u64,
    stop: &AtomicBool,
    fetch: F,
) -> Result<Vec<u8>>
where
    F: Fn(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let attempts = attempts.max(1);
    let mut last_error = None;
    for attempt in 0..attempts {
        if stop.load(Ordering::Relaxed) {
            return Err(anyhow!("DASH segment fetch stopped"));
        }

        let fetch_result = tokio::select! {
            result = fetch(url.clone(), segment_index) => result,
            _ = wait_for_dash_stop(stop) => Err(anyhow!("DASH segment fetch stopped")),
        };

        match fetch_result {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                let should_retry = is_retryable_dash_fetch_error(&error);
                last_error = Some(error);
                if !should_retry || attempt + 1 >= attempts {
                    break;
                }
                if sleep_dash_retry_backoff(stop, retry_backoff_ms).await {
                    return Err(anyhow!("DASH segment fetch stopped"));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("DASH segment fetch failed")))
}

fn is_retryable_dash_fetch_error(error: &anyhow::Error) -> bool {
    for cause in error.chain() {
        if cause.is::<tokio::time::error::Elapsed>() {
            return true;
        }

        let Some(reqwest_error) = cause.downcast_ref::<reqwest::Error>() else {
            continue;
        };
        if let Some(status) = reqwest_error.status() {
            return status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        }
        if reqwest_error.is_timeout() || reqwest_error.is_connect() || reqwest_error.is_body() {
            return true;
        }
    }
    false
}

async fn sleep_dash_retry_backoff(stop: &AtomicBool, backoff_ms: u64) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => false,
        _ = wait_for_dash_stop(stop) => true,
    }
}

async fn wait_for_dash_stop(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(
            PLAYBACK_DASH_BACKGROUND_FETCH_STOP_POLL_MS,
        ))
        .await;
    }
}

async fn fetch_dash_segments_ordered<F, Fut, S>(
    media_urls: Vec<String>,
    first_segment_index: usize,
    fetch_window: usize,
    stop: &AtomicBool,
    fetch: F,
    mut send: S,
) -> Result<DashBackgroundFetchSummary>
where
    F: Fn(String, usize) -> Fut + Clone,
    Fut: Future<Output = Result<Vec<u8>>>,
    S: FnMut(Vec<u8>) -> bool,
{
    if stop.load(Ordering::Relaxed) {
        return Ok(DashBackgroundFetchSummary {
            stopped: true,
            ..DashBackgroundFetchSummary::default()
        });
    }

    let fetch_window = fetch_window.max(1);
    let fetches = futures::stream::iter(media_urls.into_iter().enumerate().map(|(idx, url)| {
        let fetch = fetch.clone();
        async move {
            let started = Instant::now();
            let segment_index = first_segment_index + idx;
            fetch_dash_segment_with_retries(
                url,
                segment_index,
                PLAYBACK_DASH_BACKGROUND_FETCH_ATTEMPTS,
                PLAYBACK_DASH_BACKGROUND_FETCH_RETRY_BACKOFF_MS,
                stop,
                fetch,
            )
            .await
            .map(|bytes| (segment_index, started.elapsed(), bytes))
        }
    }))
    .buffered(fetch_window);
    futures::pin_mut!(fetches);

    let mut summary = DashBackgroundFetchSummary::default();
    while let Some(result) = fetches.next().await {
        if stop.load(Ordering::Relaxed) {
            summary.stopped = true;
            break;
        }

        let (_segment_index, elapsed, bytes) = result?;
        summary.slowest_fetch_ms = summary
            .slowest_fetch_ms
            .max(elapsed.as_millis().min(u128::from(u64::MAX)) as u64);
        summary.sent_bytes += bytes.len();
        if !send(bytes) {
            summary.receiver_closed = true;
            break;
        }
        summary.sent_segments += 1;
    }

    Ok(summary)
}

fn decoded_output_samples_for_secs(secs: u64, sample_rate: u32, channels: u16) -> usize {
    secs.saturating_mul(sample_rate as u64)
        .saturating_mul(channels.max(1) as u64)
        .min(usize::MAX as u64) as usize
}

fn should_backpressure_decode(unread_samples: usize, high_water_samples: usize) -> bool {
    unread_samples > high_water_samples
}

fn apply_playback_decode_backpressure(
    shared: &Arc<PlaybackSharedState>,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    let high_water =
        decoded_output_samples_for_secs(PLAYBACK_DECODE_HIGH_WATER_SECS, sample_rate, channels);
    let low_water =
        decoded_output_samples_for_secs(PLAYBACK_DECODE_LOW_WATER_SECS, sample_rate, channels);
    if !should_backpressure_decode(shared.unread_buffered_samples()?, high_water) {
        return Ok(());
    }

    while !shared.stopped.load(Ordering::Relaxed) && shared.unread_buffered_samples()? > low_water {
        thread::sleep(Duration::from_millis(PLAYBACK_DECODE_BACKPRESSURE_SLEEP_MS));
    }
    Ok(())
}

pub(crate) fn decode_and_buffer_job(
    config: PlaybackRuntimeConfig,
    job: PreparedPlaybackJob,
    shared: Arc<PlaybackSharedState>,
    device_sample_rate: u32,
    device_channels: u16,
) -> Result<()> {
    match job.source {
        PlaybackSourceRequest::LocalLibrary => {
            return Err(anyhow!(
                "local library playback is not wired into the host-audio runtime yet"
            ));
        }
        PlaybackSourceRequest::TidalStream(ref request) => {
            // ── Step 1: resolve the stream URL (async, needs a mini tokio runtime) ──────────
            let rt = TokioRuntimeBuilder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create async runtime for TIDAL stream fetch")?;

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            let stream_info = rt.block_on(config.resolve_stream(request.clone()))?;
            debug!(
                "TIDAL runtime stream resolved: track_id={}, quality={}, codec={}, sample_rate={:?}, bit_depth={:?}, dash_segments={}, start_from_segment={}, start_from_offset_ms={}",
                shared.track_id,
                stream_info.audio_quality,
                stream_info.codec,
                stream_info.sample_rate,
                stream_info.bit_depth,
                stream_info.segment_urls.len(),
                job.start_from_segment_index,
                job.start_from_offset_ms,
            );

            // Publish segment offsets to shared state so the runtime's SeekTo
            // handler can resolve a target_ms back to the segment that contains
            // it. Cheap one-shot OnceLock::set; ignore the Err if a prior call
            // already populated it (shouldn't happen, but harmless).
            let _ = shared
                .segment_offsets_ms
                .set(stream_info.segment_offsets_ms.clone());

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            // ── Step 2: stream bytes from the CDN in a background thread ─────────────────────
            // Single-URL streams are limited by the bounded chunk channel
            // below. DASH streams fetch whole media segments, so memory is
            // bounded by the initial prebuffer plus DASH_BACKGROUND_FETCH_WINDOW
            // whole segments waiting to be delivered in manifest order.
            // A separate one-shot channel carries the Content-Length from the response headers
            // so StreamPipe can report byte_len() correctly, allowing Symphonia's MSS to
            // translate SeekFrom::End into an absolute position rather than failing.
            //
            // Segment-aware seek (option C): for a segment-restart job, skip
            // `start_from_segment_index` URLs before counting prebuffer segments
            // or kicking off the background download. The init segment URL
            // (`stream_info.url`) is ALWAYS fetched - it carries the fMP4 init
            // box that the decoder needs regardless of which media segment is
            // first.
            let is_dash_stream = !stream_info.segment_urls.is_empty();
            let start_index = job
                .start_from_segment_index
                .min(stream_info.segment_urls.len());
            let sliced_segment_urls: Vec<String> = stream_info
                .segment_urls
                .iter()
                .skip(start_index)
                .cloned()
                .collect();
            let mut dash_initial = Vec::new();
            let mut remaining_segment_urls = sliced_segment_urls.clone();
            let mut prebuffered_media_segments = 0usize;
            let target_media_segments =
                dash_prebuffer_media_count(sliced_segment_urls.len(), config.dj_analysis_only);
            if target_media_segments > 0 {
                let prebuffer_stop = shared.stop_flag();
                let prebuffer_started = Instant::now();
                let prebuffer = rt
                    .block_on(fetch_dash_prebuffer(
                        &stream_info.url,
                        &sliced_segment_urls,
                        config.dj_analysis_only,
                        &prebuffer_stop,
                        |url, segment_index| {
                            let http = config.http_client.clone();
                            async move { append_stream_bytes(&http, &url, segment_index).await }
                        },
                    ))
                    .context("DASH stream prebuffer failed")?;
                if prebuffer.ended_after_prefix_failure {
                    warn!(
                        "TIDAL DASH analysis prebuffer stopped at contiguous prefix: track_id={}, fetched_segments={}, target_segments={}",
                        shared.track_id, prebuffer.fetched_media_segments, target_media_segments
                    );
                }
                if config.dj_analysis_only
                    && !prebuffer.stopped
                    && prebuffer.fetched_media_segments == 0
                {
                    return Err(anyhow!(
                        "DASH stream prebuffer failed: fetched no media segments"
                    ));
                }
                prebuffered_media_segments = prebuffer.fetched_media_segments;
                dash_initial = prebuffer.bytes;
                remaining_segment_urls.drain(0..prebuffered_media_segments);
                info!(
                    "TIDAL DASH prebuffer ready track_id={} start_index={} initial_segments={} remaining_segments={} elapsed_ms={} bytes={}",
                    shared.track_id,
                    start_index,
                    prebuffered_media_segments,
                    remaining_segment_urls.len(),
                    prebuffer_started.elapsed().as_millis(),
                    dash_initial.len()
                );
                debug!(
                    "TIDAL DASH prebuffer ready: track_id={}, start_index={}, initial_segments={}, remaining_segments={}",
                    shared.track_id,
                    start_index,
                    prebuffered_media_segments,
                    remaining_segment_urls.len()
                );
            }

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            let (len_tx, len_rx) = std::sync::mpsc::sync_channel::<Option<u64>>(1);
            let (chunk_tx, chunk_rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(32);
            let url = stream_info.url.clone();
            let download_track_id = shared.track_id;
            let segment_urls = remaining_segment_urls;
            let download_stop = shared.stop_flag();
            let download_is_analysis_only = config.dj_analysis_only;
            thread::Builder::new()
                .name("noor-stream-download".into())
                .spawn(move || {
                    let dl_rt = TokioRuntimeBuilder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to build download runtime");
                    dl_rt.block_on(async move {
                        let result: anyhow::Result<()> = async {
                            let http = build_tidal_cdn_client();
                            if !is_dash_stream {
                                if download_stop.load(Ordering::Relaxed) {
                                    return Ok(());
                                }
                                // Single-URL path (JSON manifest or DASH BaseURL shape)
                                let response = http
                                    .get(&url)
                                    .send()
                                    .await
                                    .context("download request failed")?
                                    .error_for_status()
                                    .context("download returned error status")?;
                                // Send Content-Length before any chunks so the decode thread
                                // can populate StreamPipe::known_length immediately.
                                let _ = len_tx.send(response.content_length());
                                let mut stream = response.bytes_stream();
                                while let Some(chunk) = stream.next().await {
                                    if download_stop.load(Ordering::Relaxed) {
                                        break; // track stopped mid-fetch
                                    }
                                    let bytes = chunk.context("chunk read error")?;
                                    if chunk_tx.send(Some(bytes.to_vec())).is_err() {
                                        break; // decoder stopped early (track skipped/stopped)
                                    }
                                }
                            } else {
                                let _ = len_tx.send(None);
                                let total_segments = segment_urls.len();
                                let first_segment_index = prebuffered_media_segments + 1;
                                let fetch_window = dash_background_fetch_window();
                                let fetch_http = http.clone();
                                let summary = fetch_dash_segments_ordered(
                                    segment_urls,
                                    first_segment_index,
                                    fetch_window,
                                    &download_stop,
                                    move |seg_url, segment_index| {
                                        let http = fetch_http.clone();
                                        async move {
                                            let started = Instant::now();
                                            let bytes =
                                                append_stream_bytes(&http, &seg_url, segment_index)
                                                    .await?;
                                            tracing::info!(
                                                target: "noor.dash",
                                                event = "dash_segment_fetched",
                                                track_id = download_track_id,
                                                segment_index,
                                                fetch_ms = started.elapsed().as_millis() as u64,
                                                bytes = bytes.len(),
                                                fetch_window,
                                                "DASH segment fetched"
                                            );
                                            Ok(bytes)
                                        }
                                    },
                                    |bytes| chunk_tx.send(Some(bytes)).is_ok(),
                                )
                                .await?;
                                if summary.stopped {
                                    warn!(
                                        "TIDAL DASH download cancelled: track_id={}, sent_segments={}, total_remaining_segments={}",
                                        download_track_id,
                                        summary.sent_segments,
                                        total_segments
                                    );
                                } else if summary.receiver_closed {
                                    if download_is_analysis_only {
                                        debug!(
                                            "TIDAL DASH analysis download stopped after capture: track_id={}, sent_segments={}, total_remaining_segments={}",
                                            download_track_id,
                                            summary.sent_segments,
                                            total_segments
                                        );
                                    } else {
                                        warn!(
                                            "TIDAL DASH download stopped early: track_id={}, sent_segments={}, total_remaining_segments={}",
                                            download_track_id,
                                            summary.sent_segments,
                                            total_segments
                                        );
                                    }
                                }
                                debug!(
                                    "TIDAL DASH download EOF: track_id={}, sent_segments={}, total_remaining_segments={}, bytes={}, slowest_fetch_ms={}, fetch_window={}",
                                    download_track_id,
                                    summary.sent_segments,
                                    total_segments,
                                    summary.sent_bytes,
                                    summary.slowest_fetch_ms,
                                    fetch_window
                                );
                            }
                            Ok(())
                        }
                        .await;
                        if let Err(err) = result {
                            warn!("TIDAL stream download error: {err:?}");
                            // Ensure len_rx unblocks if the request failed before sending length.
                            let _ = len_tx.try_send(None);
                        }
                        let _ = chunk_tx.send(None); // signal EOF regardless
                    });
                })
                .context("failed to spawn download thread")?;

            // Block briefly until response headers arrive so we know Content-Length.
            let content_length = len_rx.recv().ok().flatten();

            // ── Step 3: probe + decode incrementally, writing to the buffer each packet ──────
            let pipe = if is_dash_stream {
                StreamPipe::with_initial(
                    dash_initial,
                    chunk_rx,
                    content_length,
                    true,
                    shared.stop_flag(),
                )
            } else {
                StreamPipe::new(chunk_rx, content_length, shared.stop_flag())
            };
            let mss = MediaSourceStream::new(Box::new(pipe), Default::default());

            // Give Symphonia a format hint from the Tidal manifest MIME type so it
            // can skip the seeking probes it uses for format auto-detection.
            // Without this, the MP4/AAC reader tries SeekFrom::End to locate the
            // moov atom, which can fail on a live streaming pipe.
            let mut hint = Hint::new();
            if !stream_info.segment_urls.is_empty() {
                // DASH fMP4 (CMAF): init+segments form an ISOBMFF stream regardless of inner
                // codec. The m4a hint routes Symphonia to its IsoMp4 reader which handles
                // fragmented MP4 linearly without attempting a SeekFrom::End moov search.
                hint.with_extension("m4a");
            } else {
                let codec_lower = stream_info.codec.to_ascii_lowercase();
                let ext = if codec_lower.contains("flac") {
                    Some("flac")
                } else if codec_lower.contains("mp3") || codec_lower.contains("mpeg") {
                    Some("mp3")
                } else if codec_lower.contains("aac")
                    || codec_lower.contains("mp4")
                    || codec_lower.contains("m4a")
                {
                    Some("m4a")
                } else if codec_lower.contains("ogg") {
                    Some("ogg")
                } else {
                    None
                };
                if let Some(ext) = ext {
                    hint.with_extension(ext);
                }
            }

            let probed = symphonia::default::get_probe()
                .format(
                    &hint,
                    mss,
                    &FormatOptions::default(),
                    &MetadataOptions::default(),
                )
                .context("Symphonia format probe failed")?;

            let mut format = probed.format;
            let track = format
                .default_track()
                .ok_or_else(|| anyhow!("TIDAL stream had no audio track"))?;
            let decoded_sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
            let decoded_channels = track
                .codec_params
                .channels
                .map(|c| c.count() as u16)
                .unwrap_or(2);
            let mut decoder = symphonia::default::get_codecs()
                .make(&track.codec_params, &DecoderOptions::default())
                .context("failed to build Symphonia decoder")?;

            // ── Pre-loop: passive analysis capture ──────────────────────────────────────
            let mut analysis_sent = false;
            let mut analysis_buf: Vec<f32> = Vec::new();
            let dj_media_ref = dj_analysis_media_ref_for_decode(&config, &job);
            if config.dj_analysis_only && dj_media_ref.is_none() {
                return Ok(());
            }
            let mut dj_analysis_sent = false;
            let mut dj_analysis_buf: Vec<f32> = Vec::new();

            // Per-track stateful resampler. Lazily built on the first packet whose
            // input rate doesn't match the live target rate. Rebuilt whenever the
            // live target rate changes (sample-rate-follow path) or the channel
            // count flips. None when input rate already matches output rate
            // (passthrough).
            let mut resampler: Option<StreamResampler> = None;
            let mut decoded_packets: u64 = 0;
            let mut decoded_samples: u64 = 0;
            // One-shot guard for the buffer-growth warn so the audio thread's
            // CAS signal becomes at most one log line per buffer instance.
            let mut growth_warn_emitted = false;

            loop {
                if shared.stopped.load(Ordering::SeqCst) {
                    return Ok(()); // track was stopped/skipped. Exit cleanly.
                }

                let packet = match format.next_packet() {
                    Ok(p) => p,
                    Err(SymphoniaError::IoError(err)) => {
                        debug!(
                            "Playback decoder EOF: track_id={}, packets={}, samples={}, error={}",
                            shared.track_id, decoded_packets, decoded_samples, err
                        );
                        break;
                    }
                    Err(SymphoniaError::ResetRequired) => {
                        decoder.reset();
                        continue;
                    }
                    Err(err) => return Err(err.into()),
                };

                let decoded = match decoder.decode(&packet) {
                    Ok(d) => d,
                    Err(SymphoniaError::DecodeError(_)) => continue,
                    Err(err) => return Err(err.into()),
                };

                let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sb.copy_interleaved_ref(decoded);

                // Passive analysis tap: capture the first 45s as mono.
                // 45s = a 15s intro skip (PASSIVE_INTRO_SKIP_SEC, dropped in the
                // analysis actor) plus the 30s analysis window. Tracks that end
                // before 45s are flushed with whatever was captured after the
                // decode loop, so nothing is left unanalysed.
                if !analysis_sent {
                    extend_mono_from_interleaved(
                        &mut analysis_buf,
                        sb.samples(),
                        decoded_channels as usize,
                    );
                    if analysis_buf.len() >= decoded_sample_rate as usize * 45 {
                        if let Some(tx) = &config.analysis_tx {
                            let _ = tx.send((
                                shared.track_id,
                                std::mem::take(&mut analysis_buf),
                                decoded_sample_rate,
                            ));
                        }
                        analysis_sent = true;
                    }
                }

                if !dj_analysis_sent && dj_media_ref.is_some() {
                    extend_mono_from_interleaved(
                        &mut dj_analysis_buf,
                        sb.samples(),
                        decoded_channels as usize,
                    );
                    if dj_analysis_buf.len()
                        >= decoded_sample_rate as usize * DJ_ANALYSIS_MAX_SECONDS
                    {
                        send_dj_analysis_job(
                            &config,
                            dj_media_ref.clone(),
                            &job,
                            std::mem::take(&mut dj_analysis_buf),
                            decoded_sample_rate,
                            shared.generation,
                        );
                        dj_analysis_sent = true;
                    }
                }
                if config.dj_analysis_only {
                    decoded_packets += 1;
                    decoded_samples += sb.samples().len() as u64;
                    if dj_analysis_sent {
                        debug!(
                            "DJ analysis-only decode captured profile window: track_id={}, packets={}, samples={}",
                            shared.track_id, decoded_packets, decoded_samples
                        );
                        return Ok(());
                    }
                    continue;
                }

                // Channel-adapt and resample this packet's samples, then push to buffer.
                // Releasing the lock between packets lets the CPAL callback drain freely.
                //
                // Read the target sample rate live from shared state so a runtime
                // `DeviceSwap` with sample-rate-follow can re-target the
                // resampler without restarting the decoder. The cpal callback's
                // PlaybackBuffer mixes any old- and new-rate samples that were
                // already enqueued before the change at the new device's rate;
                // a brief pitch glitch across the swap boundary is acceptable
                // (matches the documented "brief silence is OK" behaviour for
                // sample-rate-follow transitions).
                let live_target_rate = shared.target_sample_rate.load(Ordering::Relaxed).max(1);
                let _ = device_sample_rate; // kept in signature for callers; live rate above is authoritative
                let channelized = adapt_channels(
                    sb.samples(),
                    decoded_channels as usize,
                    device_channels as usize,
                );

                let resampled = if decoded_sample_rate == live_target_rate {
                    // Bit-perfect passthrough. Rates already match.
                    resampler = None;
                    channelized
                } else {
                    let needs_rebuild = match resampler.as_ref() {
                        Some(r) => {
                            r.in_rate != decoded_sample_rate
                                || r.out_rate != live_target_rate
                                || r.channels != device_channels as usize
                        }
                        None => true,
                    };
                    if needs_rebuild {
                        // Fail the track instead of passing through
                        // unresampled: wrong-pitch playback with only a log
                        // line is worse than a visible track error, and the
                        // terminal-error path advances the queue anyway.
                        resampler = Some(
                            StreamResampler::new(
                                decoded_sample_rate,
                                live_target_rate,
                                device_channels as usize,
                            )
                            .with_context(|| {
                                format!(
                                    "resampler init failed ({decoded_sample_rate} -> {live_target_rate} Hz, {device_channels} ch)"
                                )
                            })?,
                        );
                    }
                    match resampler.as_mut() {
                        Some(r) => r.process(&channelized),
                        None => channelized,
                    }
                };

                let buffered_samples = {
                    let mut guard = shared
                        .buffer
                        .lock()
                        .map_err(|_| anyhow!("playback buffer poisoned"))?;
                    guard.samples.extend_from_slice(&resampled);
                    guard.samples.len()
                };
                decoded_packets += 1;
                decoded_samples += resampled.len() as u64;

                if !config.dj_analysis_only {
                    let retain_samples = crate::playback::runtime::shared::samples_from_ms(
                        PLAYBACK_BUFFER_RETAIN_BEHIND_MS,
                        live_target_rate,
                        device_channels,
                    );
                    let compact_threshold = retain_samples.saturating_mul(2);
                    if compact_threshold > 0
                        && buffered_samples.saturating_sub(shared.unread_buffered_samples()?)
                            > compact_threshold
                    {
                        let _ = shared.compact_consumed_buffer(retain_samples)?;
                    }
                    apply_playback_decode_backpressure(&shared, live_target_rate, device_channels)?;
                }

                // Observe the audio-thread's growth signal off the real-time
                // thread. local guard makes it one log per buffer; the audio
                // thread's CAS makes the signal cheap and idempotent.
                if !growth_warn_emitted && shared.growth_warned.load(Ordering::Relaxed) {
                    growth_warn_emitted = true;
                    warn!(
                        "Playback buffer grew past ~200MB: track_id={}, buffered_samples={}, decoded_packets={}",
                        shared.track_id, buffered_samples, decoded_packets
                    );
                }
            }

            // Flush any residual samples held in the resampler at end-of-stream
            // so the final fraction of a chunk doesn't get truncated.
            if let Some(r) = resampler.as_mut() {
                let tail = r.flush();
                if !tail.is_empty() {
                    let mut guard = shared
                        .buffer
                        .lock()
                        .map_err(|_| anyhow!("playback buffer poisoned"))?;
                    guard.samples.extend_from_slice(&tail);
                }
            }

            if !dj_analysis_sent && !dj_analysis_buf.is_empty() {
                send_dj_analysis_job(
                    &config,
                    dj_media_ref,
                    &job,
                    std::mem::take(&mut dj_analysis_buf),
                    decoded_sample_rate,
                    shared.generation,
                );
            }

            // Flush the passive analysis tap for tracks that ended before the
            // 45s capture threshold, so short tracks still get analysed (from
            // the start, since there is not enough audio to skip an intro).
            if !analysis_sent && !analysis_buf.is_empty() {
                if let Some(tx) = &config.analysis_tx {
                    let _ = tx.send((
                        shared.track_id,
                        std::mem::take(&mut analysis_buf),
                        decoded_sample_rate,
                    ));
                }
            }

            // Apply fade-in / fade-out ramps and mark the stream complete.
            let buffer_len = {
                let mut guard = shared
                    .buffer
                    .lock()
                    .map_err(|_| anyhow!("playback buffer poisoned"))?;

                // Fade ramps are applied dynamically in the CPAL callback. No baking needed.

                guard.mark_finished();
                guard.samples.len() as u64
            };
            // total_samples is ABSOLUTE (offset + local count) to match
            // position_samples, which is also ABSOLUTE. For a fresh play the
            // offset is 0 and the two are equal; for a segment-restart engine
            // (option C) the offset is non-zero, so storing LOCAL count here
            // would race position past total and make the fade-out branch in
            // write_output_buffer immediately compute remaining=0 (silence)
            // as soon as crossfade_samples gets armed.
            let offset = shared.position_offset_samples.load(Ordering::Relaxed);
            let absolute_total = offset.saturating_add(buffer_len);
            shared
                .total_samples
                .store(absolute_total, Ordering::Relaxed);
            let total_secs = absolute_total as f64
                / (shared.target_sample_rate.load(Ordering::Relaxed).max(1) as f64
                    * device_channels.max(1) as f64);
            debug!(
                "Playback decoder finished: track_id={}, packets={}, buffered_samples={}, offset_samples={}, total_samples={}, duration_secs={:.3}",
                shared.track_id, decoded_packets, buffer_len, offset, absolute_total, total_secs
            );

            // Notify the runtime loop that this engine's decode is complete.
            // The runtime uses this to start the crossfade stream if the window is already open.
            let _ = shared
                .command_tx
                .send(PlaybackRuntimeCommand::NextDecodeComplete {
                    track_id: shared.track_id,
                    generation: shared.generation,
                });
        }
    }

    Ok(())
}

pub(crate) fn send_dj_analysis_job(
    config: &PlaybackRuntimeConfig,
    media_ref: Option<crate::playback::dj_lookahead::DjMediaRef>,
    job: &PreparedPlaybackJob,
    samples: Vec<f32>,
    sample_rate: u32,
    deadline_generation: u64,
) {
    if !config.dj_engine_enabled {
        return;
    }
    let (Some(tx), Some(media_ref)) = (&config.dj_analysis_tx, media_ref) else {
        return;
    };
    if samples.is_empty() {
        return;
    }

    let analysis_scope_ms = ((samples.len() as u64).saturating_mul(1000)
        / sample_rate.max(1) as u64)
        .min(i64::MAX as u64) as i64;
    let track_id = media_ref
        .track_id()
        .or_else(|| (job.track.id > 0).then_some(job.track.id));
    let key = media_ref.profile_key();
    let sample_count = samples.len();
    let _ = tx.send(DjAnalysisJob {
        track_id,
        queue_item_id: media_ref.queue_item_id(),
        tidal_id: media_ref.tidal_id(),
        media_ref,
        samples,
        sample_rate,
        analysis_scope_ms,
        deadline_generation,
    });
    info!(
        media_ref_kind = %key.media_ref_kind,
        media_ref_id = %key.media_ref_id,
        sample_count,
        sample_rate,
        analysis_scope_ms,
        "DJ analysis job queued"
    );
}

fn dj_analysis_media_ref_for_decode(
    config: &PlaybackRuntimeConfig,
    job: &PreparedPlaybackJob,
) -> Option<crate::playback::dj_lookahead::DjMediaRef> {
    if !config.dj_engine_enabled || !config.dj_analysis_only {
        return None;
    }

    job.dj_media_ref.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Track;
    use crate::playback::dj_lookahead::DjMediaRef;
    use crate::playback::gapless::GaplessPlan;
    use crate::playback::player::{PlaybackSourceRequest, PreparedPlaybackJob};
    use anyhow::Context as _;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    fn test_track(track_id: i64) -> Track {
        Track {
            id: track_id,
            title: format!("Track {track_id}"),
            artist_id: 1,
            artist_name: None,
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: None,
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: None,
            best_source: None,
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "local".to_string(),
            artwork_url: None,
        }
    }

    fn test_job(track_id: i64, media_ref: DjMediaRef) -> PreparedPlaybackJob {
        PreparedPlaybackJob::new(
            test_track(track_id),
            PlaybackSourceRequest::LocalLibrary,
            GaplessPlan::disabled(),
        )
        .with_generation(7)
        .with_dj_media_ref(media_ref)
    }

    fn test_config(
        enabled: bool,
    ) -> (
        PlaybackRuntimeConfig,
        tokio::sync::mpsc::UnboundedReceiver<DjAnalysisJob>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            PlaybackRuntimeConfig::new(reqwest::Client::new(), "", None)
                .with_dj_analysis(enabled, Some(tx)),
            rx,
        )
    }

    fn test_analysis_only_config(
        enabled: bool,
    ) -> (
        PlaybackRuntimeConfig,
        tokio::sync::mpsc::UnboundedReceiver<DjAnalysisJob>,
    ) {
        let (config, rx) = test_config(enabled);
        (config.for_dj_analysis_only(), rx)
    }

    async fn retryable_test_timeout() -> Result<Vec<u8>> {
        tokio::time::timeout(Duration::from_millis(1), std::future::pending::<()>())
            .await
            .context("test retryable timeout")?;
        Ok(Vec::new())
    }

    fn run_dash_prebuffer_test(
        dj_analysis_only: bool,
        outcomes: Vec<Result<Vec<u8>>>,
    ) -> Result<DashPrebuffer> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = AtomicBool::new(false);
        let segments = ["s1", "s2", "s3", "s4"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let mut outcomes = VecDeque::from(outcomes);
        rt.block_on(fetch_dash_prebuffer(
            "init",
            &segments,
            dj_analysis_only,
            &stop,
            move |_url, _segment_index| {
                let outcome = outcomes
                    .pop_front()
                    .unwrap_or_else(|| Err(anyhow!("unexpected fetch")));
                async move { outcome }
            },
        ))
    }

    #[test]
    fn analysis_only_dash_accepts_contiguous_media_prefix() {
        let prebuffer = run_dash_prebuffer_test(
            true,
            vec![
                Ok(vec![0]),
                Ok(vec![1]),
                Err(anyhow!("segment 2 failed")),
                Err(anyhow!("segment 2 failed")),
                Err(anyhow!("segment 2 failed")),
            ],
        )
        .expect("analysis prefix");

        assert_eq!(prebuffer.bytes, vec![0, 1]);
        assert_eq!(prebuffer.fetched_media_segments, 1);
        assert!(prebuffer.ended_after_prefix_failure);
    }

    #[test]
    fn analysis_only_dash_never_skips_failed_media_gap() {
        let prebuffer = run_dash_prebuffer_test(
            true,
            vec![
                Ok(vec![0]),
                Ok(vec![1]),
                Err(anyhow!("segment 2 failed")),
                Err(anyhow!("segment 2 failed")),
                Err(anyhow!("segment 2 failed")),
                Ok(vec![3]),
            ],
        )
        .expect("analysis prefix");

        assert_eq!(prebuffer.bytes, vec![0, 1]);
        assert_eq!(prebuffer.fetched_media_segments, 1);
    }

    #[test]
    fn analysis_only_dash_retries_first_media_segment() {
        let prebuffer = run_dash_prebuffer_test(
            true,
            vec![
                Ok(vec![0]),
                Err(anyhow!("segment 1 failed")),
                Ok(vec![1]),
                Ok(vec![2]),
                Ok(vec![3]),
            ],
        )
        .expect("analysis retry");

        assert_eq!(prebuffer.bytes, vec![0, 1, 2, 3]);
        assert_eq!(prebuffer.fetched_media_segments, 3);
        assert!(!prebuffer.ended_after_prefix_failure);
    }

    #[test]
    fn analysis_only_dash_fails_after_first_media_retries_are_exhausted() {
        let error = run_dash_prebuffer_test(
            true,
            vec![
                Ok(vec![0]),
                Err(anyhow!("segment 1 failed once")),
                Err(anyhow!("segment 1 failed twice")),
                Err(anyhow!("segment 1 failed finally")),
            ],
        )
        .expect_err("first media failure should fail analysis prebuffer");

        assert!(error.to_string().contains("segment 1 failed finally"));
    }

    #[test]
    fn playback_dash_prebuffer_remains_fail_fast() {
        let error = run_dash_prebuffer_test(
            false,
            vec![
                Ok(vec![0]),
                Ok(vec![1]),
                Err(anyhow!("segment 2 failed once")),
                Err(anyhow!("segment 2 failed finally")),
            ],
        )
        .expect_err("playback prebuffer should fail on second media segment");

        assert!(error.to_string().contains("segment 2 failed finally"));
    }

    #[test]
    fn playback_dash_prebuffer_retries_transient_first_media_failure() {
        let prebuffer = run_dash_prebuffer_test(
            false,
            vec![
                Ok(vec![0]),
                Err(anyhow!("segment 1 failed once")),
                Ok(vec![1]),
                Ok(vec![2]),
            ],
        )
        .expect("playback retry");

        assert_eq!(prebuffer.bytes, vec![0, 1, 2]);
        assert_eq!(prebuffer.fetched_media_segments, 2);
        assert!(!prebuffer.ended_after_prefix_failure);
    }

    #[test]
    fn dash_background_fetch_preserves_manifest_order_with_lookahead() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = AtomicBool::new(false);
        let mut sent = Vec::new();

        let summary = rt
            .block_on(fetch_dash_segments_ordered(
                vec!["slow".to_string(), "fast".to_string(), "last".to_string()],
                3,
                2,
                &stop,
                |url, segment_index| async move {
                    if url == "slow" {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Ok(vec![segment_index as u8])
                },
                |bytes| {
                    sent.extend(bytes);
                    true
                },
            ))
            .expect("background fetch");

        assert_eq!(sent, vec![3, 4, 5]);
        assert_eq!(summary.sent_segments, 3);
        assert_eq!(summary.sent_bytes, 3);
        assert!(!summary.stopped);
        assert!(!summary.receiver_closed);
    }

    #[test]
    fn dash_background_fetch_retries_transient_segment_failure() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = AtomicBool::new(false);
        let flaky_attempts = Arc::new(Mutex::new(0usize));
        let mut sent = Vec::new();

        let summary = rt
            .block_on(fetch_dash_segments_ordered(
                vec!["first".to_string(), "flaky".to_string(), "last".to_string()],
                10,
                2,
                &stop,
                {
                    let flaky_attempts = Arc::clone(&flaky_attempts);
                    move |url, segment_index| {
                        let flaky_attempts = Arc::clone(&flaky_attempts);
                        async move {
                            if url == "flaky" {
                                let first_attempt = {
                                    let mut attempts =
                                        flaky_attempts.lock().expect("attempts lock");
                                    *attempts += 1;
                                    *attempts == 1
                                };
                                if first_attempt {
                                    return retryable_test_timeout().await;
                                }
                            }
                            Ok(vec![segment_index as u8])
                        }
                    }
                },
                |bytes| {
                    sent.extend(bytes);
                    true
                },
            ))
            .expect("background retry");

        assert_eq!(sent, vec![10, 11, 12]);
        assert_eq!(*flaky_attempts.lock().expect("attempts lock"), 2);
        assert_eq!(summary.sent_segments, 3);
        assert_eq!(summary.sent_bytes, 3);
        assert!(!summary.stopped);
        assert!(!summary.receiver_closed);
    }

    #[test]
    fn dash_background_fetch_does_not_retry_non_retryable_segment_failure() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = AtomicBool::new(false);
        let attempts = Arc::new(Mutex::new(0usize));

        let error = rt
            .block_on(fetch_dash_segments_ordered(
                vec!["bad".to_string()],
                20,
                1,
                &stop,
                {
                    let attempts = Arc::clone(&attempts);
                    move |_url, _segment_index| {
                        let attempts = Arc::clone(&attempts);
                        async move {
                            let mut attempts = attempts.lock().expect("attempts lock");
                            *attempts += 1;
                            Err(anyhow!("permanent segment failure"))
                        }
                    }
                },
                |_bytes| panic!("failed segment must not send bytes"),
            ))
            .expect_err("permanent failure");

        assert!(error.to_string().contains("permanent segment failure"));
        assert_eq!(*attempts.lock().expect("attempts lock"), 1);
    }

    #[test]
    fn dash_background_fetch_stops_during_retry_backoff() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(Mutex::new(0usize));

        let summary = rt
            .block_on(fetch_dash_segments_ordered(
                vec!["flaky".to_string()],
                30,
                1,
                stop.as_ref(),
                {
                    let attempts = Arc::clone(&attempts);
                    let stop = Arc::clone(&stop);
                    move |_url, _segment_index| {
                        let attempts = Arc::clone(&attempts);
                        let stop = Arc::clone(&stop);
                        async move {
                            {
                                let mut attempts = attempts.lock().expect("attempts lock");
                                *attempts += 1;
                            }
                            stop.store(true, Ordering::Relaxed);
                            retryable_test_timeout().await
                        }
                    }
                },
                |_bytes| panic!("stopped download must not send bytes"),
            ))
            .expect("stopped during retry");

        assert_eq!(
            summary,
            DashBackgroundFetchSummary {
                sent_segments: 0,
                sent_bytes: 0,
                slowest_fetch_ms: 0,
                stopped: true,
                receiver_closed: false,
            }
        );
        assert_eq!(*attempts.lock().expect("attempts lock"), 1);
    }

    #[test]
    fn dash_background_fetch_honors_stop_before_fetching() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = AtomicBool::new(true);

        let summary = rt
            .block_on(fetch_dash_segments_ordered(
                vec!["one".to_string()],
                1,
                4,
                &stop,
                |_url, _segment_index| async { Ok(vec![1]) },
                |_bytes| panic!("stopped download must not send bytes"),
            ))
            .expect("stopped fetch");

        assert_eq!(
            summary,
            DashBackgroundFetchSummary {
                sent_segments: 0,
                sent_bytes: 0,
                slowest_fetch_ms: 0,
                stopped: true,
                receiver_closed: false,
            }
        );
    }

    #[test]
    fn dash_background_fetch_stops_when_receiver_closes() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stop = AtomicBool::new(false);
        let mut sent = Vec::new();

        let summary = rt
            .block_on(fetch_dash_segments_ordered(
                vec!["one".to_string(), "two".to_string(), "three".to_string()],
                7,
                2,
                &stop,
                |_url, segment_index| async move { Ok(vec![segment_index as u8]) },
                |bytes| {
                    sent.extend(bytes);
                    false
                },
            ))
            .expect("closed receiver");

        assert_eq!(sent, vec![7]);
        assert_eq!(summary.sent_segments, 0);
        assert_eq!(summary.sent_bytes, 1);
        assert!(!summary.stopped);
        assert!(summary.receiver_closed);
    }

    #[test]
    fn decode_backpressure_uses_unread_samples_only() {
        let high_water = decoded_output_samples_for_secs(45, 48_000, 2);

        assert!(!should_backpressure_decode(high_water, high_water));
        assert!(should_backpressure_decode(high_water + 1, high_water));
    }

    #[test]
    fn playback_decoder_does_not_capture_dj_analysis() {
        let (config, _rx) = test_config(true);
        let job = test_job(1, DjMediaRef::LibraryTrack { track_id: 1 });

        assert!(dj_analysis_media_ref_for_decode(&config, &job).is_none());
    }

    #[test]
    fn analysis_only_decoder_captures_dj_analysis() {
        let (config, _rx) = test_analysis_only_config(true);
        let job = test_job(1, DjMediaRef::LibraryTrack { track_id: 1 });

        assert_eq!(
            dj_analysis_media_ref_for_decode(&config, &job),
            Some(DjMediaRef::LibraryTrack { track_id: 1 })
        );
    }

    #[test]
    fn dj_analysis_not_sent_when_engine_disabled() {
        let (config, mut rx) = test_config(false);
        let job = test_job(1, DjMediaRef::LibraryTrack { track_id: 1 });

        send_dj_analysis_job(
            &config,
            job.dj_media_ref.clone(),
            &job,
            vec![0.0; 128],
            48_000,
            7,
        );

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn analysis_only_decoder_sends_library_profile_key() {
        let (config, mut rx) = test_analysis_only_config(true);
        let job = test_job(1, DjMediaRef::LibraryTrack { track_id: 1 });

        send_dj_analysis_job(
            &config,
            job.dj_media_ref.clone(),
            &job,
            vec![0.0; 128],
            48_000,
            7,
        );

        let sent = rx.try_recv().expect("dj job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "library_track");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "1");
        assert_eq!(sent.track_id, Some(1));
    }

    #[test]
    fn analysis_only_decoder_sends_tidal_profile_key() {
        let (config, mut rx) = test_analysis_only_config(true);
        let job = test_job(
            10,
            DjMediaRef::TidalTrack {
                tidal_id: 99,
                track_id: Some(10),
            },
        );

        send_dj_analysis_job(
            &config,
            job.dj_media_ref.clone(),
            &job,
            vec![0.0; 128],
            48_000,
            7,
        );

        let sent = rx.try_recv().expect("dj job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "tidal_track");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "99");
        assert_eq!(sent.tidal_id, Some(99));
        assert_eq!(sent.track_id, Some(10));
    }

    #[test]
    fn analysis_only_decoder_does_not_use_synthetic_negative_track_id() {
        let (config, mut rx) = test_analysis_only_config(true);
        let job = test_job(
            -123,
            DjMediaRef::TidalTrack {
                tidal_id: 99,
                track_id: None,
            },
        );

        send_dj_analysis_job(
            &config,
            job.dj_media_ref.clone(),
            &job,
            vec![0.0; 128],
            48_000,
            7,
        );

        let sent = rx.try_recv().expect("dj job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "tidal_track");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "99");
        assert_eq!(sent.tidal_id, Some(99));
        assert_eq!(sent.track_id, None);
    }

    #[test]
    fn analysis_only_decoder_sends_queue_item_profile_key_when_unresolved() {
        let (config, mut rx) = test_analysis_only_config(true);
        let job = test_job(
            10,
            DjMediaRef::PendingQueueItem {
                queue_item_id: 44,
                pending_artist: "Artist".to_string(),
                pending_title: "Title".to_string(),
                tidal_id_hint: None,
            },
        );

        send_dj_analysis_job(
            &config,
            job.dj_media_ref.clone(),
            &job,
            vec![0.0; 128],
            48_000,
            7,
        );

        let sent = rx.try_recv().expect("dj job");
        assert_eq!(sent.media_ref.profile_key().media_ref_kind, "queue_item");
        assert_eq!(sent.media_ref.profile_key().media_ref_id, "44");
        assert_eq!(sent.queue_item_id, Some(44));
    }
}
