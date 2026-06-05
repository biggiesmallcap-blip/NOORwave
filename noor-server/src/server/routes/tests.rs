use super::catalog_routes::{
    merge_tidal_artist_album_filters, resolve_tidal_artist_release_filter,
};
use super::home_routes::{
    LastFmArtistSeed, LastFmTrackSeed, merge_lastfm_artist_seeds, merge_lastfm_track_seeds,
};
use super::*;
use crate::db::{Database, schema};
use crate::metadata::lastfm::{LastFmChartAlbum, LastFmChartArtist, LastFmChartTrack};
use crate::services::tidal::client::TidalAlbum;
use axum::{body::Body, http::Request};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

fn test_track(id: i64, title: &str) -> crate::db::models::Track {
    crate::db::models::Track {
        id,
        title: title.to_string(),
        artist_id: 1,
        artist_name: Some("Artist".to_string()),
        album_id: None,
        album_title: None,
        disc_number: None,
        track_number: None,
        duration_ms: Some(180_000),
        isrc: None,
        tidal_id: Some(id),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: Some("LOSSLESS".to_string()),
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal".to_string(),
        artwork_url: None,
    }
}

fn test_queue_item(
    id: i64,
    track: crate::db::models::Track,
    position: i32,
    source: &str,
) -> crate::db::models::QueueItem {
    crate::db::models::QueueItem {
        id,
        track,
        position,
        source: source.to_string(),
        reason: None,
        is_pending: source == "automix-new",
    }
}

fn lastfm_test_track(artist: &str, title: &str) -> LastFmChartTrack {
    LastFmChartTrack {
        artist: artist.to_string(),
        title: title.to_string(),
        mbid: None,
        image_url: None,
        listeners: None,
        playcount: None,
    }
}

fn lastfm_test_artist(name: &str) -> LastFmChartArtist {
    LastFmChartArtist {
        name: name.to_string(),
        mbid: None,
        image_url: None,
        listeners: None,
        playcount: None,
        match_score: None,
    }
}

fn lastfm_test_album(artist: &str, title: &str) -> LastFmChartAlbum {
    LastFmChartAlbum {
        artist: artist.to_string(),
        title: title.to_string(),
        mbid: None,
        image_url: None,
        playcount: None,
    }
}

fn tidal_test_album(id: i64, title: &str) -> TidalAlbum {
    TidalAlbum {
        id,
        title: title.to_string(),
        number_of_tracks: Some(1),
        number_of_volumes: Some(1),
        release_date: None,
        cover: None,
        artist: crate::services::tidal::client::TidalArtist {
            id: 42,
            name: "Artist".to_string(),
            picture: None,
            extra: HashMap::new(),
        },
        artists: None,
        audio_quality: None,
        release_type: None,
        extra: HashMap::new(),
    }
}

#[test]
fn tidal_artist_album_filter_merge_keeps_eps_and_dedupes_by_tidal_id() {
    let merged = merge_tidal_artist_album_filters([
        (
            vec![tidal_test_album(1, "Album"), tidal_test_album(2, "Shared")],
            "ALBUMS",
        ),
        (
            vec![tidal_test_album(3, "Single"), tidal_test_album(2, "Shared")],
            "EPSANDSINGLES",
        ),
        (vec![tidal_test_album(4, "Compilation")], "COMPILATIONS"),
        (vec![tidal_test_album(5, "Live")], "LIVE"),
    ]);

    let ids: Vec<i64> = merged.iter().map(|(album, _)| album.id).collect();
    let filters: Vec<&str> = merged.iter().map(|(_, filter)| *filter).collect();

    assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    assert_eq!(
        filters,
        vec!["ALBUMS", "ALBUMS", "EPSANDSINGLES", "COMPILATIONS", "LIVE"]
    );
}

#[test]
fn tidal_artist_release_filter_only_downgrades_missing_bucket_errors() {
    let missing = resolve_tidal_artist_release_filter(
            Err(anyhow::anyhow!(
                "TIDAL API error 404 Not Found: {{\"status\":404,\"subStatus\":2001,\"userMessage\":\"Resource not found\"}}"
            )),
            67003046,
            "LIVE",
        )
        .expect("missing release filters should be treated as empty");
    assert!(missing.is_empty());

    let auth_error = resolve_tidal_artist_release_filter(
        Err(anyhow::anyhow!(
            "TIDAL API error 401 Unauthorized: {{\"status\":401}}"
        )),
        67003046,
        "ALBUMS",
    )
    .expect_err("auth errors must not be swallowed as empty release filters");
    assert!(error_looks_like_auth(&auth_error));
    let expired_token_error = anyhow::anyhow!(
        "TIDAL API error 401 Unauthorized: {{\"status\":401,\"subStatus\":11003,\"userMessage\":\"The token has expired. (Expired on time)\"}}"
    );
    assert!(error_looks_like_auth(&expired_token_error));

    let rate_error = resolve_tidal_artist_release_filter(
        Err(anyhow::anyhow!(
            "TIDAL API error 429 Too Many Requests: rate limit"
        )),
        67003046,
        "EPSANDSINGLES",
    )
    .expect_err("rate limit errors must not be swallowed as empty release filters");
    assert!(rate_error.to_string().contains("429"));
}

#[test]
fn lastfm_track_seed_merge_prioritizes_recent_and_loved_context() {
    let seeds = merge_lastfm_track_seeds(
        vec![
            lastfm_test_track("Recent Artist", "Recent One"),
            lastfm_test_track("Recent Artist", "Recent Two"),
        ],
        vec![lastfm_test_track("Loved Artist", "Loved One")],
        vec![lastfm_test_track("Top Artist", "Top One")],
        0,
        4,
    );

    assert_eq!(seeds.len(), 4);
    assert_eq!(seeds[0].reason, "Because you played Recent One recently");
    assert_eq!(seeds[1].reason, "Because you played Recent Two recently");
    assert_eq!(seeds[2].reason, "Because you loved Loved One");
    assert_eq!(seeds[3].reason, "Near your top track Top One");
}

#[test]
fn lastfm_artist_seed_merge_uses_track_context_before_top_artists() {
    let track_seeds = vec![
        LastFmTrackSeed {
            artist: "Recent Artist".to_string(),
            title: "Recent One".to_string(),
            reason: "Because you played Recent One recently".to_string(),
        },
        LastFmTrackSeed {
            artist: "Recent Artist".to_string(),
            title: "Duplicate Artist".to_string(),
            reason: "Because you loved Duplicate Artist".to_string(),
        },
    ];
    let seeds = merge_lastfm_artist_seeds(
        &track_seeds,
        vec![lastfm_test_artist("Top Artist")],
        vec![lastfm_test_album("Album Artist", "Album One")],
        0,
        3,
    );

    assert_eq!(
        seeds,
        vec![
            LastFmArtistSeed {
                name: "Recent Artist".to_string(),
                reason: "Because you played Recent One recently".to_string(),
            },
            LastFmArtistSeed {
                name: "Top Artist".to_string(),
                reason: "Near your top artist Top Artist".to_string(),
            },
            LastFmArtistSeed {
                name: "Album Artist".to_string(),
                reason: "Because you play albums by Album Artist".to_string(),
            },
        ]
    );
}

#[test]
fn stream_error_mapping_marks_session_expired_as_unauthorized() {
    let (status, Json(body)) = tidal_playback_error_response(
        42,
        TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::SessionExpired {
            message: "expired".to_string(),
        }),
        "fallback",
    );

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["status"], "session_expired");
    assert_eq!(body["track_id"], 42);
}

#[test]
fn stream_error_mapping_marks_session_refresh_failures_as_unauthorized() {
    let (status, Json(body)) = tidal_playback_error_response(
        42,
        TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::SessionRefreshFailed {
            message: "refresh rejected".to_string(),
        }),
        "fallback",
    );

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["status"], "session_refresh_failed");
    assert_eq!(body["track_id"], 42);
    assert_eq!(body["details"], "refresh rejected");
}

#[test]
fn stream_error_mapping_marks_manifest_decode_failures_as_bad_gateway() {
    let (status, Json(body)) = tidal_playback_error_response(
        7,
        TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::ManifestDecodeFailed {
            message: "bad base64".to_string(),
        }),
        "fallback",
    );

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["status"], "manifest_decode_failed");
    assert_eq!(body["track_id"], 7);
}

#[tokio::test]
async fn pause_and_resume_playback_invalidate_inflight_generation() {
    let (db, db_path) = fresh_migrated_db();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    let app = api_routes(state.clone());

    let initial = {
        let guard = state.read().await;
        current_playback_generation(&guard)
    };

    let pause_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/pause")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pause_response.status(), StatusCode::OK);
    let after_pause = {
        let guard = state.read().await;
        current_playback_generation(&guard)
    };
    assert!(after_pause > initial);

    let resume_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/resume")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resume_response.status(), StatusCode::OK);
    let after_resume = {
        let guard = state.read().await;
        current_playback_generation(&guard)
    };
    assert!(after_resume > after_pause);

    let _ = std::fs::remove_file(db_path);
}

#[test]
fn stream_error_mapping_marks_rejected_stream_requests_as_forbidden() {
    let (status, Json(body)) = tidal_playback_error_response(
        11,
        TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::StreamRejected {
            message: "rejected".to_string(),
        }),
        "fallback",
    );

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["status"], "stream_rejected");
    assert_eq!(body["track_id"], 11);
}

#[test]
fn stream_error_mapping_preserves_upstream_http_status_details() {
    let (status, Json(body)) = tidal_playback_error_response(
        12,
        TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::UpstreamHttp {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: "rate limit".to_string(),
        }),
        "fallback",
    );

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["status"], "stream_upstream_http");
    assert_eq!(body["track_id"], 12);
    assert_eq!(body["details"], "rate limit");
    assert_eq!(
        body["message"],
        "TIDAL returned 429 Too Many Requests while starting playback."
    );
}

#[test]
fn video_stream_error_mapping_marks_session_refresh_failures_as_unauthorized() {
    let (status, Json(body)) = tidal_video_stream_error_response(
        99,
        tidal_stream::StreamResolveError::SessionRefreshFailed {
            message: "refresh rejected".to_string(),
        },
        "fallback",
    );

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["status"], "session_refresh_failed");
    assert_eq!(body["video_id"], 99);
    assert_eq!(body["details"], "refresh rejected");
}

#[test]
fn video_stream_error_mapping_marks_session_expired_as_unauthorized() {
    let (status, Json(body)) = tidal_video_stream_error_response(
        98,
        tidal_stream::StreamResolveError::SessionExpired {
            message: "expired".to_string(),
        },
        "fallback",
    );

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["status"], "session_expired");
    assert_eq!(body["video_id"], 98);
    assert_eq!(body["details"], "expired");
}

#[test]
fn video_stream_error_mapping_marks_manifest_decode_failures_as_bad_gateway() {
    let (status, Json(body)) = tidal_video_stream_error_response(
        101,
        tidal_stream::StreamResolveError::ManifestDecodeFailed {
            message: "bad base64".to_string(),
        },
        "fallback",
    );

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["status"], "manifest_decode_failed");
    assert_eq!(body["video_id"], 101);
    assert_eq!(body["details"], "bad base64");
}

#[test]
fn video_stream_error_mapping_marks_rejected_stream_requests_as_forbidden() {
    let (status, Json(body)) = tidal_video_stream_error_response(
        102,
        tidal_stream::StreamResolveError::StreamRejected {
            message: "rejected".to_string(),
        },
        "fallback",
    );

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["status"], "stream_rejected");
    assert_eq!(body["video_id"], 102);
    assert_eq!(body["details"], "rejected");
}

#[test]
fn video_stream_error_mapping_preserves_upstream_http_status_details() {
    let (status, Json(body)) = tidal_video_stream_error_response(
        100,
        tidal_stream::StreamResolveError::UpstreamHttp {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: "rate limit".to_string(),
        },
        "fallback",
    );

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["status"], "stream_upstream_http");
    assert_eq!(body["video_id"], 100);
    assert_eq!(body["details"], "rate limit");
    assert_eq!(
        body["message"],
        "TIDAL returned 429 Too Many Requests while starting video playback."
    );
}

#[test]
fn tidal_video_mix_id_normalization_allows_safe_ids() {
    assert_eq!(normalize_tidal_video_mix_id("abc123").unwrap(), "abc123");
    assert_eq!(
        normalize_tidal_video_mix_id("  video_mix-01  ").unwrap(),
        "video_mix-01"
    );
}

#[test]
fn tidal_video_mix_id_normalization_rejects_url_control_characters() {
    for id in [
        "",
        "../home",
        "mix/items",
        "mix?limit=1",
        "mix&includeTypes=Track",
        "mix#fragment",
        "mix id",
    ] {
        assert_eq!(
            normalize_tidal_video_mix_id(id),
            Err(StatusCode::BAD_REQUEST)
        );
    }
}

#[test]
fn tidal_search_query_normalization_short_circuits_blank_input() {
    assert_eq!(normalize_tidal_search_query(""), None);
    assert_eq!(normalize_tidal_search_query("   "), None);
    assert_eq!(
        normalize_tidal_search_query("  floating points  "),
        Some("floating points")
    );
}

#[test]
fn tidal_search_limit_is_bounded() {
    assert_eq!(normalize_tidal_search_limit(None), 20);
    assert_eq!(normalize_tidal_search_limit(Some(-1)), 1);
    assert_eq!(normalize_tidal_search_limit(Some(0)), 1);
    assert_eq!(normalize_tidal_search_limit(Some(15)), 15);
    assert_eq!(normalize_tidal_search_limit(Some(500)), 50);
}

#[test]
fn tidal_video_search_query_normalization_short_circuits_blank_input() {
    assert_eq!(normalize_tidal_video_search_query(""), None);
    assert_eq!(normalize_tidal_video_search_query("   "), None);
    assert_eq!(
        normalize_tidal_video_search_query("  live session  "),
        Some("live session")
    );
}

#[test]
fn tidal_video_search_limit_is_bounded() {
    assert_eq!(normalize_tidal_video_search_limit(None), 20);
    assert_eq!(normalize_tidal_video_search_limit(Some(-1)), 1);
    assert_eq!(normalize_tidal_video_search_limit(Some(0)), 1);
    assert_eq!(normalize_tidal_video_search_limit(Some(15)), 15);
    assert_eq!(normalize_tidal_video_search_limit(Some(500)), 50);
}

#[test]
fn tidal_playlist_search_query_normalization_short_circuits_blank_input() {
    assert_eq!(normalize_tidal_playlist_search_query(""), None);
    assert_eq!(normalize_tidal_playlist_search_query("   "), None);
    assert_eq!(
        normalize_tidal_playlist_search_query("  late night  "),
        Some("late night")
    );
}

