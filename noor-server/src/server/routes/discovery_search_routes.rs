//! Discovery search endpoints: provider selection, Last.fm query augmentation, candidate enrichment, and playing a discovery result.

use super::*;

#[derive(Debug, Deserialize)]
pub struct DiscoveryExternalResultRequest {
    pub(super) provider: String,
    pub(super) provider_track_id: String,
    pub(super) title: String,
    pub(super) artist_name: Option<String>,
    pub(super) album_title: Option<String>,
    pub(super) artwork_url: Option<String>,
    pub(super) duration_ms: Option<i64>,
    pub(super) normalized_genres: Option<Vec<String>>,
}

pub(super) async fn play_discovery_track(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryExternalResultRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let previous_track_id = current_playback_track_id(&state).await;
    let provider = normalize_external_provider(&payload.provider).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "That discovery provider is not supported yet.",
            })),
        )
    })?;

    if provider != "tidal" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "Inline playback is only wired up for TIDAL discovery right now.",
            })),
        ));
    }

    let tidal_id = parse_provider_track_id(&payload.provider_track_id)?;
    let candidate = crate::server::radio_pipeline::OrderedQueueCandidate {
        track_id: None,
        tidal_id: Some(tidal_id),
        artist: payload
            .artist_name
            .clone()
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        title: payload.title.clone(),
        album_title: payload.album_title.clone(),
        artwork_url: payload.artwork_url.clone(),
        duration_ms: payload.duration_ms,
        artist_tidal_id: None,
        album_tidal_id: None,
        reason: Some("discovery".to_string()),
    };
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let build = db
        .with_conn(|conn| {
            Ok(
                crate::server::radio_pipeline::replace_queue_with_ordered_candidates(
                    conn,
                    &[candidate],
                )?,
            )
        })
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to queue discovery track" })),
            )
        })?;
    spawn_pending_resolvers_for_queue_items(
        &state,
        &db,
        build.pending_item_ids,
        "play_discovery_track",
    )
    .await;
    let snapshot = start_first_radio_queue_item(&state).await?;
    record_transition_if_changed(&state, previous_track_id, &snapshot, "discovery", false).await;

    let state_guard = state.read().await;
    if let Some(track) = snapshot.state.current_track.as_ref() {
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id: track.id });
    }
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    drop(state_guard);
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryArtistsQuery {
    pub(super) limit: Option<i64>,
}

pub(super) async fn get_discovery_artists(
    State(state): State<SharedState>,
    Query(query): Query<DiscoveryArtistsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);

    let artists = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT a.id, a.name, COUNT(th.track_id) as listen_count
             FROM artists a
             LEFT JOIN tracks t ON t.artist_id = a.id
             LEFT JOIN track_history th ON th.track_id = t.id
             GROUP BY a.id, a.name
             ORDER BY listen_count DESC
             LIMIT ?",
            )?;
            let rows = stmt.query_map([limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .unwrap_or_default();

    let max_count = artists.iter().map(|(_, _, c)| *c).max().unwrap_or(1) as f64;
    let artist_count = artists.len();

    let artist_nodes: Vec<Value> = artists
        .into_iter()
        .enumerate()
        .map(|(i, (id, name, count))| {
            let angle = (i as f64 / artist_count.max(1) as f64) * std::f64::consts::PI * 2.0;
            let radius = 80.0 + (i as f64 * 43.0).sin() * 120.0;
            let affinity = if max_count > 0.0 {
                count as f64 / max_count
            } else {
                0.0
            };
            json!({
                "artist_id": id,
                "name": name,
                "top_genre": null,
                "affinity": affinity,
                "x": angle.cos() * radius,
                "y": angle.sin() * radius,
                "vx": 0.0,
                "vy": 0.0,
                "size": 8.0 + affinity * 32.0,
            })
        })
        .collect();

    Ok(Json(json!({
        "artists": artist_nodes,
    })))
}

pub(in crate::server) fn normalize_discovery_mode(mode: Option<&str>) -> String {
    match mode.unwrap_or("mood").trim() {
        "reference" => "reference".to_string(),
        "dj" => "dj".to_string(),
        "word-cloud" => "word-cloud".to_string(),
        _ => "mood".to_string(),
    }
}

pub(in crate::server) fn normalize_discovery_services(
    services: Option<Vec<String>>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    services
        .unwrap_or_else(|| vec!["tidal".to_string()])
        .into_iter()
        .map(|service| service.trim().to_ascii_lowercase())
        .filter(|service| !service.is_empty())
        .filter(|service| {
            matches!(
                service.as_str(),
                "tidal" | "ytmusic" | "soundcloud" | "bandcamp"
            )
        })
        .filter(|service| seen.insert(service.clone()))
        .collect()
}

pub(in crate::server) fn normalize_external_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "tidal" => Some("tidal"),
        "soundcloud" => Some("soundcloud"),
        "bandcamp" => Some("bandcamp"),
        "ytmusic" => Some("ytmusic"),
        _ => None,
    }
}

