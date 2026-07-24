use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use thiserror::Error;

const TIDAL_API_URL: &str = "https://api.tidal.com/v1";
pub const DEFAULT_AUDIO_QUALITY: &str = "LOSSLESS";
const DEFAULT_PLAYBACK_MODE: &str = "STREAM";
const DEFAULT_ASSET_PRESENTATION: &str = "FULL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRequest {
    pub track_id: i64,
    pub audio_quality: String,
    pub playback_mode: String,
    pub asset_presentation: String,
}

impl StreamRequest {
    pub fn new(track_id: i64, audio_quality: impl Into<String>) -> Self {
        Self {
            track_id,
            audio_quality: audio_quality.into(),
            playback_mode: DEFAULT_PLAYBACK_MODE.to_string(),
            asset_presentation: DEFAULT_ASSET_PRESENTATION.to_string(),
        }
    }
}

fn track_playback_info_url(
    request: &StreamRequest,
) -> std::result::Result<String, StreamResolveError> {
    let base = format!(
        "{}/tracks/{}/playbackinfopostpaywall",
        TIDAL_API_URL, request.track_id
    );
    let mut url =
        reqwest::Url::parse(&base).map_err(|error| StreamResolveError::RequestFailed {
            message: format!("failed to build TIDAL playback URL: {error}"),
        })?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("audioquality", &request.audio_quality);
        query.append_pair("playbackmode", &request.playback_mode);
        query.append_pair("assetpresentation", &request.asset_presentation);
    }
    Ok(url.to_string())
}

fn video_playback_info_url(
    video_id: i64,
    quality: &str,
) -> std::result::Result<String, StreamResolveError> {
    let base = format!(
        "{}/videos/{}/playbackinfopostpaywall",
        TIDAL_API_URL, video_id
    );
    let mut url =
        reqwest::Url::parse(&base).map_err(|error| StreamResolveError::RequestFailed {
            message: format!("failed to build TIDAL video playback URL: {error}"),
        })?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("videoquality", quality);
        query.append_pair("playbackmode", DEFAULT_PLAYBACK_MODE);
        query.append_pair("assetpresentation", DEFAULT_ASSET_PRESENTATION);
    }
    Ok(url.to_string())
}