#[test]
fn tidal_playlist_search_limit_is_bounded() {
    assert_eq!(normalize_tidal_playlist_search_limit(None), 20);
    assert_eq!(normalize_tidal_playlist_search_limit(Some(-5)), 1);
    assert_eq!(normalize_tidal_playlist_search_limit(Some(0)), 1);
    assert_eq!(normalize_tidal_playlist_search_limit(Some(12)), 12);
    assert_eq!(normalize_tidal_playlist_search_limit(Some(999)), 50);
}

#[test]
fn tidal_playlist_uuid_normalization_allows_safe_ids() {
    assert_eq!(
        normalize_tidal_playlist_uuid("123e4567-e89b-12d3-a456-426614174000").unwrap(),
        "123e4567-e89b-12d3-a456-426614174000"
    );
    assert_eq!(
        normalize_tidal_playlist_uuid("  tidal_playlist_01  ").unwrap(),
        "tidal_playlist_01"
    );
}

#[test]
fn tidal_playlist_uuid_normalization_rejects_url_control_characters() {
    for uuid in [
        "",
        "../tracks",
        "playlist/tracks",
        "playlist?limit=1",
        "playlist&countryCode=US",
        "playlist#fragment",
        "playlist id",
    ] {
        assert_eq!(
            normalize_tidal_playlist_uuid(uuid),
            Err(StatusCode::BAD_REQUEST)
        );
    }
}

#[test]
fn tidal_status_payload_reports_pkce_source_only_for_pkce_tokens() {
    let tokens = test_tidal_tokens(Some("pkce"));

    let body = tidal_status_payload(
        Some(&tokens),
        false,
        tidal_auth::TidalCredentialSource::Env,
        tidal_auth::TidalCredentialSource::Fallback,
    );

    assert_eq!(body["connected"], true);
    assert_eq!(body["auth_flow"], "pkce");
    assert_eq!(body["pkce_client_credential_source"], "env");
    assert!(body.get("legacy_client_credential_source").is_none());
}

#[test]
fn tidal_status_payload_reports_legacy_source_only_for_legacy_tokens() {
    let tokens = test_tidal_tokens(None);

    let body = tidal_status_payload(
        Some(&tokens),
        false,
        tidal_auth::TidalCredentialSource::Env,
        tidal_auth::TidalCredentialSource::Fallback,
    );

    assert_eq!(body["connected"], true);
    assert_eq!(body["auth_flow"], "legacy");
    assert_eq!(body["legacy_client_credential_source"], "fallback");
    assert!(body.get("pkce_client_credential_source").is_none());
}

#[test]
fn tidal_status_payload_disconnected_omits_credential_sources() {
    let body = tidal_status_payload(
        None,
        false,
        tidal_auth::TidalCredentialSource::Env,
        tidal_auth::TidalCredentialSource::Fallback,
    );

    assert_eq!(body["connected"], false);
    assert!(body.get("auth_flow").is_none());
    assert!(body.get("pkce_client_credential_source").is_none());
    assert!(body.get("legacy_client_credential_source").is_none());
}

#[test]
fn tidal_status_payload_reports_expired_tokens_as_disconnected() {
    let tokens = test_tidal_tokens(Some("pkce"));

    let body = tidal_status_payload(
        Some(&tokens),
        true,
        tidal_auth::TidalCredentialSource::Env,
        tidal_auth::TidalCredentialSource::Fallback,
    );

    assert_eq!(body["connected"], false);
    assert_eq!(body["reason"], "token_expired");
    assert_eq!(body["auth_flow"], "pkce");
    assert_eq!(body["user_id"], "u-1");
    assert!(body.get("pkce_client_credential_source").is_none());
    assert!(body.get("legacy_client_credential_source").is_none());
}

#[test]
fn tidal_token_expiry_uses_token_expiry_when_present() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-05T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    assert!(tidal_token_expired_at(
        Some("2026-06-04T23:58:30Z"),
        Some("2026-06-04 00:00:00"),
        86_400,
        now,
    ));
    assert!(!tidal_token_expired_at(
        Some("2026-06-05T00:05:00Z"),
        Some("2026-06-04 00:00:00"),
        86_400,
        now,
    ));
}

#[test]
fn tidal_token_expiry_falls_back_to_connected_at_plus_expires_in() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-05T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    assert!(tidal_token_expired_at(
        None,
        Some("2026-06-03 23:00:00"),
        86_400,
        now,
    ));
    assert!(!tidal_token_expired_at(
        None,
        Some("2026-06-04 23:30:00"),
        86_400,
        now,
    ));
}

#[test]
fn tidal_playlist_tracks_cache_returns_fresh_entries_and_expires_stale_entries() {
    let cache = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let key = tidal_playlist_tracks_cache_key("AU", "playlist-uuid", 100, 0);
    let tracks: Vec<TidalTrack> = Vec::new();

    put_cached_tidal_playlist_tracks(&cache, key.clone(), tracks.clone());

    assert!(get_cached_tidal_playlist_tracks(&cache, &key).is_some());

    {
        let mut guard = cache.lock().unwrap();
        guard.insert(
            key.clone(),
            (
                Instant::now() - TIDAL_PLAYLIST_TRACKS_CACHE_TTL - Duration::from_secs(1),
                tracks,
            ),
        );
    }

    assert!(get_cached_tidal_playlist_tracks(&cache, &key).is_none());
    assert!(!cache.lock().unwrap().contains_key(&key));
}

#[test]
fn ephemeral_stream_request_uses_user_audio_quality() {
    let request = build_ephemeral_tidal_stream_request(
        123,
        Some(crate::db::audio_settings::AudioQuality::HiResLossless),
    );

    assert_eq!(request.track_id, 123);
    assert_eq!(request.audio_quality, "HI_RES_LOSSLESS");
    assert_eq!(request.playback_mode, "STREAM");
    assert_eq!(request.asset_presentation, "FULL");
}

#[test]
fn ephemeral_stream_request_defaults_to_lossless_without_user_quality() {
    let request = build_ephemeral_tidal_stream_request(123, None);

    assert_eq!(request.audio_quality, tidal_stream::DEFAULT_AUDIO_QUALITY);
}

#[test]
fn requested_tidal_quality_prefers_user_setting_over_payload_quality() {
    let quality = requested_tidal_quality(
        Some(crate::db::audio_settings::AudioQuality::Lossless),
        Some("HI_RES_LOSSLESS"),
    );

    assert_eq!(quality, "LOSSLESS");
}

#[test]
fn requested_tidal_quality_uses_payload_quality_without_user_setting() {
    let quality = requested_tidal_quality(None, Some("HI_RES_LOSSLESS"));

    assert_eq!(quality, "HI_RES_LOSSLESS");
}

#[test]
fn ephemeral_synthetic_track_keeps_resolved_stream_quality() {
    let track = crate::PendingEphemeralTidalTrack {
        tidal_track_id: 456,
        title: "Resolved Track".to_string(),
        artist_name: Some("Artist".to_string()),
        album_title: Some("Album".to_string()),
        artwork_url: None,
        duration_ms: Some(180_000),
    };
    let stream = tidal_stream::StreamInfo {
        url: "https://cdn.example.test/audio.flac".to_string(),
        segment_urls: vec![],
        segment_offsets_ms: vec![],
        track_id: 456,
        audio_quality: "HI_RES_LOSSLESS".to_string(),
        codec: "audio/flac".to_string(),
        sample_rate: Some(96_000),
        bit_depth: Some(24),
    };

    let synthetic = build_ephemeral_synthetic_track(&track, &stream, None);

    assert_eq!(synthetic.id, -456);
    assert_eq!(synthetic.tidal_id, Some(456));
    assert_eq!(synthetic.best_quality.as_deref(), Some("HI_RES_LOSSLESS"));
    assert_eq!(synthetic.source, "tidal_ephemeral");
}

fn test_tidal_tokens(auth_flow: Option<&str>) -> tidal_auth::TidalTokens {
    tidal_auth::TidalTokens {
        access_token: "access-secret".to_string(),
        refresh_token: "refresh-secret".to_string(),
        token_type: "Bearer".to_string(),
        expires_in: 86_400,
        user_id: "u-1".to_string(),
        country_code: "AU".to_string(),
        auth_flow: auth_flow.map(str::to_string),
    }
}

fn test_tidal_track(id: i64, title: &str) -> crate::services::tidal::client::TidalTrack {
    crate::services::tidal::client::TidalTrack {
        id,
        title: title.to_string(),
        duration: 180,
        track_number: Some(1),
        volume_number: Some(1),
        isrc: None,
        artist: crate::services::tidal::client::TidalArtist {
            id: 10,
            name: "Artist".to_string(),
            picture: None,
            extra: HashMap::new(),
        },
        artists: None,
        album: None,
        audio_quality: Some("LOSSLESS".to_string()),
        stream_ready: Some(true),
        extra: HashMap::new(),
    }
}

#[test]
fn insert_tidal_track_uses_favorite_created_as_date_added() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        let track = test_tidal_track(2001, "Newest favorite");

        insert_tidal_track(conn, &track, true, Some("2026-05-01T12:34:56.000Z"))?;

        let date_added: String = conn.query_row(
            "SELECT date_added FROM tracks WHERE tidal_id = 2001",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(date_added, "2026-05-01T12:34:56.000Z");
        Ok(())
    })
    .expect("inserted favorite track");
    let _ = std::fs::remove_file(db_path);
}

#[test]
fn runtime_output_settings_preserve_persisted_exclusive_preferences() {
    let mut settings = crate::db::audio_settings::AudioSettings::default();
    settings.output_device = Some("Zen DAC V2".to_string());
    settings.exclusive_mode = true;
    settings.sample_rate_follow = true;
    settings.exclusive_release_grace_secs = 12;
    settings.exclusive_latency_mode = crate::db::audio_settings::ExclusiveLatencyMode::LowLatency;

    let output = runtime_output_settings_from_audio_settings(&settings);

    match output.device {
        playback_runtime::OutputDeviceSelection::Named(name) => {
            assert_eq!(name, "Zen DAC V2");
        }
        playback_runtime::OutputDeviceSelection::Default => {
            panic!("expected named output device")
        }
    }
    assert!(output.exclusive_mode);
    assert!(output.sample_rate_follow);
    assert_eq!(output.exclusive_release_grace_secs, 12);
    assert_eq!(
        output.exclusive_latency_mode,
        crate::db::audio_settings::ExclusiveLatencyMode::LowLatency
    );
}

#[test]
fn extracts_genre_candidates_from_mixed_metadata_shapes() {
    let mut extra = HashMap::new();
    extra.insert("genre".to_string(), json!("trip hop"));
    extra.insert(
        "subGenres".to_string(),
        json!([
            "shoegazee",
            { "name": "Tech House / House" },
            { "title": "Progressive House" }
        ]),
    );

    let genres =
        crate::genre::builder::collect_clear_genres(extract_genre_candidates_from_extra(&extra));

    assert_eq!(
        genres,
        vec![
            "Progressive House".to_string(),
            "Shoegaze".to_string(),
            "Trip-Hop".to_string()
        ]
    );
}

#[tokio::test]
async fn genre_heat_route_defaults_to_ninety_days() {
    let db_path = std::env::temp_dir().join(format!("noor-genre-heat-{}.db", uuid::Uuid::new_v4()));
    let db = Database::open(&db_path).expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| {
            schema::run_migrations(conn)?;
            conn.execute(
                "INSERT INTO genres (id, name, slug, parent_id) VALUES
                    (1, 'Electronic', 'electronic', NULL),
                    (2, 'Ambient', 'ambient', 1)",
                [],
            )?;
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Biosphere')", [])?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, tidal_id, best_quality, best_source, fidelity_score, is_favorite, source
                ) VALUES (1, 'Substrata', 1, 360000, 201, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO track_genres (track_id, genre_id, source, confidence)
                 VALUES (1, 2, 'musicbrainz', 1.0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
                 VALUES (1, datetime('now', '-10 days'), 180000, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
                 VALUES (1, datetime('now', '-120 days'), 180000, 1)",
                [],
            )?;
            Ok(())
        })
        .expect("seeded");

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/genres/heat")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let payload: Value = serde_json::from_slice(&body).expect("json body");
    let electronic = payload["heat"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["genre_id"] == 1))
        .expect("electronic row");

    assert_eq!(electronic["listen_count"], 1);
    assert_eq!(electronic["total_listened_ms"], 180000);

    let _ = std::fs::remove_file(db_path);
}

/// Build a fresh `AppState` backed by `db`. Single source of truth for test
/// initializers - when `crate::AppState` gains a field, add it here once.
fn fresh_test_state(db: Database) -> crate::AppState {
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    #[cfg(feature = "spotify-public")]
    let spotify_public = Arc::new(
        crate::services::spotify_public::SpotifyPublicClient::new(db.clone())
            .expect("SpotifyPublicClient::new must succeed in tests"),
    );
    crate::AppState {
        db,
        event_tx,
        http_client: reqwest::Client::new(),
        tidal_http_client: reqwest::Client::new(),
        tidal_tokens: None,
        tidal_mixes_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_radio_stations_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_moods_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_page_modules_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
        tidal_playlist_tracks_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
        lastfm_similar_cache: crate::services::radio::new_lastfm_similar_cache(),
        spotify_tokens: None,
        playback_runtime: None,
        playback_runtime_info: None,
        playback_generation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        current_stream_display: None,
        pending_stream_display: None,
        next_prebuffer_inflight: None,
        last_drop_preview: None,
        active_listen_session: None,
        live_listen_session: None,
        external_playback_track: None,
        ephemeral_tidal_track: None,
        tidal_login_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rss_aggregator: Arc::new(crate::services::rss_feeds::FeedAggregator::new(
            reqwest::Client::new(),
        )),
        acrcloud_client: None,
        analysis_tx: None,
        dj_analysis_tx: None,
        dj_profile_rebuild_inflight: Arc::new(std::sync::Mutex::new(HashMap::new())),
        audio_analysis_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        audio_analysis_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        acrcloud_scan_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        acrcloud_daily_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        spotify_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        spotify_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        spotify_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        lastfm_enrich_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        musicbrainz_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        tidal_sync_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        tidal_sync_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        discovery_train_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        radio_similarity_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        refreshed_seeds: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        embedding_cache: Arc::new(std::sync::Mutex::new(None)),
        master_key: crate::services::crypto::MasterKey::load_or_generate(
            &std::env::temp_dir().join(format!("noor-test-key-{}", uuid::Uuid::new_v4())),
        )
        .expect("test master key"),
        pending_tidal_mix_queue: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        prepared_ephemeral_tidal_next: None,
        lastfm_api_secret: None,
        server_token: String::new(),
        audio_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        user_cleared_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        #[cfg(feature = "spotify-public")]
        spotify_public,
        sportify_client: None,
        sportify_cache_config: crate::services::sportify::cache::SportifyCacheConfig::default(),
        sportify_resolve_config: crate::services::sportify::cache::SportifyResolveConfig::default(),
    }
}