pub(in crate::server) fn internal_discovery_error(
    error: anyhow::Error,
) -> (StatusCode, Json<Value>) {
    error!(
        target: "noor.discovery.external",
        event = "internal_discovery_error",
        error = %error,
        "external discovery failed"
    );
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "status": "discovery_internal_error",
            "message": "NOOR could not assemble the discovery context.",
        })),
    )
}

pub(in crate::server) fn discovery_upstream_error(
    error: anyhow::Error,
) -> (StatusCode, Json<Value>) {
    error!(
        target: "noor.discovery.external",
        event = "upstream_discovery_error",
        error = %error,
        "upstream discovery provider failed"
    );
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "status": "discovery_upstream_error",
            "message": "The external discovery provider failed to respond cleanly.",
            "details": error.to_string(),
        })),
    )
}

pub(in crate::server) async fn tidal_discovery_provider(
    state: &SharedState,
) -> Result<TidalDiscoveryProvider, (StatusCode, Json<Value>)> {
    let state_guard = state.read().await;
    let tokens = state_guard.tidal.tokens().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "not_connected",
                "message": "Connect TIDAL in Settings before searching for new music.",
            })),
        )
    })?;

    Ok(TidalDiscoveryProvider::new(
        state_guard.tidal.clone(),
        tokens.user_id,
        tokens.country_code,
        state_guard.db.clone(),
    ))
}

pub(in crate::server) async fn load_external_discovery_context(
    state: &SharedState,
) -> anyhow::Result<external_discovery_engine::ExternalDiscoveryContext> {
    let state_guard = state.read().await;
    state_guard.db.with_conn(|conn| {
        Ok(external_discovery_engine::ExternalDiscoveryContext {
            overview: queries::get_analytics_overview(conn)?,
            behavior: queries::get_behavior_metrics(conn)?,
            recent_listens: queries::get_recent_listens(conn, 12)?,
            top_artists: queries::get_top_artists_by_history(conn, 6)?,
            top_genres: queries::get_top_genres_by_history(conn, 6)?,
        })
    })
}

pub(in crate::server) async fn existing_candidate_tidal_ids(
    state: &SharedState,
    candidates: &[crate::services::discovery::DiscoveryCandidateTrack],
) -> anyhow::Result<std::collections::HashSet<i64>> {
    let tidal_ids = candidates
        .iter()
        .filter_map(|candidate| candidate.tidal_track_id)
        .collect::<Vec<_>>();
    let state_guard = state.read().await;
    state_guard
        .db
        .with_conn(|conn| queries::get_existing_tidal_track_ids(conn, &tidal_ids))
}

pub(in crate::server) fn discovery_provider_capabilities()
-> Vec<crate::db::models::DiscoveryProviderCapability> {
    vec![
        crate::db::models::DiscoveryProviderCapability {
            provider: "tidal".to_string(),
            can_save: true,
            can_play_inline: true,
            can_fetch_connections: true,
            can_map_genres: true,
        },
        crate::db::models::DiscoveryProviderCapability {
            provider: "soundcloud".to_string(),
            can_save: false,
            can_play_inline: false,
            can_fetch_connections: false,
            can_map_genres: false,
        },
        crate::db::models::DiscoveryProviderCapability {
            provider: "bandcamp".to_string(),
            can_save: false,
            can_play_inline: false,
            can_fetch_connections: false,
            can_map_genres: false,
        },
        crate::db::models::DiscoveryProviderCapability {
            provider: "ytmusic".to_string(),
            can_save: false,
            can_play_inline: false,
            can_fetch_connections: false,
            can_map_genres: false,
        },
    ]
}

pub(in crate::server) async fn augment_connection_queries_with_lastfm(
    state: &SharedState,
    seed: &DiscoveryCandidateSeed,
    base_queries: Vec<String>,
) -> Vec<String> {
    let (http_client, db) = {
        let state_guard = state.read().await;
        (state_guard.http_client.clone(), state_guard.db.clone())
    };
    let Some(lastfm) = LastFmClient::load(http_client, &db) else {
        return base_queries;
    };

    match lastfm.connection_queries(seed).await {
        Ok(extra_queries) => merge_discovery_queries(base_queries, extra_queries, 12),
        Err(error) => {
            warn!(
                target: "noor.discovery.lastfm",
                event = "connection_query_augmentation_failed",
                error = %error,
                seed_title = %seed.title,
                "failed to augment connection queries with Last.fm"
            );
            base_queries
        }
    }
}