// Clone so a route that has just resolved a stream can hand the result to the
// playback job instead of making the decoder thread resolve it a second time.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamInfo {
    pub url: String,
    pub segment_urls: Vec<String>,
    #[serde(default)]
    pub segment_offsets_ms: Vec<u64>,
    #[serde(rename = "trackId")]
    #[allow(dead_code)]
    pub track_id: i64,
    #[serde(rename = "audioQuality")]
    pub audio_quality: String,
    pub codec: String,
    #[serde(rename = "sampleRate")]
    pub sample_rate: Option<i32>,
    #[serde(rename = "bitDepth")]
    pub bit_depth: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoStreamInfo {
    pub hls_manifest_url: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub video_quality: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioQualityAgreement {
    Agreed,
    Changed,
    Unreported,
}

fn resolved_audio_quality(
    response: &serde_json::Value,
    requested_quality: &str,
) -> (String, AudioQualityAgreement) {
    match response
        .get("audioQuality")
        .and_then(|value| value.as_str())
    {
        Some(returned_quality) if returned_quality == requested_quality => {
            (returned_quality.to_string(), AudioQualityAgreement::Agreed)
        }
        Some(returned_quality) => (returned_quality.to_string(), AudioQualityAgreement::Changed),
        None => (
            requested_quality.to_string(),
            AudioQualityAgreement::Unreported,
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamCodec {
    Flac,
    Aac,
    Mp3,
    Ogg,
    Unknown,
}

impl StreamInfo {
    pub fn codec_kind(&self) -> StreamCodec {
        let codec = self.codec.to_ascii_lowercase();
        match codec.as_str() {
            // TIDAL's "Broadcast Transport Stream" is a manifest wrapper around
            // FLAC for LOSSLESS / HI_RES_LOSSLESS quality. Symphonia decodes the
            // inner FLAC, and the underlying audio supports gapless seeks the
            // same way raw FLAC does — so treat it as FLAC for gapless gating.
            "audio/flac" | "flac" | "application/vnd.tidal.bts" => StreamCodec::Flac,
            "audio/aac" | "aac" | "audio/mp4" | "audio/m4a" => StreamCodec::Aac,
            "audio/mpeg" | "audio/mp3" | "mp3" => StreamCodec::Mp3,
            "audio/ogg" | "ogg" => StreamCodec::Ogg,
            _ if codec.starts_with("mp4a.") => StreamCodec::Aac,
            _ => StreamCodec::Unknown,
        }
    }

    #[allow(dead_code)]
    pub fn is_lossless(&self) -> bool {
        matches!(self.codec_kind(), StreamCodec::Flac)
    }

    pub fn supports_gapless(&self) -> bool {
        matches!(
            self.codec_kind(),
            StreamCodec::Flac | StreamCodec::Aac | StreamCodec::Mp3
        )
    }

    #[allow(dead_code)]
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.sample_rate
            .and_then(|rate| (rate > 0).then_some(rate as u32))
    }

    #[allow(dead_code)]
    pub fn bit_depth_bits(&self) -> Option<u16> {
        self.bit_depth
            .and_then(|depth| (depth > 0).then_some(depth as u16))
    }
}

#[derive(Debug, Error)]
pub enum StreamResolveError {
    #[error("TIDAL session expired while resolving stream: {message}")]
    SessionExpired { message: String },
    #[error("TIDAL session refresh failed: {message}")]
    #[allow(dead_code)]
    SessionRefreshFailed { message: String },
    #[error("TIDAL playback request was rejected: {message}")]
    StreamRejected { message: String },
    #[error("TIDAL playback response could not be parsed: {message}")]
    ResponseParseFailed { message: String },
    #[error("TIDAL playback manifest could not be decoded: {message}")]
    ManifestDecodeFailed { message: String },
    #[error("TIDAL playback manifest could not be parsed: {message}")]
    ManifestParseFailed { message: String },
    #[error("TIDAL playback manifest did not contain a URL")]
    MissingStreamUrl,
    #[error("TIDAL playback response did not contain a manifest")]
    MissingManifest,
    #[error("TIDAL playback request failed: {message}")]
    RequestFailed { message: String },
    #[error("TIDAL playback request returned {status}: {body}")]
    UpstreamHttp {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl StreamResolveError {
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::SessionExpired { .. })
    }

    #[allow(dead_code)]
    pub fn is_stream_rejected(&self) -> bool {
        matches!(self, Self::StreamRejected { .. })
    }

    pub fn is_asset_not_ready(&self) -> bool {
        match self {
            Self::StreamRejected { message } => asset_not_ready_body(message),
            Self::UpstreamHttp { body, .. } => asset_not_ready_body(body),
            _ => false,
        }
    }
}

fn session_expired_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("valid session")
        || lower.contains("user does not have a valid session")
        || lower.contains("session expired")
        || lower.contains("\"substatus\":6001")
}

fn asset_not_ready_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("asset is not ready for playback") || lower.contains("\"substatus\":4005")
}

fn xml_entity_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

// Parses ISO 8601 duration strings of the form PT[M]M[S]S (e.g. PT3M14.093S, PT194S).
fn parse_iso_duration(s: &str) -> Option<f64> {
    let s = s.strip_prefix("PT")?;
    let (minutes, rest) = if let Some(m_pos) = s.find('M') {
        let m: f64 = s[..m_pos].parse().ok()?;
        (m, &s[m_pos + 1..])
    } else {
        (0.0, s)
    };
    let seconds: f64 = rest
        .strip_suffix('S')
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    Some(minutes * 60.0 + seconds)
}

// Extracts the audio codec string from a DASH manifest's codecs= attribute (e.g. "flac", "mp4a.40.2").
fn extract_dash_codec(xml: &str) -> String {
    static CODECS_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\bcodecs="([^"]+)""#).unwrap());
    CODECS_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "audio/mp4".to_string())
}

fn extract_bts_codec(manifest: &serde_json::Value, manifest_mime: &str) -> String {
    manifest
        .get("codecs")
        .and_then(|value| value.as_str())
        .or_else(|| manifest.get("mimeType").and_then(|value| value.as_str()))
        .unwrap_or(manifest_mime)
        .to_string()
}

fn parse_video_expiry(resp: &serde_json::Value) -> Option<DateTime<Utc>> {
    for key in ["expiresAt", "expires_at", "expirationDate", "expiration"] {
        if let Some(raw) = resp.get(key).and_then(serde_json::Value::as_str)
            && let Ok(dt) = DateTime::parse_from_rfc3339(raw)
        {
            return Some(dt.with_timezone(&Utc));
        }
    }
    None
}