/// Build a minimal test app backed by a fresh in-memory database.
async fn build_test_app() -> Router {
    let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
    let db = Database::open(&db_path).expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| schema::run_migrations(conn))
        .expect("schema migrations");
    api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(db))))
}

fn fresh_migrated_db() -> (Database, std::path::PathBuf) {
    let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
    let db = Database::open(&db_path).expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| schema::run_migrations(conn))
        .expect("schema migrations");
    (db, db_path)
}

fn app_for_db(db: Database) -> Router {
    api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(db))))
}

#[tokio::test]
async fn tracks_route_treats_key_signature_filter_as_data() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute("INSERT INTO artists (id, name) VALUES (9001, 'Filter Artist')", [])?;
        conn.execute(
            "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, tidal_id, best_quality, best_source,
                    fidelity_score, is_favorite, source
                 ) VALUES
                    (9001, 'Minor Match', 9001, 180000, 99001, 'LOSSLESS', 'tidal', 10, 0, 'tidal'),
                    (9002, 'Major Match', 9001, 180000, 99002, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
            [],
        )?;
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, key_signature)
             VALUES (9001, 'Am'), (9002, 'C')",
            [],
        )?;
        Ok::<_, anyhow::Error>(())
    })
    .expect("seed tracks with keys");

    let app = app_for_db(db);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/tracks?key_signature=Am&limit=50")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let tracks = body["tracks"].as_array().expect("tracks array");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["title"], "Minor Match");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/tracks?key_signature=Am%27%20OR%201%3D1%20--&limit=50")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let tracks = body["tracks"].as_array().expect("tracks array");
    assert!(
        tracks.is_empty(),
        "key_signature must be treated as an exact string, got {tracks:?}"
    );

    let _ = std::fs::remove_file(db_path);
}

fn seed_dj_queue_pair(db: &Database) -> (i64, i64) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (7001, 'DJ Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES
                    (7001, 'Outgoing', 7001, 180000, 77001),
                    (7002, 'Incoming', 7001, 180000, 77002)",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (7001, 0, 'test')",
            [],
        )?;
        let current_queue_item_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (7002, 1, 'test')",
            [],
        )?;
        let next_queue_item_id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE playback_state
                 SET current_track_id = 7001, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
            params![current_queue_item_id],
        )?;
        Ok((current_queue_item_id, next_queue_item_id))
    })
    .expect("seed dj queue pair")
}

async fn json_request(
    app: Router,
    method: &str,
    uri: &str,
    body: &str,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes"),
    )
    .expect("json body")
}

#[tokio::test]
async fn dj_enabled_defaults_false() {
    let (db, db_path) = fresh_migrated_db();
    let response = json_request(app_for_db(db), "GET", "/api/dj/enabled", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["enabled"], false);
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_enabled_can_be_toggled() {
    let (db, db_path) = fresh_migrated_db();
    let app = app_for_db(db);
    let response = json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["enabled"], true);

    let response = json_request(app, "PUT", "/api/dj/enabled", r#"{"enabled":false}"#).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["enabled"], false);
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn chart_snapshots_return_latest_ranked_entries_without_zero_tidal_id() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO chart_snapshots
                    (id, source_key, region, period, chart_date, fetched_at, status)
                 VALUES
                    (1, 'spotify_daily', 'AU', 'daily', '2026-05-27', 100, 'ok'),
                    (2, 'spotify_daily', 'AU', 'daily', '2026-05-28', 200, 'ok')",
            [],
        )?;
        conn.execute(
            "INSERT INTO chart_entries
                    (id, snapshot_id, rank, rank_delta, artist, title, entity_type, streams)
                 VALUES
                    (10, 1, 1, 0, 'Old Artist', 'Old Track', 'track', 10),
                    (20, 2, 2, -1, 'Second Artist', 'Second Track', 'track', 200),
                    (21, 2, 1, 1, 'Top Artist', 'Top Track', 'track', 300)",
            [],
        )?;
        conn.execute(
            "INSERT INTO chart_entry_resolutions
                    (entry_id, status, tidal_id)
                 VALUES
                    (20, 'unresolved', NULL),
                    (21, 'tidal', 4242)",
            [],
        )?;
        Ok(())
    })
    .expect("seed chart snapshots");

    let response = json_request(
        app_for_db(db),
        "GET",
        "/api/charts/snapshots?source=spotify_daily&period=daily&region=AU&limit=2",
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;

    assert_eq!(body["snapshot"]["chart_date"], "2026-05-28");
    assert_eq!(body["entries"].as_array().unwrap().len(), 2);
    assert_eq!(body["entries"][0]["rank"], 1);
    assert_eq!(body["entries"][0]["title"], "Top Track");
    assert_eq!(body["entries"][0]["resolution_status"], "tidal");
    assert_eq!(body["entries"][0]["tidal_id"], 4242);
    assert_eq!(body["entries"][1]["rank"], 2);
    assert_eq!(body["entries"][1]["title"], "Second Track");
    assert_eq!(body["entries"][1]["resolution_status"], "unresolved");
    assert!(body["entries"][1]["tidal_id"].is_null());

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn chart_matrix_returns_provider_cells_and_explicit_missing_cells() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        queries::upsert_chart_snapshot(
            conn,
            &queries::ChartSnapshotSeed {
                source_key: "spotify_daily",
                region: "global",
                period: "daily",
                chart_date: "2026-05-27",
                fetched_at: 100,
                etag: None,
                content_hash: None,
                status: "ok",
            },
            &[queries::ChartEntrySeed {
                streams: Some(10),
                ..queries::ChartEntrySeed::track(1, "Old Artist", "Old Song")
            }],
        )?;
        queries::upsert_chart_snapshot(
            conn,
            &queries::ChartSnapshotSeed {
                source_key: "spotify_daily",
                region: "global",
                period: "daily",
                chart_date: "2026-05-28",
                fetched_at: 200,
                etag: None,
                content_hash: None,
                status: "ok",
            },
            &[
                queries::ChartEntrySeed {
                    streams: Some(500),
                    tidal_id: Some(5001),
                    resolution_status: Some("tidal"),
                    ..queries::ChartEntrySeed::track(1, "Top Artist", "Top Song")
                },
                queries::ChartEntrySeed {
                    streams: Some(400),
                    ..queries::ChartEntrySeed::track(2, "Ignored Artist", "Ignored Song")
                },
            ],
        )?;
        queries::upsert_chart_snapshot(
            conn,
            &queries::ChartSnapshotSeed {
                source_key: "spotify_daily",
                region: "global",
                period: "daily",
                chart_date: "2026-05-28",
                fetched_at: 205,
                etag: None,
                content_hash: Some("same-day-refresh"),
                status: "ok",
            },
            &[queries::ChartEntrySeed {
                streams: Some(550),
                tidal_id: Some(5001),
                resolution_status: Some("tidal"),
                ..queries::ChartEntrySeed::track(1, "Top Artist", "Top Song")
            }],
        )?;
        queries::upsert_chart_snapshot(
            conn,
            &queries::ChartSnapshotSeed {
                source_key: "youtube_daily",
                region: "global",
                period: "daily",
                chart_date: "2026-05-28",
                fetched_at: 210,
                etag: None,
                content_hash: None,
                status: "ok",
            },
            &[queries::ChartEntrySeed {
                entity_type: "video",
                views: Some(900),
                resolution_status: Some("not_playable"),
                ..queries::ChartEntrySeed::track(1, "Video Artist", "Top Video")
            }],
        )?;
        queries::upsert_chart_snapshot(
            conn,
            &queries::ChartSnapshotSeed {
                source_key: "shazam_daily",
                region: "AU",
                period: "daily",
                chart_date: "2026-05-28",
                fetched_at: 220,
                etag: None,
                content_hash: None,
                status: "ok",
            },
            &[queries::ChartEntrySeed::track(1, "AU Artist", "AU Shazam")],
        )?;
        Ok(())
    })
    .expect("seed chart matrix");

    let response = json_request(app_for_db(db), "GET", "/api/charts/matrix", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;

    let providers = body["providers"].as_array().expect("providers");
    assert_eq!(providers.len(), 6);
    assert_eq!(providers[0]["source_key"], "itunes_daily");
    assert_eq!(providers[1]["source_key"], "spotify_daily");
    assert_eq!(providers[2]["source_key"], "apple_music_daily");
    assert_eq!(providers[3]["source_key"], "youtube_daily");
    assert_eq!(providers[4]["source_key"], "shazam_daily");
    assert_eq!(providers[5]["source_key"], "deezer_daily");

    let rows = body["rows"].as_array().expect("rows");
    let global = rows
        .iter()
        .find(|row| row["region"] == "global")
        .expect("global row");
    assert_eq!(global["cells"]["spotify_daily"]["title"], "Top Song");
    assert_eq!(global["cells"]["spotify_daily"]["tidal_id"], 5001);
    assert_eq!(global["cells"]["spotify_daily"]["streams"], 550);
    assert_eq!(global["cells"]["spotify_daily"]["chart_date"], "2026-05-28");
    assert_eq!(global["cells"]["youtube_daily"]["title"], "Top Video");
    assert!(global["cells"]["itunes_daily"].is_null());
    assert!(global["cells"]["deezer_daily"].is_null());

    let au = rows
        .iter()
        .find(|row| row["region"] == "AU")
        .expect("AU row");
    assert_eq!(au["cells"]["shazam_daily"]["title"], "AU Shazam");
    assert!(au["cells"]["shazam_daily"]["tidal_id"].is_null());

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn spotify_daily_import_endpoint_persists_snapshot_for_matrix() {
    let (db, db_path) = fresh_migrated_db();
    let csv = r#"rank,uri,artist_names,track_name,peak_rank,previous_rank,days_on_chart,streams
1,spotify:track:imported,"Import Artist","Import Track",1,2,3,12345
"#;

    let response = json_request(
        app_for_db(db.clone()),
        "POST",
        "/api/charts/spotify/daily/import?region=AU&date=2026-05-28",
        csv,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["source"], "spotify_daily");
    assert_eq!(body["region"], "AU");
    assert_eq!(body["chart_date"], "2026-05-28");

    let response = json_request(app_for_db(db), "GET", "/api/charts/matrix", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let au = body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["region"] == "AU")
        .expect("AU row");
    assert_eq!(au["cells"]["spotify_daily"]["title"], "Import Track");
    assert_eq!(au["cells"]["spotify_daily"]["streams"], 12345);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_enabled_true_starts_current_pair_lookahead() {
    let (db, db_path) = fresh_migrated_db();
    let (_current, next) = seed_dj_queue_pair(&db);
    let app = app_for_db(db);
    let response = json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = json_request(app, "GET", "/api/dj/status", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["enabled"], true);
    assert_eq!(body["next"]["media_ref_kind"], "tidal_track");
    assert_eq!(body["next"]["media_ref_id"], "77002");
    assert!(next > 0);
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn persisted_dj_enabled_builds_current_pair_lookahead_without_toggle() {
    let (db, db_path) = fresh_migrated_db();
    seed_dj_queue_pair(&db);
    db.with_conn(|conn| queries::set_dj_engine_enabled(conn, true))
        .expect("enable dj");
    let state = fresh_test_state(db);

    let start = active_dj_lookahead_start_for_state(&state).expect("lookahead start");

    assert_eq!(
        start.current,
        Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
            tidal_id: 77001,
            track_id: Some(7001),
        })
    );
    assert_eq!(
        start.next,
        Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
            tidal_id: 77002,
            track_id: Some(7002),
        })
    );
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_lookahead_pairs_ephemeral_current_with_persisted_queue() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (7101, 'Queue Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES (7102, 'Queued Next', 7101, 180000, 88002)",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (7102, 0, 'user_queue')",
            [],
        )?;
        queries::set_dj_engine_enabled(conn, true)?;
        Ok(())
    })
    .expect("seed queue");
    let mut state = fresh_test_state(db);
    state.ephemeral_tidal_track = Some(crate::db::models::Track {
        id: -88001,
        title: "Direct Current".to_string(),
        artist_id: 0,
        artist_name: Some("Direct Artist".to_string()),
        album_id: None,
        album_title: None,
        disc_number: None,
        track_number: None,
        duration_ms: Some(180_000),
        isrc: None,
        tidal_id: Some(88001),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: Some("LOSSLESS".to_string()),
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: None,
    });

    let start = active_dj_lookahead_start_for_state(&state).expect("lookahead start");

    assert_eq!(
        start.current,
        Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
            tidal_id: 88001,
            track_id: None,
        })
    );
    assert_eq!(
        start.next,
        Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
            tidal_id: 88002,
            track_id: Some(7102),
        })
    );
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_enabled_false_cancels_lookahead_and_discards_program() {
    let (db, db_path) = fresh_migrated_db();
    seed_dj_queue_pair(&db);
    let app = app_for_db(db);
    let response = json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_request(
        app.clone(),
        "PUT",
        "/api/dj/enabled",
        r#"{"enabled":false}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_request(app, "GET", "/api/dj/status", "").await;
    let body = response_json(response).await;
    assert_eq!(body["enabled"], false);
    assert_eq!(body["fallback_reason"], "disabled");
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_profile_rebuild_does_not_claim_accepted_without_job() {
    let (db, db_path) = fresh_migrated_db();
    seed_dj_queue_pair(&db);
    let app = app_for_db(db);
    let response = json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_request(
        app,
        "POST",
        "/api/dj/profile-rebuild",
        r#"{"media_ref_kind":"tidal_track","media_ref_id":"77001"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["accepted"], false);
    assert_eq!(body["status"], "source_unavailable");
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_profile_rebuild_does_not_mutate_armed_transition() {
    let (db, db_path) = fresh_migrated_db();
    seed_dj_queue_pair(&db);
    db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, outcome
                 ) VALUES ('tidal_track', '77001', 'tidal_track', '77002', 'SafeCrossfade', '{}', 'v1', 'armed')",
                [],
            )?;
            Ok(())
        })
        .expect("seed transition");
    let app = app_for_db(db.clone());
    let response = json_request(
        app,
        "POST",
        "/api/dj/profile-rebuild",
        r#"{"media_ref_kind":"tidal_track","media_ref_id":"77001"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: String = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT outcome FROM dj_transition_events LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .expect("outcome");
    assert_eq!(outcome, "armed");
    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn dj_profile_returns_404_for_missing_profile() {
    let (db, db_path) = fresh_migrated_db();
    let response = json_request(app_for_db(db), "GET", "/api/dj/profile/999", "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_file(db_path);
}

fn seed_spotify_stats_track(
    db: &Database,
    album_id: Option<i64>,
    title: &str,
    isrc: &str,
    spotify_track_id: &str,
    playcount: i64,
) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (42, 'Stats Artist')",
            [],
        )?;
        if let Some(album_id) = album_id {
            conn.execute(
                "INSERT INTO albums (id, title, artist_id, source)
                     VALUES (?1, 'Stats Album', 42, 'tidal')",
                rusqlite::params![album_id],
            )?;
        }
        conn.execute(
            "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, isrc, source, fidelity_score
                 ) VALUES (77, ?1, 42, ?2, 180000, ?3, 'tidal_stream', 0)",
            rusqlite::params![title, album_id, isrc],
        )?;

        let track = crate::services::sportify::models::SportifyTrack {
            id: Some(spotify_track_id.to_string()),
            playcount: Some(playcount),
            external_ids: Some(crate::services::sportify::models::SportifyExternalIds {
                isrc: Some(isrc.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        crate::services::sportify::stats::write_track_playcount(conn, &track);
        Ok::<_, anyhow::Error>(())
    })
    .expect("seed spotify stats track");
}

#[test]
fn background_resolution_batches_are_bounded_by_concurrency() {
    let inputs: Vec<(String, crate::services::sportify::models::SportifyTrack)> = (0..13)
        .map(|i| {
            (
                format!("sp-{i}"),
                crate::services::sportify::models::SportifyTrack {
                    id: Some(format!("sp-{i}")),
                    name: Some(format!("Track {i}")),
                    artist: Some("Artist".to_string()),
                    ..Default::default()
                },
            )
        })
        .collect();

    let batches = background_resolution_batches(&inputs, 6);

    assert_eq!(batches.len(), 3);
    assert_eq!(batches[0].len(), 6);
    assert_eq!(batches[1].len(), 6);
    assert_eq!(batches[2].len(), 1);
}

#[tokio::test]
async fn album_spotify_stats_returns_cached_playcounts() {
    let (db, db_path) = fresh_migrated_db();
    seed_spotify_stats_track(
        &db,
        Some(9),
        "Album Stats Track",
        "ISRCALBUMSTATS",
        "sp-album-stats",
        1_234,
    );
    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/albums/9/spotify-stats")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes"),
    )
    .expect("json body");
    let tracks = body["tracks"].as_array().expect("tracks array");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["isrc"], "ISRCALBUMSTATS");
    assert_eq!(tracks[0]["title"], "Album Stats Track");
    assert_eq!(tracks[0]["playcount"], 1_234);
    assert!(body["monthly_listeners"].is_null());

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn artist_spotify_stats_returns_cached_playcounts() {
    let (db, db_path) = fresh_migrated_db();
    seed_spotify_stats_track(
        &db,
        None,
        "Artist Stats Track",
        "ISRCARTISTSTATS",
        "sp-artist-stats",
        5_678,
    );
    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/artists/42/spotify-stats")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes"),
    )
    .expect("json body");
    let tracks = body["tracks"].as_array().expect("tracks array");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["isrc"], "ISRCARTISTSTATS");
    assert_eq!(tracks[0]["title"], "Artist Stats Track");
    assert_eq!(tracks[0]["playcount"], 5_678);
    assert!(body["monthly_listeners"].is_null());

    let _ = std::fs::remove_file(db_path);
}