pub(in crate::server) async fn augment_search_queries_with_lastfm(
    state: &SharedState,
    request: &external_discovery_engine::ExternalDiscoveryRequest,
    context: &external_discovery_engine::ExternalDiscoveryContext,
    base_queries: Vec<String>,
) -> Vec<String> {
    let (http_client, db) = {
        let state_guard = state.read().await;
        (state_guard.http_client.clone(), state_guard.db.clone())
    };
    let Some(lastfm) = LastFmClient::load(http_client, &db) else {
        return base_queries;
    };

    let prompt_genres = external_discovery_engine::inferred_prompt_genres(&request.prompt);
    let seed_artists = context
        .top_artists
        .iter()
        .take(2)
        .map(|artist| artist.artist_name.clone())
        .collect::<Vec<_>>();

    match lastfm
        .search_queries(&prompt_genres, &seed_artists, &request.mode)
        .await
    {
        Ok(extra_queries) => merge_discovery_queries(base_queries, extra_queries, 12),
        Err(error) => {
            warn!(
                target: "noor.discovery.lastfm",
                event = "search_query_augmentation_failed",
                error = %error,
                prompt = %request.prompt,
                "failed to augment discovery search queries with Last.fm"
            );
            base_queries
        }
    }
}

pub(in crate::server) async fn enrich_candidates_with_metadata(
    state: &SharedState,
    mut candidates: Vec<crate::services::discovery::DiscoveryCandidateTrack>,
) -> Vec<crate::services::discovery::DiscoveryCandidateTrack> {
    let (http_client, db) = {
        let state_guard = state.read().await;
        (state_guard.http_client.clone(), state_guard.db.clone())
    };
    let lastfm = LastFmClient::load(http_client.clone(), &db);
    let discogs = DiscogsClient::new(http_client);

    for candidate in candidates.iter_mut().take(16) {
        if let (Some(lastfm), Some(artist_name)) =
            (lastfm.as_ref(), candidate.artist_name.as_deref())
        {
            match lastfm.track_signals(artist_name, &candidate.title).await {
                Ok(signals) => {
                    candidate.lastfm_tags = signals.tags;
                }
                Err(error) => {
                    warn!(
                        target: "noor.discovery.lastfm",
                        event = "track_signal_enrichment_failed",
                        error = %error,
                        artist = %artist_name,
                        title = %candidate.title,
                        "failed to enrich discovery candidate with Last.fm tags"
                    );
                }
            }
        }

        match discogs
            .enrich_track(
                candidate.artist_name.as_deref(),
                &candidate.title,
                candidate.album_title.as_deref(),
            )
            .await
        {
            Ok(Some(enrichment)) => {
                candidate.discogs_genres = enrichment.genres;
                candidate.discogs_styles = enrichment.styles;
                candidate.discogs_label = enrichment.label;
                candidate.discogs_year = enrichment.year;
                candidate.discogs_confidence = Some(enrichment.confidence);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    target: "noor.discovery.discogs",
                    event = "track_enrichment_failed",
                    error = %error,
                    title = %candidate.title,
                    "failed to enrich discovery candidate with Discogs metadata"
                );
            }
        }
    }

    candidates
}

pub(super) fn merge_discovery_queries(
    base_queries: Vec<String>,
    extra_queries: Vec<String>,
    limit: usize,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    base_queries
        .into_iter()
        .chain(extra_queries)
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
        .filter(|query| seen.insert(query.to_ascii_lowercase()))
        .take(limit)
        .collect()
}

pub(super) fn parse_provider_track_id(
    provider_track_id: &str,
) -> Result<i64, (StatusCode, Json<Value>)> {
    provider_track_id.parse::<i64>().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "invalid_provider_track_id",
                "message": "Provider track id was not valid for playback.",
            })),
        )
    })
}

pub(in crate::server) fn discovery_request_to_trail_item(
    payload: &DiscoveryExternalResultRequest,
) -> crate::db::models::DiscoveryConnectionTrailItem {
    crate::db::models::DiscoveryConnectionTrailItem {
        provider: payload.provider.clone(),
        provider_track_id: payload.provider_track_id.clone(),
        title: payload.title.clone(),
        artist_name: payload.artist_name.clone(),
        album_title: payload.album_title.clone(),
        artwork_url: payload.artwork_url.clone(),
        normalized_genres: payload.normalized_genres.clone().unwrap_or_default(),
        connection_reason: if payload
            .normalized_genres
            .as_ref()
            .map(|genres| !genres.is_empty())
            .unwrap_or(false)
        {
            format!(
                "genre cues like {}",
                payload
                    .normalized_genres
                    .clone()
                    .unwrap_or_default()
                    .join(", ")
            )
        } else if let Some(artist) = payload.artist_name.as_deref() {
            format!("adjacent energy around {artist}")
        } else {
            "adjacent energy".to_string()
        },
    }
}