fn find_hls_url(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                Some(trimmed.to_string())
            } else {
                None
            }
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_hls_url),
        serde_json::Value::Object(map) => {
            for key in [
                "hlsUrl",
                "hls_url",
                "manifestUrl",
                "manifest_url",
                "streamUrl",
                "stream_url",
                "url",
            ] {
                if let Some(url) = map.get(key).and_then(find_hls_url) {
                    return Some(url);
                }
            }
            for value in map.values() {
                if let Some(url) = find_hls_url(value) {
                    return Some(url);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_video_manifest_string(manifest: &str) -> Result<String, StreamResolveError> {
    let trimmed = manifest.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }

    let manifest_bytes = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .map_err(|error| StreamResolveError::ManifestDecodeFailed {
            message: error.to_string(),
        })?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| {
        StreamResolveError::ManifestParseFailed {
            message: format!("video manifest is not valid UTF-8: {error}"),
        }
    })?;

    let manifest_text = manifest_text.trim();
    if manifest_text.starts_with("http://") || manifest_text.starts_with("https://") {
        return Ok(manifest_text.to_string());
    }

    let manifest_json: serde_json::Value =
        serde_json::from_str(manifest_text).map_err(|error| {
            StreamResolveError::ManifestParseFailed {
                message: format!("failed to parse video manifest JSON: {error}"),
            }
        })?;
    find_hls_url(&manifest_json).ok_or(StreamResolveError::MissingStreamUrl)
}

fn parse_video_stream_info(
    resp: &serde_json::Value,
    requested_quality: &str,
) -> Result<VideoStreamInfo, StreamResolveError> {
    let hls_manifest_url = if let Some(manifest) = resp.get("manifest").and_then(|v| v.as_str()) {
        parse_video_manifest_string(manifest)?
    } else {
        find_hls_url(resp).ok_or(StreamResolveError::MissingStreamUrl)?
    };

    Ok(VideoStreamInfo {
        hls_manifest_url,
        expires_at: parse_video_expiry(resp),
        video_quality: resp
            .get("videoQuality")
            .or_else(|| resp.get("video_quality"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(requested_quality)
            .to_string(),
    })
}

fn redact_tidal_stream_body(raw: &str) -> String {
    const MAX_LEN: usize = 900;
    let redacted = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut value) => {
            redact_tidal_stream_value(&mut value);
            value.to_string()
        }
        Err(_) => "unparseable TIDAL stream response body".to_string(),
    };
    if redacted.len() > MAX_LEN {
        let mut end = MAX_LEN;
        while !redacted.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &redacted[..end])
    } else {
        redacted
    }
}

fn redact_tidal_stream_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if is_sensitive_tidal_stream_key(key) {
                    *value = serde_json::Value::String("<redacted>".to_string());
                } else {
                    redact_tidal_stream_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_tidal_stream_value(value);
            }
        }
        _ => {}
    }
}

fn is_sensitive_tidal_stream_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "manifest"
            | "manifesturl"
            | "streamurl"
            | "url"
            | "urls"
            | "hlsurl"
            | "dashurl"
            | "licenseurl"
    )
}

// Parses a DASH SegmentTemplate manifest and returns
// (init_url, vec_of_segment_urls, vec_of_segment_start_ms).
// `segment_start_ms[i]` is the playback time offset of `segment_urls[i]` from
// track start, in milliseconds (derived from the manifest timescale).
// Handles both duration= attribute (uniform segments) and <SegmentTimeline> (variable).
fn parse_dash_segment_template(
    xml: &str,
) -> Result<(String, Vec<String>, Vec<u64>), StreamResolveError> {
    fn parse_err(msg: impl Into<String>) -> StreamResolveError {
        StreamResolveError::ManifestParseFailed {
            message: msg.into(),
        }
    }

    static MPD_DURATION_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"mediaPresentationDuration="([^"]+)""#).unwrap()
    });
    static INIT_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"initialization="([^"]+)""#).unwrap());
    static MEDIA_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\bmedia="([^"]+)""#).unwrap());
    static TIMESCALE_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"timescale="(\d+)""#).unwrap());
    static SEG_DUR_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\bduration="(\d+)""#).unwrap());
    static START_NUM_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"startNumber="(\d+)""#).unwrap());
    static S_ELEM_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"<S\s[^>]*>"#).unwrap());
    static S_R_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\br="(-?\d+)""#).unwrap());
    static S_D_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\bd="(\d+)""#).unwrap());
    static S_T_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\bt="(\d+)""#).unwrap());

    let duration_str = MPD_DURATION_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| parse_err("DASH manifest missing mediaPresentationDuration"))?;
    let total_secs = parse_iso_duration(duration_str).ok_or_else(|| {
        parse_err(format!(
            "DASH manifest unparseable duration: {duration_str}"
        ))
    })?;

    let init_url = INIT_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .map(|m| xml_entity_decode(m.as_str()))
        .ok_or_else(|| parse_err("DASH manifest missing initialization URL"))?;
    let media_template = MEDIA_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .map(|m| xml_entity_decode(m.as_str()))
        .ok_or_else(|| parse_err("DASH manifest missing media URL template"))?;
    let timescale: u64 = TIMESCALE_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .ok_or_else(|| parse_err("DASH manifest missing timescale"))?;
    let start_number: u64 = START_NUM_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(1);
    let total_ts = (total_secs * timescale as f64).ceil() as u64;

    let (segment_urls, segment_offsets_ms): (Vec<String>, Vec<u64>) = if S_ELEM_RE.is_match(xml) {
        let entries: Vec<(Option<u64>, u64, i64)> = S_ELEM_RE
            .captures_iter(xml)
            .filter_map(|cap| {
                let elem = cap.get(0).map_or("", |m| m.as_str());
                let duration = S_D_RE
                    .captures(elem)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<u64>().ok())?;
                let start_time = S_T_RE
                    .captures(elem)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<u64>().ok());
                let repeat = S_R_RE
                    .captures(elem)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<i64>().ok())
                    .unwrap_or(0);
                Some((start_time, duration, repeat))
            })
            .collect();

        let mut urls = Vec::new();
        let mut offsets_ms = Vec::new();
        let mut current_time = entries.first().and_then(|entry| entry.0).unwrap_or(0);
        let mut current_number = start_number;

        for (idx, (start_time, duration, repeat)) in entries.iter().copied().enumerate() {
            if let Some(start_time) = start_time {
                current_time = start_time;
            }

            let repeat_count = if repeat >= 0 {
                repeat as u64 + 1
            } else {
                let end_time = entries
                    .get(idx + 1)
                    .and_then(|entry| entry.0)
                    .unwrap_or(total_ts);
                if end_time <= current_time {
                    1
                } else {
                    (end_time - current_time).div_ceil(duration)
                }
            };

            for _ in 0..repeat_count {
                urls.push(fill_dash_template(
                    &media_template,
                    current_number,
                    current_time,
                ));
                offsets_ms.push(current_time.saturating_mul(1000) / timescale);
                current_number += 1;
                current_time = current_time.saturating_add(duration);
            }
        }

        (urls, offsets_ms)
    } else if let Some(seg_dur) = SEG_DUR_RE
        .captures(xml)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok())
    {
        let segment_count = total_ts.div_ceil(seg_dur);
        (0..segment_count)
            .map(|idx| {
                let number = start_number + idx;
                let time = idx * seg_dur;
                let offset_ms = time.saturating_mul(1000) / timescale;
                (fill_dash_template(&media_template, number, time), offset_ms)
            })
            .unzip()
    } else {
        (Vec::new(), Vec::new())
    };

    if segment_urls.is_empty() {
        return Err(parse_err(
            "DASH manifest: could not determine segment count",
        ));
    }

    Ok((init_url, segment_urls, segment_offsets_ms))
}