fn seed_basic_tracks(db: &Database) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, source, fidelity_score
                 ) VALUES
                    (1, 'First Track', 1, 180000, 'tidal_stream', 0),
                    (2, 'Second Track', 1, 180000, 'tidal_stream', 0)",
            [],
        )?;
        Ok(())
    })
    .expect("seed tracks");
}

#[tokio::test]
async fn clear_queue_returns_snapshot_and_preserves_current() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let current_qid: i64 = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO tracks (
                        id, title, artist_id, duration_ms, source, fidelity_score
                     ) VALUES (3, 'Third Track', 1, 180000, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (3, 2, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![qid],
            )?;
            Ok(qid)
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], current_qid);
    assert_eq!(queue[0]["track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_queue_item_id"], current_qid);

    let persisted_queue_count: i64 = db
        .with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(persisted_queue_count, 1);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn clear_queue_preserves_only_current_queue_item_for_duplicate_track() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (duplicate_qid, current_qid): (i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let duplicate_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 1, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 2, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((duplicate_qid, current_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], current_qid);
    assert_ne!(queue[0]["id"], duplicate_qid);
    assert_eq!(queue[0]["track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_queue_item_id"], current_qid);

    let persisted_ids: Vec<i64> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })
        .unwrap();
    assert_eq!(persisted_ids, vec![current_qid]);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn clear_queue_repairs_mismatched_anchor_before_preserving_current() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, mismatched_qid): (i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let mismatched_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![mismatched_qid],
            )?;
            Ok((current_qid, mismatched_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], current_qid);
    assert_ne!(queue[0]["id"], mismatched_qid);
    assert_eq!(queue[0]["track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_queue_item_id"], current_qid);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn clear_queue_falls_back_to_track_id_when_current_queue_item_is_stale() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let stale_qid = 999_999_i64;
    let preserved_qid = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let preserved_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![stale_qid],
            )?;
            Ok(preserved_qid)
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], preserved_qid);
    assert_eq!(queue[0]["track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_track"]["id"], 1);
    assert_eq!(
        body["playback_state"]["current_queue_item_id"],
        preserved_qid
    );

    let (persisted_track_ids, current_queue_item_id): (Vec<i64>, Option<i64>) = db
        .with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT track_id FROM queue ORDER BY position ASC, id ASC")?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let current_queue_item_id = conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            Ok((ids, current_queue_item_id))
        })
        .unwrap();
    assert_eq!(persisted_track_ids, vec![1]);
    assert_eq!(current_queue_item_id, Some(preserved_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn clear_queue_repairs_missing_anchor_and_removes_duplicate_track_rows() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (first_qid, duplicate_qid): (i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let first_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 1, 'user')",
                [],
            )?;
            let duplicate_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 2, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = NULL, is_playing = 1
                     WHERE id = 1",
                [],
            )?;
            Ok((first_qid, duplicate_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], first_qid);
    assert_ne!(queue[0]["id"], duplicate_qid);
    assert_eq!(queue[0]["track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_queue_item_id"], first_qid);

    let persisted_ids: Vec<i64> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })
        .unwrap();
    assert_eq!(persisted_ids, vec![first_qid]);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn remove_current_queue_item_advances_and_switches_runtime() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 2);
                assert_eq!(generation, 2);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                assert_eq!(job.generation, 2);
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard.tidal_tokens = Some(tidal_auth::TidalTokens {
            access_token: "test-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            user_id: "test-user".to_string(),
            country_code: "US".to_string(),
            auth_flow: Some("pkce".to_string()),
        });
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    let app = api_routes(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/remove")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "queue_item_id": current_qid })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], next_qid);
    assert_eq!(body["playback_state"]["current_track"]["id"], 2);
    assert_eq!(body["playback_state"]["current_queue_item_id"], next_qid);

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        2
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(2));
    assert_eq!(current_queue_item_id, Some(next_qid));
    assert!(is_playing);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn remove_current_queue_item_repairs_stale_anchor_and_returns_playback_state() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let stale_qid = 999_999_i64;
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 0
                     WHERE id = 1",
                rusqlite::params![stale_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/remove")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "queue_item_id": current_qid })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], next_qid);
    assert_eq!(body["playback_state"]["current_track"]["id"], 2);
    assert_eq!(body["playback_state"]["current_queue_item_id"], next_qid);
    assert_eq!(body["playback_state"]["is_playing"], false);

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(2));
    assert_eq!(current_queue_item_id, Some(next_qid));
    assert!(!is_playing);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn remove_current_queue_item_repairs_mismatched_anchor_and_returns_playback_state() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 0
                     WHERE id = 1",
                rusqlite::params![next_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/remove")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "queue_item_id": current_qid })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], next_qid);
    assert_eq!(body["playback_state"]["current_track"]["id"], 2);
    assert_eq!(body["playback_state"]["current_queue_item_id"], next_qid);
    assert_eq!(body["playback_state"]["is_playing"], false);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn move_queue_item_repairs_stale_current_anchor_and_returns_playback_state() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let stale_qid = 999_999_i64;
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![stale_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/move")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "item_id": current_qid, "new_pos": 1 })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0]["id"], next_qid);
    assert_eq!(queue[1]["id"], current_qid);
    assert_eq!(body["playback_state"]["current_track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_queue_item_id"], current_qid);

    let current_queue_item_id: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(current_queue_item_id, Some(current_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn move_queue_item_repairs_mismatched_current_anchor_and_returns_playback_state() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, mismatched_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let mismatched_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![mismatched_qid],
            )?;
            Ok((current_qid, mismatched_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/move")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "item_id": current_qid, "new_pos": 1 })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let queue = body["queue"].as_array().expect("queue array");
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0]["id"], mismatched_qid);
    assert_eq!(queue[1]["id"], current_qid);
    assert_eq!(body["playback_state"]["current_track"]["id"], 1);
    assert_eq!(body["playback_state"]["current_queue_item_id"], current_qid);

    let current_queue_item_id: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(current_queue_item_id, Some(current_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn playback_shuffle_returns_debug_and_persists_seed() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/shuffle")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mode":"true"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let seed = body["shuffle_debug"]["seed"]
        .as_i64()
        .expect("positive seed");
    assert!(seed > 0);
    assert_eq!(body["shuffle_debug"]["mode"], "true");
    assert_eq!(body["shuffle_debug"]["scope"], "playback_state");

    let stored_seed: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT shuffle_seed FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(stored_seed, Some(seed));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn replace_playback_queue_accepts_one_shot_shuffle_mode() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"track_ids":[1,2],"shuffle_mode":"true"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["queue"].as_array().expect("queue").len(), 2);
    assert_eq!(body["shuffle_debug"]["mode"], "true");
    assert_eq!(body["shuffle_debug"]["scope"], "queue_replace");

    let _ = std::fs::remove_file(db_path);
}

#[test]
fn tidal_play_mix_shuffle_returns_debug_and_preserves_tracks() {
    let mut tracks = vec![
        PlayTidalRequest {
            tidal_track_id: 101,
            title: "One".to_string(),
            artist_name: Some("Artist A".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(180_000),
        },
        PlayTidalRequest {
            tidal_track_id: 102,
            title: "Two".to_string(),
            artist_name: Some("Artist B".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(181_000),
        },
        PlayTidalRequest {
            tidal_track_id: 103,
            title: "Three".to_string(),
            artist_name: Some("Artist C".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(182_000),
        },
    ];

    let debug = shuffle_tidal_mix_tracks(&mut tracks, Some("true")).expect("shuffle debug");
    let mut ids: Vec<i64> = tracks.iter().map(|track| track.tidal_track_id).collect();
    ids.sort_unstable();

    assert_eq!(ids, vec![101, 102, 103]);
    assert_eq!(debug.mode, "true");
    assert!(debug.seed > 0);
    assert_eq!(debug.scope, "tidal_mix");
    assert_eq!(debug.locked_count, 0);
    assert_eq!(debug.candidate_count, 3);
}

#[tokio::test]
async fn genre_snapshot_route_returns_galaxy_payload() {
    let app = build_test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/genres/snapshot?days=30")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert!(body["genres"].is_array());
    assert!(body["heat"].is_array());
    assert!(body["cohorts"].is_array());
    assert!(body["evolution"].is_array());
    assert!(body["metrics"].is_array());
    assert_eq!(body["filter"], "confidence_0_50");
}

#[tokio::test]
async fn discovery_space_includes_resolved_sidecar_external_neighbors() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES (1, 'Seed Track', 1, 200000, 1001)",
            [],
        )?;
        let model = queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:space-external",
            "discovery-fusion-v2",
            2,
            "ready",
            None,
        )?;
        queries::activate_embedding_model(conn, model.id)?;
        let unresolved = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "unresolved-space".to_string(),
                title: "Unresolved External".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        let resolved = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(990_001),
                mbid: None,
                dedupe_key: "tidal:990001".to_string(),
                title: "Resolved External".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(181_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        queries::replace_external_candidate_neighbors(
            conn,
            model.id,
            1,
            &[
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: unresolved.id,
                    rank: 1,
                    score: 0.99,
                    audio_score: 0.99,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                },
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: resolved.id,
                    rank: 2,
                    score: 0.91,
                    audio_score: 0.91,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                },
            ],
        )?;
        Ok(())
    })
    .expect("seed discovery space");

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/discovery/space")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"seed_track_id":1,"mode":"radio","limit":20}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let tracks = body["tracks"].as_array().expect("tracks array");
    let external = tracks
        .iter()
        .find(|track| track["track_id"] == 990_001)
        .expect("resolved external sidecar node");
    assert_eq!(external["source"], "external");
    assert_eq!(external["is_in_library"], false);
    assert_eq!(external["primary_reason"], "external");
    assert!(
        tracks
            .iter()
            .all(|track| track["title"] != "Unresolved External"),
        "unresolved external candidate must stay hidden"
    );
    let edges = body["edges"].as_array().expect("edges array");
    assert!(
        edges
            .iter()
            .any(|edge| { edge["from_track_id"] == 1 && edge["to_track_id"] == 990_001 })
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn discovery_blend_space_includes_pending_external_nodes_and_health() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES
                    (1, 'Seed One', 1, 200000, 1001),
                    (2, 'Seed Two', 1, 201000, 1002)",
            [],
        )?;
        let model = queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:blend-space",
            "discovery-fusion-v2",
            2,
            "ready",
            None,
        )?;
        queries::activate_embedding_model(conn, model.id)?;
        let pending = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "pending-blend".to_string(),
                title: "Pending Blend".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        let resolved = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(990_002),
                mbid: None,
                dedupe_key: "tidal:990002".to_string(),
                title: "Resolved Blend".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(181_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        for seed_id in [1, 2] {
            queries::replace_external_candidate_neighbors(
                conn,
                model.id,
                seed_id,
                &[
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: pending.id,
                        rank: 1,
                        score: 0.94,
                        audio_score: 0.94,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: resolved.id,
                        rank: 2,
                        score: 0.90,
                        audio_score: 0.90,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                ],
            )?;
        }
        Ok(())
    })
    .expect("seed blend space");

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/discovery/blend/space")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"seeds":[{"kind":"library","track_id":1},{"kind":"library","track_id":2}],"limit":20}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let tracks = body["tracks"].as_array().expect("tracks array");
    let pending = tracks
        .iter()
        .find(|track| track["title"] == "Pending Blend")
        .expect("pending external blend node");
    assert_eq!(pending["role"], "external_candidate");
    assert_eq!(pending["playability"], "pending");
    assert_eq!(body["health"]["pending_external_count"], 1);
    assert_eq!(body["health"]["playable_external_count"], 1);
    assert!(body["health"]["coverage_ratio"].as_f64().unwrap() > 0.0);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn discovery_blend_space_uses_external_seed_anchors() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Guide Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES
                    (1, 'Guide One', 1, 200000, 1001),
                    (2, 'Guide Two', 1, 201000, 1002)",
            [],
        )?;
        let model = queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:blend-external-seeds",
            "discovery-fusion-v2",
            2,
            "ready",
            None,
        )?;
        queries::activate_embedding_model(conn, model.id)?;
        let external_seed_one = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(991_001),
                mbid: None,
                dedupe_key: "tidal:991001".to_string(),
                title: "External Seed One".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        let external_seed_two = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(991_002),
                mbid: None,
                dedupe_key: "tidal:991002".to_string(),
                title: "External Seed Two".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(181_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        let shared_discovery = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(991_003),
                mbid: None,
                dedupe_key: "tidal:991003".to_string(),
                title: "Shared Discovery".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(182_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        queries::replace_external_candidate_neighbors(
            conn,
            model.id,
            1,
            &[
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: external_seed_one.id,
                    rank: 1,
                    score: 0.99,
                    audio_score: 0.99,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                },
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: shared_discovery.id,
                    rank: 2,
                    score: 0.91,
                    audio_score: 0.91,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                },
            ],
        )?;
        queries::replace_external_candidate_neighbors(
            conn,
            model.id,
            2,
            &[
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: external_seed_two.id,
                    rank: 1,
                    score: 0.99,
                    audio_score: 0.99,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                },
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: shared_discovery.id,
                    rank: 2,
                    score: 0.89,
                    audio_score: 0.89,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                },
            ],
        )?;
        Ok(())
    })
    .expect("seed external blend anchors");

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/discovery/blend/space")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"seeds":[{"kind":"tidal","tidal_id":991001,"title":"External Seed One","artist":"Outside"},{"kind":"tidal","tidal_id":991002,"title":"External Seed Two","artist":"Outside"}],"limit":20}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let tracks = body["tracks"].as_array().expect("tracks array");
    assert!(
        tracks
            .iter()
            .any(|track| track["title"] == "Shared Discovery"),
        "external blend seeds should produce anchored discoveries"
    );
    assert_eq!(body["health"]["playable_external_count"], 1);
    assert!(body["health"]["coverage_ratio"].as_f64().unwrap() > 0.0);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn discovery_blend_add_appends_discoveries_without_replacing_queue() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES (1, 'Seed One', 1, 200000, 1001)",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user_queue')",
            [],
        )?;
        let model = queries::create_embedding_model(
            conn,
            "discovery-fusion-v2:blend-add",
            "discovery-fusion-v2",
            2,
            "ready",
            None,
        )?;
        queries::activate_embedding_model(conn, model.id)?;
        let resolved = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(990_003),
                mbid: None,
                dedupe_key: "tidal:990003".to_string(),
                title: "Resolved Add".to_string(),
                artist_name: "Outside".to_string(),
                genre_tags_json: None,
                duration_ms: Some(181_000),
                expires_at: "2099-01-01 00:00:00".to_string(),
            },
        )?;
        queries::replace_external_candidate_neighbors(
            conn,
            model.id,
            1,
            &[queries::ExternalCandidateNeighborWriteRow {
                candidate_id: resolved.id,
                rank: 1,
                score: 0.95,
                audio_score: 0.95,
                metadata_score: 0.0,
                reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
            }],
        )?;
        Ok(())
    })
    .expect("seed blend add");

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/discovery/blend/add")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"seeds":[{"kind":"library","track_id":1}],"limit":10}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["pending_count"], 1);
    db.with_conn(|conn| {
        let rows: Vec<(i32, Option<i64>, Option<i64>)> = conn
            .prepare("SELECT position, track_id, tidal_id_hint FROM queue ORDER BY position ASC")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(rows, vec![(0, Some(1), None), (1, None, Some(990_003))]);
        Ok(())
    })
    .unwrap();

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn tidal_mix_overlay_preserves_pending_deque_order() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (8101, 'Old Playlist Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms)
                 VALUES
                    (8101, 'Old Playlist First', 8101, 200000),
                    (8102, 'Old Playlist Second', 8101, 200000)",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source)
                 VALUES (8101, 0, 'playlist'), (8102, 1, 'playlist')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    {
        let mut guard = state.write().await;
        guard.ephemeral_tidal_track = Some(crate::db::models::Track {
            id: -158_296_914,
            title: "The Harder They Come".to_string(),
            artist_id: 0,
            artist_name: Some("Jimmy Cliff".to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: Some(220_000),
            isrc: None,
            tidal_id: Some(158_296_914),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some("tidal".to_string()),
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal_ephemeral".to_string(),
            artwork_url: None,
        });
        let mut pending = guard.pending_tidal_mix_queue.lock().unwrap();
        pending.push_back(crate::PendingEphemeralTidalTrack {
            tidal_track_id: 873_891_22,
            title: "The Big Tree".to_string(),
            artist_name: Some("Stand High Patrol".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(180_000),
        });
        pending.push_back(crate::PendingEphemeralTidalTrack {
            tidal_track_id: 341_378_223,
            title: "Positive".to_string(),
            artist_name: Some("Jamback".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(170_000),
        });
        pending.push_back(crate::PendingEphemeralTidalTrack {
            tidal_track_id: 172_522_829,
            title: "Moi Aussi Marianne".to_string(),
            artist_name: Some("Yatoba Lia".to_string()),
            album_title: None,
            artwork_url: None,
            duration_ms: Some(210_000),
        });
    }

    let snapshot = {
        let guard = state.read().await;
        guard.db.with_conn(player::load_snapshot).unwrap()
    };
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;

    assert_eq!(
        snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.title.as_str()),
        Some("The Harder They Come")
    );
    let titles = snapshot
        .queue
        .iter()
        .map(|item| item.track.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        titles,
        vec!["The Big Tree", "Positive", "Moi Aussi Marianne"]
    );
    assert!(
        snapshot.queue.iter().all(|item| item.source == "tidal_mix"),
        "active TIDAL mix overlay must shadow the stale durable playlist queue"
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn tidal_mix_overlay_preserves_large_loaded_album_rows() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (8301, 'Unrelated Playlist Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms)
                 VALUES
                    (8301, 'Unrelated Queue First', 8301, 200000),
                    (8302, 'Unrelated Queue Second', 8301, 200000)",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source)
                 VALUES (8301, 0, 'playlist'), (8302, 1, 'playlist')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    let album_tidal_id = 58_520_793_i64;
    let tidal_track_base = album_tidal_id * 100;
    {
        let mut current = test_track(-(tidal_track_base + 1), "Anthology Track 1");
        current.artist_id = 0;
        current.artist_name = Some("The Beatles".to_string());
        current.album_title = Some("Anthology 2".to_string());
        current.tidal_id = Some(tidal_track_base + 1);
        current.source = "tidal_ephemeral".to_string();
        current.artwork_url =
            Some("https://resources.tidal.com/images/anthology-1/640x640.jpg".to_string());

        let mut guard = state.write().await;
        guard.ephemeral_tidal_track = Some(current);
        let mut pending = guard.pending_tidal_mix_queue.lock().unwrap();
        for track_number in 2..=45 {
            pending.push_back(crate::PendingEphemeralTidalTrack {
                tidal_track_id: tidal_track_base + track_number,
                title: format!("Anthology Track {track_number}"),
                artist_name: Some("The Beatles".to_string()),
                album_title: Some("Anthology 2".to_string()),
                artwork_url: Some(format!(
                    "https://resources.tidal.com/images/anthology-{track_number}/640x640.jpg"
                )),
                duration_ms: Some(180_000 + track_number),
            });
        }
    }

    let snapshot = {
        let guard = state.read().await;
        guard.db.with_conn(player::load_snapshot).unwrap()
    };
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;

    assert_eq!(
        snapshot
            .state
            .current_track
            .as_ref()
            .map(|track| track.title.as_str()),
        Some("Anthology Track 1")
    );
    assert_eq!(snapshot.queue.len(), 44);
    assert_eq!(
        snapshot.queue.first().map(|item| item.track.title.as_str()),
        Some("Anthology Track 2")
    );
    assert_eq!(
        snapshot.queue.last().map(|item| item.track.title.as_str()),
        Some("Anthology Track 45")
    );
    assert_eq!(
        snapshot
            .queue
            .iter()
            .map(|item| item.track.tidal_id.expect("tidal id"))
            .collect::<Vec<_>>(),
        (2..=45)
            .map(|track_number| tidal_track_base + track_number)
            .collect::<Vec<_>>()
    );
    assert!(
        snapshot.queue.iter().all(|item| item.source == "tidal_mix"
            && item.track.album_title.as_deref() == Some("Anthology 2")
            && item
                .track
                .artwork_url
                .as_deref()
                .is_some_and(|url| url.contains("resources.tidal.com"))),
        "visible TIDAL album queue rows must preserve album metadata and artwork"
    );
    assert!(
        snapshot
            .queue
            .iter()
            .all(|item| !item.track.title.starts_with("Unrelated Queue")),
        "active TIDAL album overlay must not leak stale durable queue rows"
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn tidal_mix_replacement_clears_stale_persisted_queue() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (8200, 'Old Queue Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8201, 'Old Queue First', 8200, 200000, 88201, 'tidal', 'tidal'),
                    (8202, 'Old Queue Second', 8200, 200000, 88202, 'tidal', 'tidal')",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source)
                 VALUES (8201, 0, 'playlist'), (8202, 1, 'playlist')",
            [],
        )?;
        Ok::<_, anyhow::Error>(())
    })
    .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    {
        let guard = state.read().await;
        guard.pending_tidal_mix_queue.lock().unwrap().push_back(
            crate::PendingEphemeralTidalTrack {
                tidal_track_id: 123_456,
                title: "Album Track Two".to_string(),
                artist_name: Some("Album Artist".to_string()),
                album_title: Some("Album".to_string()),
                artwork_url: Some(
                    "https://resources.tidal.com/images/a/b/c/320x320.jpg".to_string(),
                ),
                duration_ms: Some(180_000),
            },
        );
    }

    if let Err((status, _)) = clear_persisted_queue_for_tidal_mix(&state).await {
        panic!("clear_persisted_queue_for_tidal_mix failed: {status}");
    }

    let queue_len = db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get::<_, i64>(0))
                .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(queue_len, 0);
    assert_eq!(
        state
            .read()
            .await
            .pending_tidal_mix_queue
            .lock()
            .unwrap()
            .len(),
        1,
        "durable queue cleanup must preserve the active TIDAL continuation"
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn clear_queue_clears_pending_tidal_mix_overlay() {
    let (db, db_path) = fresh_migrated_db();
    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    {
        let guard = state.read().await;
        guard.pending_tidal_mix_queue.lock().unwrap().push_back(
            crate::PendingEphemeralTidalTrack {
                tidal_track_id: 987_654,
                title: "Queued TIDAL Mix Track".to_string(),
                artist_name: Some("TIDAL Artist".to_string()),
                album_title: Some("TIDAL Mix".to_string()),
                artwork_url: None,
                duration_ms: Some(180_000),
            },
        );
    }
    let app = api_routes(state.clone());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playback/queue/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/playback/queue")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let queue = body["queue"].as_array().expect("queue array");
    assert!(
        queue.is_empty(),
        "pending TIDAL mix overlay must not reappear after clear"
    );
    assert!(
        state
            .read()
            .await
            .pending_tidal_mix_queue
            .lock()
            .unwrap()
            .is_empty(),
        "pending TIDAL mix deque must be cleared"
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn direct_tidal_finish_advances_persisted_queue_and_switches_runtime() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (8100, 'Queued Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES (8101, 'Queued Track', 8100, 180000, 88101, 'tidal', 'tidal')",
            [],
        )?;
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (8101, 0, 'test')",
            [],
        )?;
        conn.execute(
            "UPDATE playback_state
                 SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 1
                 WHERE id = 1",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let direct_track = crate::db::models::Track {
        id: -441,
        title: "Direct TIDAL".to_string(),
        artist_id: 0,
        artist_name: Some("Direct Artist".to_string()),
        album_id: None,
        album_title: None,
        disc_number: None,
        track_number: None,
        duration_ms: Some(180_000),
        isrc: None,
        tidal_id: Some(441),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: Some("LOSSLESS".to_string()),
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: None,
    };

    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 8101);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard.tidal_tokens = Some(tidal_auth::TidalTokens {
            access_token: "test-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            user_id: "test-user".to_string(),
            country_code: "US".to_string(),
            auth_flow: Some("pkce".to_string()),
        });
        guard.external_playback_track = Some(direct_track.clone());
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(direct_track.id),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_finished(state.clone(), direct_track.id, 1)
        .await
        .expect("runtime finish");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        8101
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, is_playing): (Option<i64>, bool) = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, is_playing FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(8101));
    assert!(is_playing);

    let guard = state.read().await;
    assert!(guard.external_playback_track.is_none());
    assert_eq!(
        guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id),
        Some(8101)
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn runtime_finish_skips_unresolved_pending_row_and_starts_next_library_track() {
    let (db, db_path) = fresh_migrated_db();
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8200, 'Queued Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8201, 'Finished Track', 8200, 180000, 88201, 'tidal', 'tidal'),
                    (8202, 'Next Library Track', 8200, 180000, 88202, 'tidal', 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8201, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 1, 'radio_pending', 'Missing Artist', 'Missing Title', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8202, 2, 'test')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 8201, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();
    assert!(current_qid > 0);

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 8202);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard.tidal_tokens = Some(tidal_auth::TidalTokens {
            access_token: "test-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            user_id: "test-user".to_string(),
            country_code: "US".to_string(),
            auth_flow: Some("pkce".to_string()),
        });
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(8201),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_finished(state.clone(), 8201, 1)
        .await
        .expect("runtime finish");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        8202
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(8202));
    assert_eq!(current_queue_item_id, Some(next_qid));
    assert!(is_playing);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn runtime_finish_adopts_pending_row_resolved_by_background_resolver() {
    let (db, db_path) = fresh_migrated_db();
    let (current_qid, pending_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8300, 'Queued Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8301, 'Finished Track', 8300, 180000, 88301, 'tidal', 'tidal'),
                    (8302, 'Unrelated Next Track', 8300, 180000, 88302, 'tidal', 'tidal'),
                    (8303, 'Background Resolved Track', 8300, 180000, 88303, 'tidal', 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8301, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, resolving_at
                 ) VALUES (
                    NULL, 1, 'radio_pending', 'Resolved Artist', 'Resolved Title',
                    datetime('now'), datetime('now')
                 )",
                [],
            )?;
            let pending_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8302, 2, 'test')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 8301, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, pending_qid))
        })
        .unwrap();
    assert!(current_qid > 0);

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 8303);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard.tidal_tokens = Some(tidal_auth::TidalTokens {
            access_token: "test-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            user_id: "test-user".to_string(),
            country_code: "US".to_string(),
            auth_flow: Some("pkce".to_string()),
        });
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(8301),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    let db_for_resolve = db.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(
            PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS / 2,
        ))
        .await;
        db_for_resolve
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE queue
                     SET track_id = 8303,
                         resolving_at = NULL,
                         resolved_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![pending_qid],
                )?;
                Ok(())
            })
            .expect("promote pending row");
    });

    handle_runtime_finished(state.clone(), 8301, 1)
        .await
        .expect("runtime finish");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        8303
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(8303));
    assert_eq!(current_queue_item_id, Some(pending_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn manual_previous_skips_unresolved_pending_rows_to_prior_library_track() {
    let (db, db_path) = fresh_migrated_db();
    let (previous_qid, current_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8400, 'Previous Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id, best_source, source)
                 VALUES
                    (8401, 'Previous Library Track', 8400, 180000, 88401, 'tidal', 'tidal'),
                    (8402, 'Current Library Track', 8400, 180000, 88402, 'tidal', 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8401, 0, 'test')",
                [],
            )?;
            let previous_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 1, 'radio_pending', 'Missing Artist A', 'Missing Title A', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 2, 'radio_pending', 'Missing Artist B', 'Missing Title B', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (8402, 3, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 8402, current_queue_item_id = ?1, is_playing = 1, position_ms = 0
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((previous_qid, current_qid))
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let playback_generation = bump_playback_generation(&state).await;
    let snapshot = previous_persisted_playback_snapshot(&state)
        .await
        .expect("initial previous snapshot");
    assert!(snapshot.state.current_track.is_none());
    assert_ne!(snapshot.state.current_queue_item_id, Some(current_qid));

    let snapshot = resolve_or_skip_pending_current_previous(
        &state,
        snapshot,
        playback_generation,
        "manual_previous_track",
    )
    .await
    .expect("previous pending skip");

    assert_eq!(
        snapshot.state.current_track.as_ref().map(|track| track.id),
        Some(8401)
    );
    assert_eq!(snapshot.state.current_queue_item_id, Some(previous_qid));
    assert!(snapshot.state.is_playing);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn manual_previous_stops_when_pending_rows_cannot_move_back() {
    let (db, db_path) = fresh_migrated_db();
    let current_qid = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 0, 'radio_pending', 'Missing Artist A', 'Missing Title A', datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at
                 ) VALUES (NULL, 1, 'radio_pending', 'Missing Artist B', 'Missing Title B', datetime('now'))",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = NULL, current_queue_item_id = ?1, is_playing = 1, position_ms = 0
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok(current_qid)
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let playback_generation = bump_playback_generation(&state).await;
    let snapshot = previous_persisted_playback_snapshot(&state)
        .await
        .expect("initial previous snapshot");
    assert!(snapshot.state.current_track.is_none());
    assert_ne!(snapshot.state.current_queue_item_id, Some(current_qid));

    let snapshot = resolve_or_skip_pending_current_previous(
        &state,
        snapshot,
        playback_generation,
        "manual_previous_track",
    )
    .await
    .expect("previous pending stop");

    assert!(snapshot.state.current_track.is_none());
    assert_eq!(snapshot.state.current_queue_item_id, None);
    assert!(!snapshot.state.is_playing);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn prepared_runtime_track_error_keeps_current_playback_running() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let current_qid: i64 = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'test')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok(current_qid)
        })
        .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    {
        let mut guard = state.write().await;
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_prepared_runtime_track_error(&state, 2, "prebuffer decode failed").await;

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(1));
    assert_eq!(current_queue_item_id, Some(current_qid));
    assert!(is_playing);

    let guard = state.read().await;
    let info = guard.playback_runtime_info.as_ref().expect("runtime info");
    assert_eq!(info.active_track_id, Some(1));
    assert_eq!(info.last_error.as_deref(), Some("prebuffer decode failed"));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn runtime_track_error_advances_to_next_library_track() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (current_qid, next_qid) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'test')",
                [],
            )?;
            let next_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((current_qid, next_qid))
        })
        .unwrap();
    assert!(current_qid > 0);

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (switched_tx, switched_rx) = std::sync::mpsc::channel();
    let runtime_thread = std::thread::spawn(move || {
        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("track status command")
        {
            playback_runtime::PlaybackRuntimeCommand::TrackStatus {
                track_id,
                generation,
                respond_to,
            } => {
                assert_eq!(track_id, 2);
                assert_eq!(generation, 1);
                respond_to
                    .send(playback_runtime::PlaybackTrackStatus::Prepared)
                    .expect("track status response");
            }
            other => panic!("expected TrackStatus command, got {other:?}"),
        }

        match command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switch command")
        {
            playback_runtime::PlaybackRuntimeCommand::Switch(job) => {
                assert_eq!(job.generation, 1);
                switched_tx.send(job.track.id).expect("switched track id");
            }
            other => panic!("expected Switch command, got {other:?}"),
        }
    });

    {
        let mut guard = state.write().await;
        guard.tidal_tokens = Some(tidal_auth::TidalTokens {
            access_token: "test-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            user_id: "test-user".to_string(),
            country_code: "US".to_string(),
            auth_flow: Some("pkce".to_string()),
        });
        guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token: "test-token".to_string(),
            handle: playback_runtime::PlaybackRuntimeHandle::test_with_command_tx(command_tx),
        });
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 48_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: false,
            exclusive_transport_format: None,
        });
    }

    handle_runtime_track_error(state.clone(), 1, 1, "active decode failed")
        .await
        .expect("track error should advance");

    assert_eq!(
        switched_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("switched track"),
        2
    );
    runtime_thread.join().expect("runtime thread");

    let (current_track_id, current_queue_item_id, is_playing): (Option<i64>, Option<i64>, bool) =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id, current_queue_item_id, is_playing
                 FROM playback_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
    assert_eq!(current_track_id, Some(2));
    assert_eq!(current_queue_item_id, Some(next_qid));
    assert!(is_playing);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn disabling_exclusive_clears_runtime_engaged_state() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
        let mut settings = crate::db::audio_settings::AudioSettings::default();
        settings.exclusive_mode = true;
        crate::db::audio_settings::save(conn, &settings)?;
        Ok(())
    })
    .unwrap();

    let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
    {
        let mut guard = state.write().await;
        guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
            device_name: "Test DAC".to_string(),
            sample_rate: 96_000,
            channels: 2,
            active_track_id: Some(1),
            last_error: None,
            exclusive_engaged: true,
            exclusive_transport_format: Some("i24-in-32".to_string()),
        });
    }
    let app = api_routes(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/playback/runtime")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["runtime"]["exclusive_transport_format"], "i24-in-32");

    let mut next_settings = crate::db::audio_settings::AudioSettings::default();
    next_settings.exclusive_mode = false;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/audio/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&next_settings).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/playback/runtime")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["runtime"]["exclusive_engaged"], false);
    assert_eq!(body["runtime"]["exclusive_transport_format"], Value::Null);

    let _ = std::fs::remove_file(db_path);
}

