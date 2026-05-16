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
use crate::services::tidal::stream::resolve_stream;
use anyhow::{Context, Result, anyhow};
use futures::StreamExt as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::runtime::Builder as TokioRuntimeBuilder;
use tracing::{debug, warn};

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
        PlaybackSourceRequest::TidalStream(request) => {
            // ── Step 1: resolve the stream URL (async, needs a mini tokio runtime) ──────────
            let rt = TokioRuntimeBuilder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create async runtime for TIDAL stream fetch")?;

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            let stream_info = rt.block_on(async {
                resolve_stream(&config.http_client, &config.access_token, &request).await
            })?;
            debug!(
                "TIDAL runtime stream resolved: track_id={}, quality={}, codec={}, sample_rate={:?}, bit_depth={:?}, dash_segments={}",
                shared.track_id,
                stream_info.audio_quality,
                stream_info.codec,
                stream_info.sample_rate,
                stream_info.bit_depth,
                stream_info.segment_urls.len()
            );

            if shared.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }

            // ── Step 2: stream bytes from the CDN in a background thread ─────────────────────
            // Using a bounded channel limits peak memory: at most 32 in-flight chunks of ~64KB
            // each ≈ 2 MB of download head room while the decoder works.
            // A separate one-shot channel carries the Content-Length from the response headers
            // so StreamPipe can report byte_len() correctly, allowing Symphonia's MSS to
            // translate SeekFrom::End into an absolute position rather than failing.
            let is_dash_stream = !stream_info.segment_urls.is_empty();
            let mut dash_initial = Vec::new();
            let mut remaining_segment_urls = stream_info.segment_urls.clone();
            let initial_media_segments = dash_initial_media_count(stream_info.segment_urls.len());
            if initial_media_segments > 0 {
                let prebuffer_stop = shared.stop_flag();
                dash_initial = rt
                    .block_on(async {
                        if prebuffer_stop.load(Ordering::Relaxed) {
                            return anyhow::Ok(Vec::new());
                        }
                        let mut bytes =
                            append_stream_bytes(&config.http_client, &stream_info.url, 0).await?;
                        if prebuffer_stop.load(Ordering::Relaxed) {
                            return anyhow::Ok(bytes);
                        }
                        for (idx, segment_url) in stream_info
                            .segment_urls
                            .iter()
                            .take(initial_media_segments)
                            .enumerate()
                        {
                            if prebuffer_stop.load(Ordering::Relaxed) {
                                return anyhow::Ok(bytes);
                            }
                            bytes.extend(
                                append_stream_bytes(&config.http_client, segment_url, idx + 1)
                                    .await?,
                            );
                        }
                        anyhow::Ok(bytes)
                    })
                    .context("DASH stream prebuffer failed")?;
                remaining_segment_urls.drain(0..initial_media_segments);
                debug!(
                    "TIDAL DASH prebuffer ready: track_id={}, initial_segments={}, remaining_segments={}",
                    shared.track_id,
                    initial_media_segments,
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
                                let mut sent_segments = 0usize;
                                let mut sent_bytes = 0usize;
                                for (idx, seg_url) in segment_urls.into_iter().enumerate() {
                                    if download_stop.load(Ordering::Relaxed) {
                                        warn!(
                                            "TIDAL DASH download cancelled: track_id={}, sent_segments={}, total_remaining_segments={}",
                                            download_track_id, sent_segments, total_segments
                                        );
                                        break;
                                    }
                                    let bytes = append_stream_bytes(
                                        &http,
                                        &seg_url,
                                        initial_media_segments + idx + 1,
                                    )
                                    .await?;
                                    if download_stop.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    sent_bytes += bytes.len();
                                    if chunk_tx.send(Some(bytes)).is_err() {
                                        warn!(
                                            "TIDAL DASH download stopped early: track_id={}, sent_segments={}, total_remaining_segments={}",
                                            download_track_id,
                                            sent_segments,
                                            total_segments
                                        );
                                        break;
                                    }
                                    sent_segments += 1;
                                    if sent_segments <= 3
                                        || sent_segments == total_segments
                                        || sent_segments % 10 == 0
                                    {
                                        debug!(
                                            "TIDAL DASH segment queued: track_id={}, sent_segments={}, total_remaining_segments={}, fetch_window={}",
                                            download_track_id,
                                            sent_segments,
                                            total_segments,
                                            dash_background_fetch_window()
                                        );
                                    }
                                }
                                debug!(
                                    "TIDAL DASH download EOF: track_id={}, sent_segments={}, total_remaining_segments={}, bytes={}",
                                    download_track_id,
                                    sent_segments,
                                    total_segments,
                                    sent_bytes
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

            // Per-track stateful resampler. Lazily built on the first packet whose
            // input rate doesn't match the live target rate. Rebuilt whenever the
            // live target rate changes (sample-rate-follow path) or the channel
            // count flips. None when input rate already matches output rate
            // (passthrough).
            let mut resampler: Option<StreamResampler> = None;
            let mut decoded_packets: u64 = 0;
            let mut decoded_samples: u64 = 0;

            loop {
                if shared.stopped.load(Ordering::SeqCst) {
                    return Ok(()); // track was stopped/skipped — exit cleanly
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

                // ── Passive analysis tap: capture first 30 seconds as mono ──────────────
                if !analysis_sent {
                    extend_mono_from_interleaved(
                        &mut analysis_buf,
                        sb.samples(),
                        decoded_channels as usize,
                    );
                    if analysis_buf.len() >= decoded_sample_rate as usize * 30 {
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
                    // Bit-perfect passthrough — rates already match.
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
                        match StreamResampler::new(
                            decoded_sample_rate,
                            live_target_rate,
                            device_channels as usize,
                        ) {
                            Ok(r) => resampler = Some(r),
                            Err(e) => {
                                warn!(
                                    "Resampler init failed ({decoded_sample_rate} -> {live_target_rate} Hz, {} ch): {e}; passing through unresampled (pitch will be wrong)",
                                    device_channels
                                );
                                resampler = None;
                            }
                        }
                    }
                    match resampler.as_mut() {
                        Some(r) => r.process(&channelized),
                        None => channelized,
                    }
                };

                let mut guard = shared
                    .buffer
                    .lock()
                    .map_err(|_| anyhow!("playback buffer poisoned"))?;
                guard.samples.extend_from_slice(&resampled);
                decoded_packets += 1;
                decoded_samples += resampled.len() as u64;
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

            // Apply fade-in / fade-out ramps and mark the stream complete.
            let total = {
                let mut guard = shared
                    .buffer
                    .lock()
                    .map_err(|_| anyhow!("playback buffer poisoned"))?;

                // Fade ramps are applied dynamically in the CPAL callback — no baking needed.

                guard.mark_finished();
                guard.samples.len() as u64
            };
            shared.total_samples.store(total, Ordering::Relaxed);
            let total_secs = total as f64
                / (shared.target_sample_rate.load(Ordering::Relaxed).max(1) as f64
                    * device_channels.max(1) as f64);
            debug!(
                "Playback decoder finished: track_id={}, packets={}, buffered_samples={}, duration_secs={:.3}",
                shared.track_id, decoded_packets, total, total_secs
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