fn fill_dash_template(template: &str, number: u64, time: u64) -> String {
    fn format_token(value: u64, caps: &regex::Captures<'_>) -> String {
        let Some(width) = caps
            .get(1)
            .and_then(|width| width.as_str().parse::<usize>().ok())
        else {
            return value.to_string();
        };
        format!("{value:0width$}")
    }

    static NUMBER_TOKEN_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\$Number(?:%0?(\d+)d)?\$"#).unwrap());
    static TIME_TOKEN_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\$Time(?:%0?(\d+)d)?\$"#).unwrap());

    let with_number = NUMBER_TOKEN_RE.replace_all(template, |caps: &regex::Captures<'_>| {
        format_token(number, caps)
    });
    TIME_TOKEN_RE
        .replace_all(&with_number, |caps: &regex::Captures<'_>| {
            format_token(time, caps)
        })
        .into_owned()
}

/// Resolve a TIDAL stream using a pre-built request description.
pub async fn resolve_stream(
    http: &reqwest::Client,
    access_token: &str,
    request: &StreamRequest,
) -> std::result::Result<StreamInfo, StreamResolveError> {
    crate::services::tidal::backoff::global()
        .check()
        .map_err(|error| StreamResolveError::RequestFailed {
            message: error.to_string(),
        })?;

    let url = track_playback_info_url(request)?;

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|error| StreamResolveError::RequestFailed {
            message: error.to_string(),
        })?;

    let status = resp.status();
    let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
    let raw = resp
        .text()
        .await
        .map_err(|error| StreamResolveError::RequestFailed {
            message: format!("failed to read playback response body: {error}"),
        })?;
    let safe_body = redact_tidal_stream_body(&raw);
    tracing::debug!(
        response_bytes = raw.len(),
        "TIDAL playback response received"
    );

    if !status.is_success() {
        crate::services::tidal::backoff::global().classify(status.as_u16(), &raw, retry_after);

        if asset_not_ready_body(&raw) {
            return Err(StreamResolveError::StreamRejected {
                message: format!("TIDAL rejected playback request with {status}: {safe_body}"),
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || session_expired_body(&raw) {
            return Err(StreamResolveError::SessionExpired {
                message: format!("TIDAL returned {status}: {safe_body}"),
            });
        }

        if status.is_client_error() {
            return Err(StreamResolveError::StreamRejected {
                message: format!("TIDAL rejected playback request with {status}: {safe_body}"),
            });
        }

        return Err(StreamResolveError::UpstreamHttp {
            status,
            body: safe_body,
        });
    }

    let resp: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| StreamResolveError::ResponseParseFailed {
            message: format!("failed to parse playback response JSON: {error}; body: {safe_body}"),
        })?;

    // The manifest is base64-encoded JSON containing the actual URL
    let manifest_b64 = resp
        .get("manifest")
        .and_then(|value| value.as_str())
        .ok_or(StreamResolveError::MissingManifest)?;

    let manifest_bytes = base64::engine::general_purpose::STANDARD
        .decode(manifest_b64)
        .map_err(|error| StreamResolveError::ManifestDecodeFailed {
            message: error.to_string(),
        })?;

    let manifest_mime = resp
        .get("manifestMimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Resolve the manifest into (stream_url, segment_urls, segment_offsets_ms, audio_codec).
    // segment_urls is non-empty only for segmented DASH (SegmentTemplate shape);
    // segment_offsets_ms is parallel to segment_urls (millisecond start of each segment).
    let (stream_url, segment_urls, segment_offsets_ms, dash_codec) =
        if manifest_mime.contains("dash+xml") {
            let manifest_str = std::str::from_utf8(&manifest_bytes).map_err(|error| {
                StreamResolveError::ManifestParseFailed {
                    message: format!("DASH manifest is not valid UTF-8: {error}"),
                }
            })?;
            let codec = extract_dash_codec(manifest_str);

            // Try SegmentTemplate (segmented CMAF fMP4) first.
            match parse_dash_segment_template(manifest_str) {
                Ok((init_url, segs, offsets)) => (init_url, segs, offsets, Some(codec)),
                Err(_) => {
                    // Fall back to single <BaseURL> (simpler DASH shape from older catalogue).
                    static DASH_URL_RE: std::sync::LazyLock<regex::Regex> =
                        std::sync::LazyLock::new(|| {
                            regex::Regex::new(r"<BaseURL[^>]*>(https?://[^<]+)</BaseURL>").unwrap()
                        });
                    match DASH_URL_RE
                        .captures(manifest_str)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_string())
                    {
                        Some(url) => (url, vec![], vec![], Some(codec)),
                        None => {
                            let preview: String = manifest_str.chars().take(2048).collect();
                            tracing::warn!(
                                track_id = request.track_id,
                                manifest_mime = %manifest_mime,
                                manifest_preview = %preview,
                                "TIDAL DASH manifest: no SegmentTemplate or BaseURL found"
                            );
                            return Err(StreamResolveError::MissingStreamUrl);
                        }
                    }
                }
            }
        } else {
            // JSON manifest (application/vnd.tidal.bts or similar)
            let manifest: serde_json::Value =
                serde_json::from_slice(&manifest_bytes).map_err(|error| {
                    StreamResolveError::ManifestParseFailed {
                        message: error.to_string(),
                    }
                })?;
            match manifest
                .get("urls")
                .and_then(|urls| urls.as_array())
                .and_then(|urls| urls.first())
                .and_then(|url| url.as_str())
                .map(|s| s.to_string())
            {
                Some(url) => (
                    url,
                    vec![],
                    vec![],
                    Some(extract_bts_codec(&manifest, manifest_mime)),
                ),
                None => {
                    let preview: String = String::from_utf8_lossy(&manifest_bytes)
                        .chars()
                        .take(2048)
                        .collect();
                    tracing::warn!(
                        track_id = request.track_id,
                        manifest_mime = %manifest_mime,
                        manifest_preview = %preview,
                        "TIDAL JSON manifest missing urls[0]"
                    );
                    return Err(StreamResolveError::MissingStreamUrl);
                }
            }
        };

    let (audio_quality, quality_agreement) = resolved_audio_quality(&resp, &request.audio_quality);
    let sample_rate = resp
        .get("sampleRate")
        .and_then(|value| value.as_i64())
        .map(|v| v as i32);
    let bit_depth = resp
        .get("bitDepth")
        .and_then(|value| value.as_i64())
        .map(|v| v as i32);
    // For DASH, use the codec extracted from codecs= attribute (e.g. "flac", "mp4a.40.2")
    // so codec_kind() returns the right audio type for gapless decisions.
    // For JSON manifests, use manifestMimeType as before.
    let codec = dash_codec.unwrap_or_else(|| {
        resp.get("manifestMimeType")
            .and_then(|value| value.as_str())
            .unwrap_or("audio/flac")
            .to_string()
    });

    match quality_agreement {
        AudioQualityAgreement::Agreed => {
            tracing::info!(
                target: "noor.playback.tidal",
                event = "playback_quality_agreed",
                track_id = request.track_id,
                requested_quality = %request.audio_quality,
                returned_quality = %audio_quality,
                sample_rate = ?sample_rate,
                bit_depth = ?bit_depth,
                manifest_mime = %manifest_mime,
                inner_codec = %codec,
                codec = %codec,
                dash_segments = segment_urls.len(),
                "TIDAL playback quality agreed"
            );
        }
        AudioQualityAgreement::Changed => {
            tracing::warn!(
                target: "noor.playback.tidal",
                event = "playback_quality_changed",
                track_id = request.track_id,
                requested_quality = %request.audio_quality,
                returned_quality = %audio_quality,
                sample_rate = ?sample_rate,
                bit_depth = ?bit_depth,
                manifest_mime = %manifest_mime,
                inner_codec = %codec,
                codec = %codec,
                dash_segments = segment_urls.len(),
                "TIDAL playback quality differed from request"
            );
        }
        AudioQualityAgreement::Unreported => {
            tracing::warn!(
                target: "noor.playback.tidal",
                event = "playback_quality_unreported",
                track_id = request.track_id,
                requested_quality = %request.audio_quality,
                sample_rate = ?sample_rate,
                bit_depth = ?bit_depth,
                manifest_mime = %manifest_mime,
                inner_codec = %codec,
                codec = %codec,
                dash_segments = segment_urls.len(),
                "TIDAL playback response omitted audio quality"
            );
        }
    }

    tracing::info!(
        track_id = request.track_id,
        requested_quality = %request.audio_quality,
        returned_quality = %audio_quality,
        sample_rate = ?sample_rate,
        bit_depth = ?bit_depth,
        manifest_mime = %manifest_mime,
        inner_codec = %codec,
        dash_segments = segment_urls.len(),
        "TIDAL playback stream resolved"
    );

    Ok(StreamInfo {
        url: stream_url,
        segment_urls,
        segment_offsets_ms,
        track_id: request.track_id,
        audio_quality,
        codec,
        sample_rate,
        bit_depth,
    })
}

/// Get streaming URL for a TIDAL track.
pub async fn get_stream_url(
    http: &reqwest::Client,
    access_token: &str,
    track_id: i64,
    quality: &str,
) -> std::result::Result<StreamInfo, StreamResolveError> {
    resolve_stream(http, access_token, &StreamRequest::new(track_id, quality)).await
}

pub async fn resolve_video_stream(
    http: &reqwest::Client,
    access_token: &str,
    video_id: i64,
    video_quality: &str,
) -> std::result::Result<VideoStreamInfo, StreamResolveError> {
    crate::services::tidal::backoff::global()
        .check()
        .map_err(|error| StreamResolveError::RequestFailed {
            message: error.to_string(),
        })?;

    let quality = if video_quality.trim().is_empty() {
        "HIGH"
    } else {
        video_quality.trim()
    };
    let url = video_playback_info_url(video_id, quality)?;

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|error| StreamResolveError::RequestFailed {
            message: error.to_string(),
        })?;

    let status = resp.status();
    let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
    let raw = resp
        .text()
        .await
        .map_err(|error| StreamResolveError::RequestFailed {
            message: format!("failed to read video playback response body: {error}"),
        })?;
    let safe_body = redact_tidal_stream_body(&raw);
    tracing::debug!(
        target: "tidal::video",
        video_id,
        response_bytes = raw.len(),
        "TIDAL video playback response received"
    );

    if !status.is_success() {
        crate::services::tidal::backoff::global().classify(status.as_u16(), &raw, retry_after);

        if asset_not_ready_body(&raw) {
            return Err(StreamResolveError::StreamRejected {
                message: format!(
                    "TIDAL rejected video playback request with {status}: {safe_body}"
                ),
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || session_expired_body(&raw) {
            return Err(StreamResolveError::SessionExpired {
                message: format!("TIDAL returned {status}: {safe_body}"),
            });
        }

        if status.is_client_error() {
            return Err(StreamResolveError::StreamRejected {
                message: format!(
                    "TIDAL rejected video playback request with {status}: {safe_body}"
                ),
            });
        }

        return Err(StreamResolveError::UpstreamHttp {
            status,
            body: safe_body,
        });
    }

    let resp_json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| StreamResolveError::ResponseParseFailed {
            message: format!(
                "failed to parse video playback response JSON: {error}; body: {safe_body}"
            ),
        })?;

    parse_video_stream_info(&resp_json, quality)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identifies_session_errors() {
        assert!(
            StreamResolveError::SessionExpired {
                message: "expired".to_string(),
            }
            .is_session_expired()
        );
        assert!(!StreamResolveError::MissingStreamUrl.is_session_expired());
    }

    #[test]
    fn decodes_missing_manifest_errors_cleanly() {
        let err = resolve_manifest_url(&serde_json::json!({})).unwrap_err();
        assert!(matches!(err, StreamResolveError::MissingManifest));
    }

    #[test]
    fn classifies_body_session_clues() {
        assert!(session_expired_body(
            r#"{"userMessage":"User does not have a valid session"}"#
        ));
        assert!(session_expired_body(r#"{"subStatus":6001}"#));
        assert!(!session_expired_body(r#"{"ok":true}"#));
    }

    #[test]
    fn classifies_asset_not_ready_as_stream_rejection_clue() {
        assert!(asset_not_ready_body(
            r#"{"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
        ));
        assert!(!session_expired_body(
            r#"{"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
        ));
    }

    #[test]
    fn classifies_client_errors_as_rejected_stream_requests() {
        let err = StreamResolveError::StreamRejected {
            message: "rejected".to_string(),
        };
        assert!(err.is_stream_rejected());
        assert!(!StreamResolveError::MissingManifest.is_stream_rejected());
    }

    #[test]
    fn exposes_asset_not_ready_stream_rejections() {
        let err = StreamResolveError::StreamRejected {
            message:
                r#"TIDAL rejected playback request with 401 Unauthorized: {"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#
                    .to_string(),
        };

        assert!(err.is_stream_rejected());
        assert!(err.is_asset_not_ready());
    }

    #[test]
    fn track_playback_info_url_keeps_quality_values_in_query_value() {
        let request = StreamRequest {
            track_id: 42,
            audio_quality: "HIGH&countryCode=US".to_string(),
            playback_mode: "STREAM".to_string(),
            asset_presentation: "FULL".to_string(),
        };

        let url = track_playback_info_url(&request).expect("URL should build");

        assert_eq!(
            url,
            "https://api.tidal.com/v1/tracks/42/playbackinfopostpaywall?audioquality=HIGH%26countryCode%3DUS&playbackmode=STREAM&assetpresentation=FULL"
        );
    }

    #[test]
    fn video_playback_info_url_keeps_quality_values_in_query_value() {
        let url =
            video_playback_info_url(77, "HIGH&playbackmode=OFFLINE").expect("URL should build");

        assert_eq!(
            url,
            "https://api.tidal.com/v1/videos/77/playbackinfopostpaywall?videoquality=HIGH%26playbackmode%3DOFFLINE&playbackmode=STREAM&assetpresentation=FULL"
        );
    }

    #[test]
    fn parses_direct_video_hls_url() {
        let info = parse_video_stream_info(
            &json!({
                "hlsUrl": "https://cdn.example.test/master.m3u8",
                "videoQuality": "HIGH",
                "expiresAt": "2026-05-05T10:00:00Z"
            }),
            "LOW",
        )
        .expect("video stream should parse");

        assert_eq!(
            info.hls_manifest_url,
            "https://cdn.example.test/master.m3u8"
        );
        assert_eq!(info.video_quality, "HIGH");
        assert!(info.expires_at.is_some());
    }

    #[test]
    fn parses_base64_json_video_manifest() {
        let manifest = base64::engine::general_purpose::STANDARD
            .encode(r#"{"urls":["https://cdn.example.test/video.m3u8"]}"#);
        let info = parse_video_stream_info(&json!({ "manifest": manifest }), "MEDIUM")
            .expect("base64 JSON manifest should parse");

        assert_eq!(info.hls_manifest_url, "https://cdn.example.test/video.m3u8");
        assert_eq!(info.video_quality, "MEDIUM");
    }

    #[test]
    fn tidal_stream_body_redaction_removes_signed_media_values() {
        let raw = json!({
            "streamUrl": "https://cdn.example.test/audio.flac?Signature=secret&Expires=123",
            "manifest": "base64-manifest-with-signed-urls",
            "urls": ["https://cdn.example.test/video.m3u8?token=secret"],
            "audioQuality": "HI_RES_LOSSLESS"
        })
        .to_string();

        let redacted = redact_tidal_stream_body(&raw);

        assert!(redacted.contains("HI_RES_LOSSLESS"));
        assert!(!redacted.contains("Signature=secret"));
        assert!(!redacted.contains("base64-manifest-with-signed-urls"));
        assert!(!redacted.contains("token=secret"));
    }

    #[test]
    fn dash_template_expands_negative_timeline_repeat_to_full_duration() {
        let xml = r#"
            <MPD mediaPresentationDuration="PT10S">
              <Period>
                <AdaptationSet>
                  <Representation>
                    <SegmentTemplate
                      timescale="1000"
                      initialization="https://cdn.example.test/init.mp4"
                      media="https://cdn.example.test/seg-$Number$.m4s"
                      startNumber="1">
                      <SegmentTimeline>
                        <S d="3000" r="-1"/>
                      </SegmentTimeline>
                    </SegmentTemplate>
                  </Representation>
                </AdaptationSet>
              </Period>
            </MPD>
        "#;

        let (init_url, segment_urls, segment_offsets_ms) =
            parse_dash_segment_template(xml).expect("DASH parses");

        assert_eq!(init_url, "https://cdn.example.test/init.mp4");
        assert_eq!(
            segment_urls,
            vec![
                "https://cdn.example.test/seg-1.m4s",
                "https://cdn.example.test/seg-2.m4s",
                "https://cdn.example.test/seg-3.m4s",
                "https://cdn.example.test/seg-4.m4s"
            ]
        );
        assert_eq!(segment_offsets_ms, vec![0u64, 3000, 6000, 9000]);
    }

    #[test]
    fn dash_template_expands_time_tokens_and_padding() {
        let xml = r#"
            <MPD mediaPresentationDuration="PT12S">
              <Period>
                <AdaptationSet>
                  <Representation>
                    <SegmentTemplate
                      timescale="1000"
                      initialization="https://cdn.example.test/init.mp4"
                      media="https://cdn.example.test/time-$Time%05d$.m4s"
                      startNumber="9">
                      <SegmentTimeline>
                        <S t="1000" d="3000" r="2"/>
                        <S d="1000" r="1"/>
                      </SegmentTimeline>
                    </SegmentTemplate>
                  </Representation>
                </AdaptationSet>
              </Period>
            </MPD>
        "#;

        let (_, segment_urls, segment_offsets_ms) =
            parse_dash_segment_template(xml).expect("DASH parses");

        assert_eq!(
            segment_urls,
            vec![
                "https://cdn.example.test/time-01000.m4s",
                "https://cdn.example.test/time-04000.m4s",
                "https://cdn.example.test/time-07000.m4s",
                "https://cdn.example.test/time-10000.m4s",
                "https://cdn.example.test/time-11000.m4s"
            ]
        );
        assert_eq!(segment_offsets_ms, vec![1000u64, 4000, 7000, 10000, 11000]);
    }

    #[test]
    fn dash_template_prefers_timeline_over_template_duration() {
        let xml = r#"
            <MPD mediaPresentationDuration="PT10S">
              <Period>
                <AdaptationSet>
                  <Representation>
                    <SegmentTemplate
                      timescale="1000"
                      duration="2000"
                      initialization="https://cdn.example.test/init.mp4"
                      media="https://cdn.example.test/time-$Time$.m4s"
                      startNumber="1">
                      <SegmentTimeline>
                        <S t="9000" d="3000" r="2"/>
                      </SegmentTimeline>
                    </SegmentTemplate>
                  </Representation>
                </AdaptationSet>
              </Period>
            </MPD>
        "#;

        let (_, segment_urls, segment_offsets_ms) =
            parse_dash_segment_template(xml).expect("DASH parses");

        assert_eq!(
            segment_urls,
            vec![
                "https://cdn.example.test/time-9000.m4s",
                "https://cdn.example.test/time-12000.m4s",
                "https://cdn.example.test/time-15000.m4s"
            ]
        );
        assert_eq!(segment_offsets_ms, vec![9000u64, 12000, 15000]);
    }

    #[test]
    fn resolved_audio_quality_marks_matching_response_as_agreed() {
        let (quality, agreement) = resolved_audio_quality(
            &json!({
                "audioQuality": "HI_RES_LOSSLESS"
            }),
            "HI_RES_LOSSLESS",
        );

        assert_eq!(quality, "HI_RES_LOSSLESS");
        assert_eq!(agreement, AudioQualityAgreement::Agreed);
    }

    #[test]
    fn resolved_audio_quality_marks_different_response_as_changed() {
        let (quality, agreement) = resolved_audio_quality(
            &json!({
                "audioQuality": "LOSSLESS"
            }),
            "HI_RES_LOSSLESS",
        );

        assert_eq!(quality, "LOSSLESS");
        assert_eq!(agreement, AudioQualityAgreement::Changed);
    }

    #[test]
    fn resolved_audio_quality_marks_missing_response_as_unreported() {
        let (quality, agreement) = resolved_audio_quality(&json!({}), "LOSSLESS");

        assert_eq!(quality, "LOSSLESS");
        assert_eq!(agreement, AudioQualityAgreement::Unreported);
    }

    #[test]
    fn codec_kind_recognizes_mp4a_codec_strings_as_aac() {
        let info = StreamInfo {
            url: "https://audio.example.test/track.m4a".to_string(),
            segment_urls: vec![],
            segment_offsets_ms: vec![],
            track_id: 1,
            audio_quality: "HIGH".to_string(),
            codec: "mp4a.40.2".to_string(),
            sample_rate: None,
            bit_depth: None,
        };

        assert_eq!(info.codec_kind(), StreamCodec::Aac);
        assert!(!info.is_lossless());
    }

    #[test]
    fn extracts_inner_codec_from_bts_manifest() {
        let manifest = json!({
            "mimeType": "audio/mp4",
            "codecs": "mp4a.40.2",
            "urls": ["https://audio.example.test/track.m4a"]
        });

        assert_eq!(
            extract_bts_codec(&manifest, "application/vnd.tidal.bts"),
            "mp4a.40.2"
        );
    }

    fn resolve_manifest_url(
        resp: &serde_json::Value,
    ) -> std::result::Result<String, StreamResolveError> {
        let manifest_b64 = resp
            .get("manifest")
            .and_then(|value| value.as_str())
            .ok_or(StreamResolveError::MissingManifest)?;
        let manifest_bytes = base64::engine::general_purpose::STANDARD
            .decode(manifest_b64)
            .map_err(|error| StreamResolveError::ManifestDecodeFailed {
                message: error.to_string(),
            })?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).map_err(|error| {
                StreamResolveError::ManifestParseFailed {
                    message: error.to_string(),
                }
            })?;
        let stream_url = manifest
            .get("urls")
            .and_then(|urls| urls.as_array())
            .and_then(|urls| urls.first())
            .and_then(|url| url.as_str())
            .ok_or(StreamResolveError::MissingStreamUrl)?
            .to_string();
        Ok(stream_url)
    }
}