#[test]
fn retries_exclusive_release_only_while_playing_with_exclusive_enabled() {
    assert!(should_retry_exclusive_release(true, true));
    assert!(!should_retry_exclusive_release(false, true));
    assert!(!should_retry_exclusive_release(true, false));
    assert!(!should_retry_exclusive_release(false, false));
}

#[test]
fn exclusive_crossfade_policy_suppresses_crossfade() {
    assert_eq!(effective_crossfade_for_exclusive(true, false, 1_500), 0);
    assert_eq!(
        effective_crossfade_for_exclusive(false, false, 1_500),
        1_500
    );
    assert_eq!(effective_crossfade_for_exclusive(true, true, 1_500), 1_500);
    assert_eq!(effective_crossfade_for_exclusive(true, true, -10), 0);
}

#[test]
fn next_prebuffer_slot_suppresses_duplicate_pair_only() {
    let key = crate::NextPrebufferKey {
        current_track_id: 1,
        next_track_id: 2,
        generation: 3,
    };
    let replacement = crate::NextPrebufferKey {
        current_track_id: 1,
        next_track_id: 4,
        generation: 3,
    };
    let mut slot = None;

    assert!(claim_next_prebuffer_slot(&mut slot, key));
    assert!(!claim_next_prebuffer_slot(&mut slot, key));
    assert!(claim_next_prebuffer_slot(&mut slot, replacement));
    release_next_prebuffer_slot(&mut slot, key);
    assert_eq!(slot, Some(replacement));
    release_next_prebuffer_slot(&mut slot, replacement);
    assert_eq!(slot, None);
}

