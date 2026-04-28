use base64::Engine;
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

#[derive(Debug, Deserialize)]
pub struct StreamInfo {
    pub url: String,
    #[serde(rename = "trackId")]
    pub track_id: i64,
    #[serde(rename = "audioQuality")]
    pub audio_quality: String,
    pub codec: String,
    #[serde(rename = "sampleRate")]
    pub sample_rate: Option<i32>,
    #[serde(rename = "bitDepth")]
    pub bit_depth: Option<i32>,
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
        match self.codec.to_ascii_lowercase().as_str() {
            // TIDAL's "Broadcast Transport Stream" is a manifest wrapper around
            // FLAC for LOSSLESS / HI_RES_LOSSLESS quality. Symphonia decodes the
            // inner FLAC, and the underlying audio supports gapless seeks the
            // same way raw FLAC does — so treat it as FLAC for gapless gating.
            "audio/flac" | "flac" | "application/vnd.tidal.bts" => StreamCodec::Flac,
            "audio/aac" | "aac" | "audio/mp4" | "audio/m4a" => StreamCodec::Aac,
            "audio/mpeg" | "audio/mp3" | "mp3" => StreamCodec::Mp3,
            "audio/ogg" | "ogg" => StreamCodec::Ogg,
            _ => StreamCodec::Unknown,
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(self.codec_kind(), StreamCodec::Flac)
    }

    pub fn supports_gapless(&self) -> bool {
        matches!(
            self.codec_kind(),
            StreamCodec::Flac | StreamCodec::Aac | StreamCodec::Mp3
        )
    }

    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.sample_rate
            .and_then(|rate| (rate > 0).then_some(rate as u32))
    }

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

    pub fn is_stream_rejected(&self) -> bool {
        matches!(self, Self::StreamRejected { .. })
    }
}

fn session_expired_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("valid session")
        || lower.contains("user does not have a valid session")
        || lower.contains("session expired")
        || lower.contains("\"substatus\":6001")
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

    let url = format!(
        "{}/tracks/{}/playbackinfopostpaywall?audioquality={}&playbackmode={}&assetpresentation={}",
        TIDAL_API_URL,
        request.track_id,
        request.audio_quality,
        request.playback_mode,
        request.asset_presentation
    );

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|error| StreamResolveError::RequestFailed {
            message: error.to_string(),
        })?;

    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|error| StreamResolveError::RequestFailed {
            message: format!("failed to read playback response body: {error}"),
        })?;
    tracing::debug!("TIDAL playback response: {}", raw);

    if !status.is_success() {
        crate::services::tidal::backoff::global().classify(status.as_u16(), &raw);

        if status == reqwest::StatusCode::UNAUTHORIZED || session_expired_body(&raw) {
            return Err(StreamResolveError::SessionExpired {
                message: format!("TIDAL returned {status}: {raw}"),
            });
        }

        if status.is_client_error() {
            return Err(StreamResolveError::StreamRejected {
                message: format!("TIDAL rejected playback request with {status}: {raw}"),
            });
        }

        return Err(StreamResolveError::UpstreamHttp { status, body: raw });
    }

    let resp: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| StreamResolveError::ResponseParseFailed {
            message: format!("failed to parse playback response JSON: {error}; body: {raw}"),
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

    let stream_url = if manifest_mime.contains("dash+xml") {
        // MPEG-DASH XML manifest: extract the first <BaseURL> element
        let manifest_str =
            std::str::from_utf8(&manifest_bytes).map_err(|error| {
                StreamResolveError::ManifestParseFailed {
                    message: format!("DASH manifest is not valid UTF-8: {error}"),
                }
            })?;
        static DASH_URL_RE: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| {
                regex::Regex::new(r"<BaseURL[^>]*>(https?://[^<]+)</BaseURL>").unwrap()
            });
        DASH_URL_RE
            .captures(manifest_str)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or(StreamResolveError::MissingStreamUrl)?
    } else {
        // JSON manifest (application/vnd.tidal.bts or similar)
        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).map_err(|error| {
                StreamResolveError::ManifestParseFailed {
                    message: error.to_string(),
                }
            })?;
        manifest
            .get("urls")
            .and_then(|urls| urls.as_array())
            .and_then(|urls| urls.first())
            .and_then(|url| url.as_str())
            .ok_or(StreamResolveError::MissingStreamUrl)?
            .to_string()
    };

    Ok(StreamInfo {
        url: stream_url,
        track_id: request.track_id,
        audio_quality: resp
            .get("audioQuality")
            .and_then(|value| value.as_str())
            .unwrap_or(request.audio_quality.as_str())
            .to_string(),
        codec: resp
            .get("manifestMimeType")
            .and_then(|value| value.as_str())
            .unwrap_or("audio/flac")
            .to_string(),
        sample_rate: resp
            .get("sampleRate")
            .and_then(|value| value.as_i64())
            .map(|v| v as i32),
        bit_depth: resp
            .get("bitDepth")
            .and_then(|value| value.as_i64())
            .map(|v| v as i32),
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn classifies_client_errors_as_rejected_stream_requests() {
        let err = StreamResolveError::StreamRejected {
            message: "rejected".to_string(),
        };
        assert!(err.is_stream_rejected());
        assert!(!StreamResolveError::MissingManifest.is_stream_rejected());
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
