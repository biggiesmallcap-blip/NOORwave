use crate::services::acrcloud::{AcrCloudClient, HmacSha1};
use base64::{Engine, engine::general_purpose};
use chrono::Utc;
use hmac::Mac;
use serde::{Deserialize, Serialize};

/// Generate a WAV file header (44 bytes)
pub fn encode_wav_header(
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    data_size: u32,
) -> Vec<u8> {
    let mut header = Vec::with_capacity(44);
    // RIFF header
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data_size).to_le_bytes());
    header.extend_from_slice(b"WAVE");
    // fmt chunk
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    header.extend_from_slice(&1u16.to_le_bytes()); // PCM
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    header.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * bits_per_sample / 8;
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data chunk
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_size.to_le_bytes());
    header
}

/// Convert mono f32 samples to 16-bit PCM WAV bytes
pub fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32; // 16-bit = 2 bytes per sample
    let mut wav = encode_wav_header(sample_rate, 1, 16, data_size);
    for &s in samples {
        let sample = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Sign the ACRCloud request using HMAC-SHA1
fn sign_request(
    access_secret: &str,
    method: &str,
    uri: &str,
    key: &str,
    data_type: &str,
    timestamp: i64,
) -> String {
    let sign_string = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method, uri, key, data_type, 1, timestamp
    );

    let mut mac = HmacSha1::new_from_slice(access_secret.as_bytes()).unwrap();
    mac.update(sign_string.as_bytes());
    let result = mac.finalize();
    general_purpose::STANDARD.encode(result.into_bytes())
}

#[derive(Debug, Deserialize)]
pub struct AcrCloudResponse {
    pub status: AcrCloudStatus,
    pub metadata: Option<AcrCloudMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct AcrCloudStatus {
    pub msg: String,
    pub code: i64,
}

#[derive(Debug, Deserialize)]
pub struct AcrCloudMetadata {
    pub music: Option<Vec<AcrCloudTrack>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AcrCloudTrack {
    pub title: Option<String>,
    pub artists: Option<Vec<AcrCloudArtist>>,
    pub album: Option<AcrCloudAlbum>,
    pub release_date: Option<String>,
    pub external_metadata: Option<AcrCloudExternalMetadata>,
    pub score: i64,
    pub sample_start_time_offset_ms: Option<i64>,
    pub sample_end_time_offset_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AcrCloudArtist {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AcrCloudAlbum {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AcrCloudExternalMetadata {
    pub isrc: Option<String>,
}

/// Outcome of an ACRCloud identify request. Network/HTTP errors never bubble
/// up as `Err`; instead callers get `NoMatch` (so the scan continues) or
/// `RateLimited` (so the scan backs off).
#[derive(Debug)]
pub enum IdentifyResult {
    /// A track was matched.
    Match(AcrCloudTrack),
    /// Request completed but no match (or a recoverable network/HTTP error).
    NoMatch,
    /// ACRCloud returned HTTP 429 — caller should back off.
    RateLimited,
}

/// Identify a track sample via ACRCloud API.
///
/// This function NEVER returns `Err` for network-layer problems. Timeouts,
/// connection errors, 5xx responses, and JSON decode errors all map to
/// `IdentifyResult::NoMatch` with a `warn!` log so the scanner can continue
/// to the next track. HTTP 429 maps to `IdentifyResult::RateLimited` so the
/// caller can sleep before trying again.
pub async fn identify_track(
    client: &AcrCloudClient,
    samples: &[f32],
    sample_rate: u32,
) -> IdentifyResult {
    // Rate limit: 1 req/3s
    let permit = match client.rate_limit_semaphore.acquire().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("ACRCloud semaphore closed: {}", e);
            return IdentifyResult::NoMatch;
        }
    };

    let timestamp = Utc::now().timestamp();
    let wav_data = samples_to_wav(samples, sample_rate);

    // Build multipart form
    let boundary = "----NOORwaveACRCloudBoundary";
    let mut body = Vec::new();

    // Signature
    let signature = sign_request(
        &client.config.access_secret,
        "POST",
        "/v1/identify",
        &client.config.access_key,
        "audio",
        timestamp,
    );

    // Form fields
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"access_key\"\r\n\r\n");
    body.extend_from_slice(client.config.access_key.as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"sample_bytes\"\r\n\r\n");
    body.extend_from_slice(&wav_data);
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"timestamp\"\r\n\r\n");
    body.extend_from_slice(timestamp.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"signature\"\r\n\r\n");
    body.extend_from_slice(signature.as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"data_type\"\r\n\r\naudio\r\n");

    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"sample_rate\"\r\n\r\n");
    body.extend_from_slice(sample_rate.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"audio_format\"\r\n\r\nwav\r\n");

    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let host = match client.config.region.as_str() {
        "eu-west-1" => "identify-eu-west-1.acrcloud.com",
        "us-east-1" => "identify-us-east-1.acrcloud.com",
        _ => "identify-eu-west-1.acrcloud.com",
    };

    let url = format!("https://{}/v1/identify", host);

    let response = client
        .http_client
        .post(&url)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(body)
        .send()
        .await;

    // Drop the permit so the semaphore is released before we await sleep
    drop(permit);

    match response {
        Ok(resp) => {
            let status = resp.status();
            if status.as_u16() == 429 {
                tracing::warn!("ACRCloud rate limited (429) — signalling scanner to back off");
                return IdentifyResult::RateLimited;
            }
            if status.is_server_error() {
                tracing::warn!("ACRCloud server error ({}) — skipping track", status);
                return IdentifyResult::NoMatch;
            }
            if !status.is_success() {
                tracing::warn!(
                    "ACRCloud API non-success status {} — skipping track",
                    status
                );
                return IdentifyResult::NoMatch;
            }
            let data: AcrCloudResponse = match resp.json().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("ACRCloud response decode failed: {} — skipping track", e);
                    return IdentifyResult::NoMatch;
                }
            };
            if data.status.code == 0
                && let Some(meta) = data.metadata
                && let Some(tracks) = meta.music
                && let Some(track) = tracks.into_iter().next()
            {
                return IdentifyResult::Match(track);
            }
            IdentifyResult::NoMatch
        }
        Err(e) => {
            // Covers timeouts, connection refused, DNS failures, TLS errors, etc.
            if e.is_timeout() {
                tracing::warn!("ACRCloud request timed out — skipping track");
            } else if e.is_connect() {
                tracing::warn!("ACRCloud connection failed: {} — skipping track", e);
            } else {
                tracing::warn!("ACRCloud request failed: {} — skipping track", e);
            }
            IdentifyResult::NoMatch
        }
    }
}