#[test]
fn exclusive_sample_rate_follow_skips_prebuffer_on_rate_change() {
    assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        44_100,
        Some(96_000),
        Some(16),
        Some(24),
    ));
    assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        96_000,
        Some(96_000),
        Some(24),
        Some(24),
    ));
    assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        false,
        44_100,
        Some(96_000),
        Some(16),
        Some(24),
    ));
    assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        44_100,
        None,
        Some(16),
        Some(16),
    ));
    assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
        true,
        true,
        44_100,
        Some(44_100),
        Some(16),
        Some(24),
    ));
}

#[test]
fn shared_sample_rate_follow_skips_prebuffer_on_rate_change() {
    assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
        false,
        true,
        44_100,
        Some(96_000),
        Some(16),
        Some(24),
    ));
}

#[tokio::test]
async fn queue_append_library_track_returns_updated_queue() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));

    let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/queue/append")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"kind":"library","track_id":1,"artist":"Seed Artist","title":"First Track"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["queue"].as_array().unwrap().len(), 1);
    assert_eq!(body["queue"][0]["track"]["id"], 1);
    assert_eq!(body["queue"][0]["is_pending"], false);

    let source: String = db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT source FROM queue WHERE track_id = 1", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(source, "user_queue");

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_play_next_tidal_inserts_pending_row_after_current_with_hint() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let current_qid: i64 = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![qid],
            )?;
            Ok(qid)
        })
        .unwrap();
    assert!(current_qid > 0);

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/queue/play_next")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"kind":"tidal","tidal_id":777,"artist":"External Artist","title":"External Title"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["queue"].as_array().unwrap().len(), 3);
    assert_eq!(body["queue"][1]["is_pending"], true);
    assert_eq!(body["queue"][1]["track"]["title"], "External Title");

    let pending: (i32, String, String, String, Option<i64>) = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT position, source, pending_artist, pending_title, tidal_id_hint
                     FROM queue WHERE track_id IS NULL",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?)
        })
        .unwrap();
    assert_eq!(
        pending,
        (
            1,
            "user_play_next".into(),
            "External Artist".into(),
            "External Title".into(),
            Some(777)
        )
    );

    let shifted_pos: i32 = db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT position FROM queue WHERE track_id = 2", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(shifted_pos, 2);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_play_next_uses_current_queue_item_for_duplicate_track() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (duplicate_qid, current_qid, tail_qid): (i64, i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let duplicate_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 1, 'user')",
                [],
            )?;
            let current_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 2, 'user')",
                [],
            )?;
            let tail_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![current_qid],
            )?;
            Ok((duplicate_qid, current_qid, tail_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/queue/play_next")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"tidal","tidal_id":778,"artist":"External Artist","title":"Exact Current Next"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let (rows, current_queue_item_id): (Vec<(i64, Option<i64>, Option<String>)>, Option<i64>) = db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, track_id, pending_title FROM queue ORDER BY position ASC")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let current_queue_item_id = conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            Ok((rows, current_queue_item_id))
        })
        .unwrap();

    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], (duplicate_qid, Some(1), None));
    assert_eq!(rows[1], (current_qid, Some(1), None));
    assert_eq!(rows[2].1, None);
    assert_eq!(rows[2].2.as_deref(), Some("Exact Current Next"));
    assert_eq!(rows[3], (tail_qid, Some(2), None));
    assert_eq!(current_queue_item_id, Some(current_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_play_next_repairs_stale_anchor_before_insert() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let stale_qid = 999_999_i64;
    let (repaired_qid, duplicate_qid, tail_qid): (i64, i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let repaired_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 1, 'user')",
                [],
            )?;
            let duplicate_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 2, 'user')",
                [],
            )?;
            let tail_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![stale_qid],
            )?;
            Ok((repaired_qid, duplicate_qid, tail_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/queue/play_next")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"tidal","tidal_id":779,"artist":"External Artist","title":"Repaired Next"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let (rows, current_queue_item_id): (Vec<(i64, Option<i64>, Option<String>)>, Option<i64>) = db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, track_id, pending_title FROM queue ORDER BY position ASC")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let current_queue_item_id = conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            Ok((rows, current_queue_item_id))
        })
        .unwrap();

    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], (repaired_qid, Some(1), None));
    assert_eq!(rows[1].1, None);
    assert_eq!(rows[1].2.as_deref(), Some("Repaired Next"));
    assert_eq!(rows[2], (duplicate_qid, Some(1), None));
    assert_eq!(rows[3], (tail_qid, Some(2), None));
    assert_eq!(current_queue_item_id, Some(repaired_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_play_next_repairs_mismatched_anchor_before_insert() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (repaired_qid, mismatched_qid): (i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let repaired_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            let mismatched_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                rusqlite::params![mismatched_qid],
            )?;
            Ok((repaired_qid, mismatched_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/queue/play_next")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"tidal","tidal_id":780,"artist":"External Artist","title":"Repaired Mismatch Next"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let (rows, current_queue_item_id): (Vec<(i64, Option<i64>, Option<String>)>, Option<i64>) = db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, track_id, pending_title FROM queue ORDER BY position ASC")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let current_queue_item_id = conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            Ok((rows, current_queue_item_id))
        })
        .unwrap();

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (repaired_qid, Some(1), None));
    assert_eq!(rows[1].1, None);
    assert_eq!(rows[1].2.as_deref(), Some("Repaired Mismatch Next"));
    assert_eq!(rows[2], (mismatched_qid, Some(2), None));
    assert_eq!(current_queue_item_id, Some(repaired_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_play_next_many_preserves_requested_order() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
            [],
        )?;
        let qid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
            [],
        )?;
        conn.execute(
            "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
            rusqlite::params![qid],
        )?;
        Ok(())
    })
    .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/queue/play_next_many")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"items":[
                            {"kind":"tidal","tidal_id":101,"artist":"A","title":"First external"},
                            {"kind":"tidal","tidal_id":102,"artist":"B","title":"Second external"}
                        ]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["queue"].as_array().unwrap().len(), 4);
    assert_eq!(body["queue"][1]["track"]["title"], "First external");
    assert_eq!(body["queue"][2]["track"]["title"], "Second external");
    assert_eq!(body["queue"][3]["track"]["id"], 2);

    let rows: Vec<(i32, String, Option<i64>)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT position, pending_title, tidal_id_hint
                     FROM queue
                     WHERE track_id IS NULL
                     ORDER BY position ASC",
            )?;
            Ok(stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (1, "First external".to_string(), Some(101)),
            (2, "Second external".to_string(), Some(102)),
        ]
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_play_next_many_repairs_missing_anchor_and_preserves_order() {
    let (db, db_path) = fresh_migrated_db();
    seed_basic_tracks(&db);
    let (repaired_qid, duplicate_qid, tail_qid): (i64, i64, i64) = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let repaired_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 1, 'user')",
                [],
            )?;
            let duplicate_qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 2, 'user')",
                [],
            )?;
            let tail_qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = NULL, is_playing = 1
                     WHERE id = 1",
                [],
            )?;
            Ok((repaired_qid, duplicate_qid, tail_qid))
        })
        .unwrap();

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/queue/play_next_many")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"items":[
                            {"kind":"tidal","tidal_id":201,"artist":"A","title":"Batch First"},
                            {"kind":"tidal","tidal_id":202,"artist":"B","title":"Batch Second"}
                        ]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let (rows, current_queue_item_id): (Vec<(i64, Option<i64>, Option<String>)>, Option<i64>) = db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, track_id, pending_title FROM queue ORDER BY position ASC")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let current_queue_item_id = conn.query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            Ok((rows, current_queue_item_id))
        })
        .unwrap();

    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0], (repaired_qid, Some(1), None));
    assert_eq!(rows[1].1, None);
    assert_eq!(rows[1].2.as_deref(), Some("Batch First"));
    assert_eq!(rows[2].1, None);
    assert_eq!(rows[2].2.as_deref(), Some("Batch Second"));
    assert_eq!(rows[3], (duplicate_qid, Some(1), None));
    assert_eq!(rows[4], (tail_qid, Some(2), None));
    assert_eq!(current_queue_item_id, Some(repaired_qid));

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn queue_append_external_track_creates_pending_row_without_hint() {
    let (db, db_path) = fresh_migrated_db();
    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/queue/append")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"external","artist":"Aphex Twin","title":"Xtal"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["queue"].as_array().unwrap().len(), 1);
    assert_eq!(body["queue"][0]["is_pending"], true);
    assert_eq!(body["queue"][0]["track"]["artist_name"], "Aphex Twin");
    assert_eq!(body["queue"][0]["track"]["title"], "Xtal");

    let hint: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT tidal_id_hint FROM queue WHERE track_id IS NULL",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(hint, None);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn create_playlist_from_queue_imports_pending_tidal_rows_with_hint() {
    let (db, db_path) = fresh_migrated_db();
    db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, tidal_id_hint
                 ) VALUES (NULL, 0, 'user_queue', 'Queued Artist', 'Queued TIDAL', datetime('now'), 777)",
                [],
            )?;
            Ok(())
        })
        .expect("seed pending queue row");

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playlists/from-queue")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Saved pending TIDAL","include_tidal_only":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["added"], 1);

    let saved: Vec<(i64, String)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.tidal_id, t.title
                     FROM playlist_tracks pt
                     JOIN playlists p ON p.id = pt.playlist_id
                     JOIN tracks t ON t.id = pt.track_id
                     WHERE p.name = 'Saved pending TIDAL'
                     ORDER BY pt.position ASC",
            )?;
            Ok(stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap();
    assert_eq!(saved, vec![(777, "Queued TIDAL".to_string())]);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn create_playlist_from_queue_imports_ephemeral_tidal_overlay() {
    let (db, db_path) = fresh_migrated_db();
    let mut state = fresh_test_state(db.clone());
    let mut current = test_track(-901, "Current TIDAL");
    current.tidal_id = Some(901);
    current.artist_name = Some("Current Artist".to_string());
    current.album_title = Some("Current Album".to_string());
    current.source = "tidal_ephemeral".to_string();
    state.ephemeral_tidal_track = Some(current);
    {
        let mut pending = state.pending_tidal_mix_queue.lock().unwrap();
        pending.push_back(crate::PendingEphemeralTidalTrack {
            tidal_track_id: 902,
            title: "Next TIDAL".to_string(),
            artist_name: Some("Next Artist".to_string()),
            album_title: Some("Next Album".to_string()),
            artwork_url: Some("https://resources.tidal.com/images/cover.jpg".to_string()),
            duration_ms: Some(181_000),
        });
        pending.push_back(crate::PendingEphemeralTidalTrack {
            tidal_track_id: 903,
            title: "Third TIDAL".to_string(),
            artist_name: None,
            album_title: None,
            artwork_url: None,
            duration_ms: None,
        });
    }

    let app = api_routes(Arc::new(tokio::sync::RwLock::new(state)));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/playlists/from-queue")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Saved ephemeral TIDAL","include_tidal_only":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["added"], 3);

    let saved: Vec<(i64, String)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.tidal_id, t.title
                     FROM playlist_tracks pt
                     JOIN playlists p ON p.id = pt.playlist_id
                     JOIN tracks t ON t.id = pt.track_id
                     WHERE p.name = 'Saved ephemeral TIDAL'
                     ORDER BY pt.position ASC",
            )?;
            Ok(stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .unwrap();
    assert_eq!(
        saved,
        vec![
            (901, "Current TIDAL".to_string()),
            (902, "Next TIDAL".to_string()),
            (903, "Third TIDAL".to_string())
        ]
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn promote_pending_row_emit_broadcasts_queue_updated() {
    let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
    let db = Database::open(&db_path).expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| schema::run_migrations(conn))
        .expect("schema migrations");

    // Seed an artist + a real track to be the promotion target, plus a
    // pending queue row pointing at "Pending Artist / Pending Title".
    db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Promoted Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, source, fidelity_score
                 ) VALUES (1, 'Promoted Title', 1, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
                 VALUES (NULL, 0, 'radio_pending', 'Pending Artist', 'Pending Title', datetime('now'))",
                [],
            )?;
            Ok(())
        })
        .expect("seed");

    let queue_item_id: i64 = db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT id FROM queue WHERE track_id IS NULL", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();

    let (event_tx, mut rx) = tokio::sync::broadcast::channel(8);
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 950);
    assert!(promoted, "promotion must succeed for a NULL-track row");

    let evt = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("event arrived in time")
        .expect("event channel open");
    assert!(matches!(evt, AppEvent::QueueUpdated));

    // Confirm DB: the row is no longer pending.
    let resolved_track_id: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT track_id FROM queue WHERE id = ?1",
                rusqlite::params![queue_item_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(resolved_track_id, Some(1));

    // Idempotency: a second promotion attempt is a no-op (track_id already set)
    // and must NOT broadcast a second event.
    let again = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 950);
    assert!(!again);
    let no_more = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
    assert!(
        no_more.is_err(),
        "no second event should fire on idempotent retry"
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn promote_pending_row_emit_marks_external_candidate_resolved() {
    let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
    let db = Database::open(&db_path).expect("db opened");
    db.run_migrations().expect("migrations");
    db.with_conn(|conn| schema::run_migrations(conn))
        .expect("schema migrations");

    db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Resolved Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, tidal_id, source, fidelity_score
                 ) VALUES (1, 'Resolved Title', 1, 4242, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO external_track_candidates (
                    tidal_id, dedupe_key, title, artist_name, expires_at
                 ) VALUES (4242, 'tidal:4242', 'Resolved Title', 'Resolved Artist', '2026-03-01 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, tidal_id_hint
                 ) VALUES (NULL, 0, 'automix-new', 'Resolved Artist', 'Resolved Title', datetime('now'), 4242)",
                [],
            )?;
            Ok(())
        })
        .expect("seed");

    let queue_item_id: i64 = db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT id FROM queue WHERE track_id IS NULL", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();

    let (event_tx, _rx) = tokio::sync::broadcast::channel(8);
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 990);
    assert!(promoted);

    let resolved: Option<i64> = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT resolved_track_id FROM external_track_candidates WHERE tidal_id = 4242",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(resolved, Some(1));

    let _ = std::fs::remove_file(db_path);
}

