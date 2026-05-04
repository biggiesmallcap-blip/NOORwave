use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use uuid::Uuid;

const EVENT_URL: &str = "https://ec.tidal.com/api/event-batch";
const APP_VERSION: &str = "2.47.0";

struct JwtClaims {
    uid: i64,
    cid: String,
    sid: String,
}

fn decode_jwt_claims(token: &str) -> Result<JwtClaims> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    anyhow::ensure!(parts.len() >= 2, "invalid JWT: fewer than 2 segments");
    let payload = URL_SAFE_NO_PAD.decode(parts[1])?;
    let v: serde_json::Value = serde_json::from_slice(&payload)?;
    Ok(JwtClaims {
        uid: v["userId"].as_i64().unwrap_or(0),
        cid: v["cid"].as_str().unwrap_or("").to_string(),
        sid: v["sid"].as_str().unwrap_or("").to_string(),
    })
}

fn encode_sqs_batch(message_body: &str, headers_json: &str) -> String {
    let entry_id = Uuid::new_v4().to_string();
    [
        format!(
            "SendMessageBatchRequestEntry.1.Id={}",
            urlencoding::encode(&entry_id)
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageBody={}",
            urlencoding::encode(message_body)
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageAttribute.1.Name={}",
            urlencoding::encode("Name")
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageAttribute.1.Value.StringValue={}",
            urlencoding::encode("playback_session")
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageAttribute.1.Value.DataType={}",
            urlencoding::encode("String")
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageAttribute.2.Name={}",
            urlencoding::encode("Headers")
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageAttribute.2.Value.StringValue={}",
            urlencoding::encode(headers_json)
        ),
        format!(
            "SendMessageBatchRequestEntry.1.MessageAttribute.2.Value.DataType={}",
            urlencoding::encode("String")
        ),
    ]
    .join("&")
}

pub async fn report_play(
    http: &reqwest::Client,
    access_token: &str,
    tidal_track_id: i64,
    audio_quality: &str,
    duration_ms: i64,
) -> Result<()> {
    let claims = decode_jwt_claims(access_token)?;
    let now_ms = Utc::now().timestamp_millis();
    let end_ms = now_ms + duration_ms;
    let duration_secs = duration_ms as f64 / 1000.0;
    let session_id = Uuid::new_v4().to_string();
    let event_id = Uuid::new_v4().to_string();
    let track_id_str = tidal_track_id.to_string();

    let event = serde_json::json!({
        "group": "play_log",
        "version": 2,
        "ts": now_ms,
        "uuid": event_id,
        "user": {
            "id": claims.uid,
            "clientId": claims.cid.parse::<i64>().unwrap_or_else(|_| {
                tracing::warn!("play_reporter: cid '{}' is not an integer, sending 0", claims.cid);
                0
            }),
            "sessionId": claims.sid,
        },
        "client": {
            "token": claims.cid,
            "deviceType": "androidAuto",
            "version": APP_VERSION,
            "platform": "android",
        },
        "payload": {
            "playbackSessionId": session_id,
            "productType": "TRACK",
            "actualProductId": track_id_str,
            "requestedProductId": track_id_str,
            "actualAssetPresentation": "FULL",
            "actualAudioMode": "STEREO",
            "actualQuality": audio_quality,
            "sourceType": "",
            "sourceId": "",
            "startTimestamp": now_ms,
            "endTimestamp": end_ms,
            "startAssetPosition": 0.0_f64,
            "endAssetPosition": duration_secs,
            "isPostPaywall": true,
            "actions": [
                {"actionType": "PLAYBACK_START", "assetPosition": 0.0_f64, "timestamp": now_ms},
                {"actionType": "PLAYBACK_STOP", "assetPosition": duration_secs, "timestamp": end_ms},
            ],
        },
        "extras": serde_json::Value::Null,
    });

    let headers_obj = serde_json::json!({
        "app-name": "TIDAL",
        "app-version": APP_VERSION,
        "client-id": claims.cid,
        "consent-category": "NECESSARY",
        "os-name": "android",
        "requested-sent-timestamp": now_ms,
        "authorization": access_token,
    });

    let body_str = serde_json::to_string(&event)?;
    let headers_str = serde_json::to_string(&headers_obj)?;
    let form_body = encode_sqs_batch(&body_str, &headers_str);

    let resp = http
        .post(EVENT_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Authorization", format!("Bearer {}", access_token))
        .body(form_body)
        .send()
        .await?;

    tracing::debug!("play report: HTTP {}", resp.status());
    let _ = resp.bytes().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_decode_extracts_uid_cid_sid() {
        let payload_json = r#"{"userId":12345,"cid":"67890","sid":"sess-abc"}"#;
        let encoded = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let token = format!("header.{}.sig", encoded);
        let claims = decode_jwt_claims(&token).unwrap();
        assert_eq!(claims.uid, 12345);
        assert_eq!(claims.cid, "67890");
        assert_eq!(claims.sid, "sess-abc");
    }

    #[test]
    fn jwt_decode_fails_on_invalid_token() {
        assert!(decode_jwt_claims("notajwt").is_err());
    }

    #[test]
    fn sqs_form_body_contains_required_keys() {
        let body = encode_sqs_batch(r#"{"test":1}"#, r#"{"auth":"tok"}"#);
        assert!(body.contains("SendMessageBatchRequestEntry.1.Id="));
        assert!(body.contains("SendMessageBatchRequestEntry.1.MessageBody="));
        assert!(body.contains("playback_session"));
        assert!(body.contains("Headers"));
    }

    #[test]
    fn sqs_form_body_url_encodes_json() {
        // JSON braces must be percent-encoded — { → %7B
        let body = encode_sqs_batch(r#"{"key":"val"}"#, "{}");
        assert!(
            body.contains("%7B"),
            "JSON must be URL-encoded in the form body"
        );
    }
}