#[test]
fn automix_discover_new_fallback_waits_when_sidecar_new_rows_fill_slots() {
    let current = test_track(1, "Current");
    let mut snapshot = crate::playback::player::PlaybackSnapshot {
        state: crate::db::models::PlaybackState {
            current_track: Some(current.clone()),
            current_queue_item_id: Some(10),
            position_ms: 0,
            is_playing: true,
            volume: 1.0,
            shuffle_mode: "off".to_string(),
            repeat_mode: "off".to_string(),
            automix_enabled: true,
            crossfade_ms: 0,
            automix_discover_new: true,
            automix_use_learning: true,
            automix_allow_external: true,
            buffered_ms: 0,
            buffered_start_ms: 0,
        },
        queue: vec![
            test_queue_item(10, current, 0, "manual"),
            test_queue_item(11, test_track(2, "Sidecar A"), 1, "automix-new"),
            test_queue_item(12, test_track(3, "Sidecar B"), 2, "automix-new"),
        ],
    };

    assert!(automix_discover_new_fallback_seed(&snapshot).is_none());

    snapshot.queue.pop();

    assert!(automix_discover_new_fallback_seed(&snapshot).is_some());
}

#[test]
fn automix_discover_new_fallback_ignores_mismatched_queue_anchor() {
    let current = test_track(1, "Current");
    let snapshot = crate::playback::player::PlaybackSnapshot {
        state: crate::db::models::PlaybackState {
            current_track: Some(current.clone()),
            current_queue_item_id: Some(13),
            position_ms: 0,
            is_playing: true,
            volume: 1.0,
            shuffle_mode: "off".to_string(),
            repeat_mode: "off".to_string(),
            automix_enabled: true,
            crossfade_ms: 0,
            automix_discover_new: true,
            automix_use_learning: true,
            automix_allow_external: true,
            buffered_ms: 0,
            buffered_start_ms: 0,
        },
        queue: vec![
            test_queue_item(10, current, 0, "manual"),
            test_queue_item(11, test_track(2, "Sidecar A"), 1, "automix-new"),
            test_queue_item(12, test_track(3, "Sidecar B"), 2, "automix-new"),
            test_queue_item(13, test_track(4, "Stale Anchor"), 3, "manual"),
        ],
    };

    assert!(automix_discover_new_fallback_seed(&snapshot).is_none());
}

#[test]
fn automix_discover_new_fallback_stays_off_when_disabled() {
    let current = test_track(1, "Current");
    let snapshot = crate::playback::player::PlaybackSnapshot {
        state: crate::db::models::PlaybackState {
            current_track: Some(current.clone()),
            current_queue_item_id: Some(10),
            position_ms: 0,
            is_playing: true,
            volume: 1.0,
            shuffle_mode: "off".to_string(),
            repeat_mode: "off".to_string(),
            automix_enabled: true,
            crossfade_ms: 0,
            automix_discover_new: false,
            automix_use_learning: true,
            automix_allow_external: true,
            buffered_ms: 0,
            buffered_start_ms: 0,
        },
        queue: vec![test_queue_item(10, current, 0, "manual")],
    };

    assert!(automix_discover_new_fallback_seed(&snapshot).is_none());
}

#[tokio::test]
async fn server_info_returns_defaults() {
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/server/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["host_mode"], false);
    assert!(body["bind_address"].as_str().unwrap().contains("3334"));
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn put_host_mode_persists() {
    let app = build_test_app().await;

    // Enable host mode
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/server/host_mode")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"host_mode":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Reading info should now reflect host_mode = true
    let resp2 = app
        .oneshot(
            Request::builder()
                .uri("/api/server/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["host_mode"], true);
    assert!(
        body["bind_address"]
            .as_str()
            .unwrap()
            .starts_with("0.0.0.0")
    );
}

/// Reproducer for the Phase 2b hotfix: `/api/radio/song` must
/// reject ephemeral Tidal track ids (negative or zero) with a
/// 400 + actionable error body, not a 500 with no body.
///
/// Pre-fix behaviour: handler accepted any i64, passed it
/// through to `orchestrate_song` which logged
/// `WARN "radio_song failed: seed track not found: -85771852"`
/// and returned 500. Frontend kept the prior queue, producing
/// the "kitchen-sink" symptom the bug report described.
#[tokio::test]
async fn radio_song_rejects_negative_seed_id_with_400() {
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/radio/song")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"seed_track_id": -85771852}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("positive library id"),
        "unexpected error body: {body}"
    );
    assert!(
        body["hint"]
            .as_str()
            .unwrap_or("")
            .contains("seed_tidal_id"),
        "expected hint to mention seed_tidal_id: {body}"
    );
}

/// Boundary: `seed_track_id == 0` is also rejected. Zero is
/// neither a valid library id (rowids start at 1) nor an
/// ephemeral negative - it usually indicates a serialisation
/// default leaking through, which still shouldn't reach the
/// orchestrator.
#[tokio::test]
async fn radio_song_rejects_zero_seed_id_with_400() {
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/radio/song")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"seed_track_id": 0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// Characterization tests for TIDAL-handler failure shapes. These differ on
// purpose: the album endpoint is "best-effort" (TIDAL is enrichment) while
// tidal_search treats a disconnected session as a user-visible error.
// A single shared error helper would erase this distinction - so these tests
// exist to flag any refactor that does.

#[tokio::test]
async fn get_album_tracks_returns_local_tracks_when_tidal_session_absent() {
    let (db, db_path) = fresh_migrated_db();
    // The album MUST have a tidal_id, otherwise the handler returns at
    // routes.rs:977 with album_tidal_id: null - which is a different code
    // path. To exercise the "no TIDAL session" branch (routes.rs:995) we
    // need a TIDAL-mapped album with no session.
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Local Artist')",
            [],
        )?;
        conn.execute(
            "INSERT INTO albums (id, tidal_id, title, artist_id, source)
                 VALUES (5, 8888, 'Local Album', 1, 'tidal')",
            [],
        )?;
        conn.execute(
            "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, source, fidelity_score
                 ) VALUES (10, 'Local Track', 1, 5, 180000, 'tidal_stream', 0)",
            [],
        )?;
        Ok::<_, anyhow::Error>(())
    })
    .expect("seed");

    // fresh_test_state has tidal_tokens: None -> the session is "disconnected".
    let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
        db.clone(),
    ))));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/albums/5/tracks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Best-effort path: the album still resolves, local tracks are returned,
    // tidal_tracks is empty, album_tidal_id is preserved. Do NOT change this
    // to 400 / 502 / hide the album_tidal_id in a refactor.
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let tracks = body["tracks"].as_array().expect("tracks array");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["title"], "Local Track");
    let tidal_tracks = body["tidal_tracks"].as_array().expect("tidal_tracks array");
    assert!(
        tidal_tracks.is_empty(),
        "tidal_tracks must be [] when disconnected"
    );
    assert_eq!(
        body["album_tidal_id"], 8888,
        "album_tidal_id must survive even when session is absent"
    );

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
async fn tidal_search_returns_400_when_tidal_session_absent() {
    // Opposite of the album endpoint: tidal_search has no library fallback,
    // so a missing session must surface as a user-visible 400, not silently
    // return an empty result set.
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tidal/search?q=foo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("TIDAL not connected"),
        "expected 'TIDAL not connected' error, got: {body}"
    );
}

#[tokio::test]
async fn tidal_search_blank_query_returns_empty_payload_without_session() {
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tidal/search?q=%20%20%20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["tracks"].as_array().expect("tracks").len(), 0);
    assert_eq!(body["albums"].as_array().expect("albums").len(), 0);
    assert_eq!(body["artists"].as_array().expect("artists").len(), 0);
    assert_eq!(body["videos"].as_array().expect("videos").len(), 0);
}

#[tokio::test]
async fn tidal_artist_profile_rejects_non_positive_ids_before_session_lookup() {
    let app = build_test_app().await;

    for uri in ["/api/tidal/artists/0", "/api/tidal/artists/-7"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "uri: {uri}");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("positive TIDAL artist id"),
            "expected invalid artist id error for {uri}, got: {body}"
        );
    }
}

#[tokio::test]
async fn tidal_artist_profile_positive_id_still_requires_session() {
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tidal/artists/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("TIDAL not connected"),
        "expected existing disconnected-session behavior, got: {body}"
    );
}

#[tokio::test]
async fn tidal_album_routes_reject_non_positive_ids_before_session_lookup() {
    let app = build_test_app().await;

    for (method, uri) in [
        ("GET", "/api/tidal/albums/0/tracks"),
        ("GET", "/api/tidal/albums/-7/tracks"),
        ("POST", "/api/tidal/albums/0/import"),
        ("POST", "/api/tidal/albums/-7/import"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{method} {uri}");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("positive TIDAL album id"),
            "expected invalid album id error for {method} {uri}, got: {body}"
        );
    }
}

#[tokio::test]
async fn tidal_album_routes_positive_ids_still_require_session() {
    let app = build_test_app().await;

    for (method, uri) in [
        ("GET", "/api/tidal/albums/1/tracks"),
        ("POST", "/api/tidal/albums/1/import"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{method} {uri}");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("TIDAL not connected"),
            "expected existing disconnected-session behavior for {method} {uri}, got: {body}"
        );
    }
}

#[tokio::test]
async fn tidal_video_playback_rejects_non_positive_ids_before_session_lookup() {
    let app = build_test_app().await;

    for uri in [
        "/api/tidal/videos/0/playback",
        "/api/tidal/videos/-7/playback",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "uri: {uri}");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("positive TIDAL video id"),
            "expected invalid video id error for {uri}, got: {body}"
        );
    }
}

#[tokio::test]
async fn tidal_video_playback_positive_id_still_requires_session() {
    let app = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tidal/videos/1/playback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("TIDAL not connected"),
        "expected existing disconnected-session behavior, got: {body}"
    );
}

// Note: an integration test for the recover_tidal_session path on a 401 upstream
// response is deferred - it requires intercepting the reqwest::Client, which
// requires wiremock or a trait-based http client. Until that infra lands, the
// refresh-on-auth path in tidal_search / tidal_video_playback / tidal_playlist_*
// remains uncovered.

// Library-track filter predicate (favorite_only / liked_only) is already
// covered by characterization-grade tests in db/queries.rs:
//   - liked_only_excludes_album_favorited_tracks (queries.rs:7931)
//   - favorite_only_preserves_legacy_union_behavior (queries.rs:7949)
//   - liked_only_takes_precedence_over_favorite_only (queries.rs:8036)
// Do not add duplicates here.

/// Route-registration smoke test. Probes every `/api/*` route registered in
/// `api_routes` with a deliberately-wrong HTTP method and asserts the
/// response is NOT 404.
///
/// Why wrong-method: axum returns 405 METHOD_NOT_ALLOWED when a path is
/// registered but the method doesn't match, and 404 NOT_FOUND when the path
/// isn't registered at all. Probing with the wrong method means routing
/// resolves before any extractor runs - so path params, request bodies, and
/// auth never enter the picture. A 404 here means the route is genuinely
/// missing, which is exactly the failure mode a careless handler extraction
/// introduces: move a handler to a submodule, forget to re-register it.
///
/// This is a structural guard, NOT a behavior contract. It deliberately
/// does not assert on bodies or success codes. Per-route behavior belongs
/// in dedicated tests. When you add a route, add it here too.
#[tokio::test]
async fn all_api_routes_are_registered() {
    // (real_method, path). Path params use concrete sentinels; the value is
    // irrelevant because the wrong-method probe short-circuits at routing.
    let routes: &[(&str, &str)] = &[
        ("GET", "/api/tracks"),
        ("GET", "/api/tracks/count"),
        ("GET", "/api/albums"),
        ("GET", "/api/albums/1/tracks"),
        ("GET", "/api/albums/1/spotify-stats"),
        ("GET", "/api/artists"),
        ("GET", "/api/artists/1"),
        ("GET", "/api/artists/1/tracks"),
        ("GET", "/api/artists/1/discography"),
        ("GET", "/api/artists/1/spotify-stats"),
        ("GET", "/api/tidal/albums/1/tracks"),
        ("POST", "/api/tidal/albums/1/import"),
        ("POST", "/api/tidal/tracks/import"),
        ("GET", "/api/genres"),
        ("GET", "/api/genres/snapshot"),
        ("GET", "/api/genres/heat"),
        ("GET", "/api/genres/co-occurrence"),
        ("GET", "/api/genres/cohorts"),
        ("GET", "/api/genres/evolution"),
        ("GET", "/api/genres/audio-metrics"),
        ("GET", "/api/genres/1/tracks"),
        ("GET", "/api/playlists"),
        ("GET", "/api/playlists/1/tracks"),
        ("PATCH", "/api/playlists/1/favorite"),
        ("GET", "/api/playlists/1/cover-sample"),
        ("POST", "/api/smart/playlists"),
        ("PUT", "/api/smart/playlists/1"),
        ("GET", "/api/smart/playlists/1/evaluate"),
        ("POST", "/api/smart/playlists/preview"),
        ("GET", "/api/artists/search"),
        ("GET", "/api/analytics/overview"),
        ("GET", "/api/analytics/dashboard"),
        ("GET", "/api/analytics/signals"),
        ("GET", "/api/analytics/listens/recent"),
        ("POST", "/api/discovery/preview"),
        ("POST", "/api/discovery/new"),
        ("POST", "/api/discovery/save"),
        ("POST", "/api/discovery/play"),
        ("POST", "/api/discovery/connections"),
        ("GET", "/api/discovery/status"),
        ("POST", "/api/discovery/train"),
        ("GET", "/api/discovery/train/status"),
        ("POST", "/api/discovery/train/stop"),
        ("GET", "/api/discovery/train/intensity"),
        ("GET", "/api/discovery/train/engine"),
        ("GET", "/api/discovery/train/safety"),
        ("GET", "/api/discovery/train/safety-profile"),
        ("POST", "/api/discovery/feedback"),
        ("GET", "/api/discovery/presets"),
        ("POST", "/api/discovery/radio"),
        ("POST", "/api/discovery/radio/compute"),
        ("POST", "/api/discovery/space"),
        ("POST", "/api/discovery/blend/space"),
        ("POST", "/api/discovery/blend/add"),
        ("POST", "/api/discovery/blend/play"),
        ("POST", "/api/discovery/blend/radio"),
        ("GET", "/api/resolve/tidal/track"),
        ("POST", "/api/resolve/tidal/bulk"),
        ("GET", "/api/resolve/tidal/status"),
        ("GET", "/api/discovery/sportify/search"),
        ("GET", "/api/discovery/sportify/track/x"),
        ("GET", "/api/discovery/sportify/album/x"),
        ("GET", "/api/discovery/sportify/playlist/x/meta"),
        ("GET", "/api/discovery/sportify/playlist/x"),
        ("GET", "/api/discovery/sportify/artist/x"),
        ("GET", "/api/discovery/sportify/artist/x/top-tracks"),
        ("GET", "/api/discovery/sportify/artist/x/related"),
        ("GET", "/api/discovery/sportify/album/x/related"),
        ("GET", "/api/discovery/sportify/track/x/related"),
        ("POST", "/api/spotify-playlist/save"),
        ("POST", "/api/spotify-track/save"),
        ("POST", "/api/spotify-album/save"),
        ("POST", "/api/radio/song"),
        ("POST", "/api/radio/album"),
        ("POST", "/api/radio/artist"),
        ("POST", "/api/radio/start"),
        ("GET", "/api/discovery/space/meta"),
        ("GET", "/api/discovery/artists"),
        ("POST", "/api/library/batch/add-to-playlist"),
        ("POST", "/api/library/batch/delete"),
        ("POST", "/api/library/batch/set-genre"),
        ("POST", "/api/library/enrich/musicbrainz"),
        ("GET", "/api/library/enrich/musicbrainz/status"),
        ("GET", "/api/library/enrich/musicbrainz/portable"),
        ("POST", "/api/library/enrich/musicbrainz/portable/export"),
        ("POST", "/api/library/enrich/musicbrainz/portable/import"),
        ("POST", "/api/library/tracks/favorite"),
        ("POST", "/api/library/duplicates/scan"),
        ("GET", "/api/library/duplicates"),
        ("POST", "/api/library/duplicates/1/resolve"),
        ("POST", "/api/library/duplicates/1/dismiss"),
        ("GET", "/api/playback/state"),
        ("GET", "/api/playback/runtime"),
        ("POST", "/api/playback/play"),
        ("POST", "/api/playback/pause"),
        ("POST", "/api/playback/resume"),
        ("POST", "/api/playback/previous"),
        ("POST", "/api/playback/next"),
        ("POST", "/api/playback/position"),
        ("POST", "/api/playback/volume"),
        ("POST", "/api/playback/shuffle"),
        ("POST", "/api/playback/repeat"),
        ("POST", "/api/playback/automix"),
        ("GET", "/api/playback/queue"),
        ("POST", "/api/playback/queue/add"),
        ("POST", "/api/playback/queue/remove"),
        ("POST", "/api/playback/queue/move"),
        ("POST", "/api/playback/queue/clear"),
        ("POST", "/api/queue/play_next"),
        ("POST", "/api/queue/play_next_many"),
        ("POST", "/api/queue/append"),
        ("POST", "/api/queue/append_many"),
        ("POST", "/api/playlists/from-queue"),
        ("GET", "/api/audio/devices"),
        ("GET", "/api/audio/settings"),
        ("POST", "/api/audio/exclusive/retry"),
        ("GET", "/api/search"),
        ("POST", "/api/search/audio"),
        ("GET", "/api/search/vibe"),
        ("GET", "/api/search/underrated"),
        ("POST", "/api/tidal/login"),
        ("POST", "/api/tidal/login/complete"),
        ("POST", "/api/tidal/login/poll"),
        ("POST", "/api/tidal/sync"),
        ("POST", "/api/tidal/sync/cancel"),
        ("GET", "/api/tidal/status"),
        ("GET", "/api/tidal/search"),
        ("GET", "/api/tidal/videos/search"),
        ("GET", "/api/tidal/videos/1/playback"),
        ("GET", "/api/tidal/video-mixes/1/items"),
        ("GET", "/api/tidal/playlists/search"),
        ("GET", "/api/tidal/playlists/x/tracks"),
        ("POST", "/api/tidal/play"),
        ("GET", "/api/tidal/artists/1"),
        ("POST", "/api/tidal/logout"),
        ("POST", "/api/spotify/config"),
        ("GET", "/api/spotify/status"),
        ("POST", "/api/library/enrich/spotify"),
        ("GET", "/api/library/enrich/spotify/status"),
        ("POST", "/api/library/enrich/spotify/reset"),
        ("POST", "/api/library/tidal-stream/purge"),
        ("POST", "/api/lastfm/config"),
        ("GET", "/api/lastfm/config"),
        ("DELETE", "/api/lastfm/config"),
        ("GET", "/api/lastfm/status"),
        ("POST", "/api/listenbrainz/config"),
        ("GET", "/api/listenbrainz/config"),
        ("DELETE", "/api/listenbrainz/config"),
        ("GET", "/api/listenbrainz/status"),
        ("POST", "/api/lastfm/auth/start"),
        ("POST", "/api/lastfm/auth/complete"),
        ("POST", "/api/lastfm/auth/disconnect"),
        ("POST", "/api/library/enrich/lastfm"),
        ("POST", "/api/library/enrich/lastfm/stop"),
        ("GET", "/api/library/enrich/lastfm/status"),
        ("POST", "/api/library/enrich/lastfm/reset"),
        ("POST", "/api/scrobbling/backfill"),
        ("POST", "/api/library/analyze/audio-features"),
        ("POST", "/api/library/analyze/stop"),
        ("GET", "/api/library/analyze/status"),
        ("GET", "/api/library/analyze/passive"),
        ("GET", "/api/tracks/1/audio-features"),
        ("POST", "/api/tracks/1/bpm-multiplier"),
        ("GET", "/api/library/audio-features/stats"),
        ("GET", "/api/library/audio-features/quality"),
        ("GET", "/api/library/analytics"),
        ("GET", "/api/library/analyze/reanalyze-stale"),
        ("POST", "/api/library/analyze/reset"),
        ("GET", "/api/sync/info"),
        ("POST", "/api/sync/auto"),
        ("GET", "/api/status"),
        ("GET", "/api/home/releases"),
        ("GET", "/api/home/picks"),
        ("GET", "/api/home/articles"),
        ("GET", "/api/home/news"),
        ("GET", "/api/home/recommendations"),
        ("GET", "/api/tidal/mixes"),
        ("GET", "/api/tidal/mixes/1/tracks"),
        ("POST", "/api/tidal/play-mix"),
        ("GET", "/api/tidal/radio-stations"),
        ("GET", "/api/tidal/home-modules"),
        ("GET", "/api/tidal/discover-modules/1/items"),
        ("GET", "/api/tidal/page/explore"),
        ("GET", "/api/tidal/page/mood/1"),
        ("GET", "/api/tidal/moods"),
        ("GET", "/api/tidal/mood-page/mood_party"),
        ("GET", "/api/charts"),
        ("GET", "/api/charts/snapshots"),
        ("POST", "/api/charts/spotify/daily/import"),
        ("GET", "/api/charts/matrix"),
        ("POST", "/api/charts/matrix/refresh"),
        ("GET", "/api/charts/lastfm/genres"),
        ("GET", "/api/charts/lastfm/countries"),
        ("GET", "/api/server/token"),
        ("POST", "/api/server/token/regenerate"),
        ("GET", "/api/server/info"),
        ("PUT", "/api/server/host_mode"),
    ];

    let app = build_test_app().await;
    let mut missing = Vec::new();
    for (method, path) in routes {
        // Probe with a method the route does NOT use, so routing resolves
        // to 405 (registered) or 404 (not registered) before extractors run.
        let probe = if *method == "GET" { "POST" } else { "GET" };
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(probe)
                    .uri(*path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if resp.status() == StatusCode::NOT_FOUND {
            missing.push(format!("{method} {path}"));
        }
    }

    assert!(
        missing.is_empty(),
        "routes returned 404 to a wrong-method probe (not registered):\n  {}",
        missing.join("\n  ")
    );
}
