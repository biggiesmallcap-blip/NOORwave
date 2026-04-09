use crate::db::models::{Playlist, Track};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// Additional track-scoped data useful when building analytics views.
#[derive(Debug, Clone, Default)]
pub struct AnalyticsContext {
    genres_by_track: HashMap<i64, HashSet<String>>,
}

impl AnalyticsContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_track_genres<I, S>(mut self, track_id: i64, genres: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let entry = self.genres_by_track.entry(track_id).or_default();
        for genre in genres {
            entry.insert(genre.into());
        }
        self
    }

    pub fn genres_for_track(&self, track_id: i64) -> Option<&HashSet<String>> {
        self.genres_by_track.get(&track_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedArtist {
    pub name: String,
    pub track_count: usize,
    pub play_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedTrack {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub play_count: i32,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub date_added: Option<String>,
    pub last_played_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityShare {
    pub label: String,
    pub count: usize,
    pub share: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenreShare {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryAnalytics {
    pub track_count: usize,
    pub playlist_count: usize,
    pub smart_playlist_count: usize,
    pub favorite_track_count: usize,
    pub total_play_time_ms: i64,
    pub total_play_count: i64,
    pub average_track_duration_ms: i64,
    pub top_artists: Vec<RankedArtist>,
    pub top_tracks: Vec<RankedTrack>,
    pub quality_mix: Vec<QualityShare>,
    pub genre_breakdown: Vec<GenreShare>,
}

/// Build a dashboard-friendly summary from the current library snapshot.
pub fn summarize_library(
    tracks: &[Track],
    playlists: &[Playlist],
    context: &AnalyticsContext,
) -> LibraryAnalytics {
    let track_count = tracks.len();
    let playlist_count = playlists.len();
    let smart_playlist_count = playlists
        .iter()
        .filter(|playlist| playlist.is_smart)
        .count();
    let favorite_track_count = tracks.iter().filter(|track| track.is_favorite).count();
    let total_play_time_ms = tracks
        .iter()
        .map(|track| track.duration_ms.unwrap_or(0))
        .sum::<i64>();
    let total_play_count = tracks
        .iter()
        .map(|track| i64::from(track.play_count))
        .sum::<i64>();
    let average_track_duration_ms = if track_count == 0 {
        0
    } else {
        total_play_time_ms / track_count as i64
    };

    let mut artist_totals: HashMap<String, (usize, i64)> = HashMap::new();
    for track in tracks {
        let artist_name = track
            .artist_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("Unknown");
        let entry = artist_totals.entry(artist_name.to_string()).or_default();
        entry.0 += 1;
        entry.1 += i64::from(track.play_count);
    }

    let mut top_artists = artist_totals
        .into_iter()
        .map(|(name, (track_count, play_count))| RankedArtist {
            name,
            track_count,
            play_count,
        })
        .collect::<Vec<_>>();
    top_artists.sort_by(|left, right| {
        right
            .play_count
            .cmp(&left.play_count)
            .then(right.track_count.cmp(&left.track_count))
            .then(left.name.cmp(&right.name))
    });
    top_artists.truncate(8);

    let mut top_tracks = tracks
        .iter()
        .map(|track| RankedTrack {
            id: track.id,
            title: track.title.clone(),
            artist_name: track.artist_name.clone(),
            play_count: track.play_count,
            duration_ms: track.duration_ms,
            best_quality: track.best_quality.clone(),
            date_added: track.date_added.clone(),
            last_played_at: track.last_played_at.clone(),
        })
        .collect::<Vec<_>>();
    top_tracks.sort_by(compare_ranked_tracks);
    top_tracks.truncate(8);

    let mut quality_totals: HashMap<String, usize> = HashMap::new();
    for track in tracks {
        let label = quality_label(track.best_quality.as_deref());
        *quality_totals.entry(label.to_string()).or_default() += 1;
    }
    let mut quality_mix = quality_totals
        .into_iter()
        .map(|(label, count)| QualityShare {
            label,
            count,
            share: if track_count == 0 {
                0.0
            } else {
                count as f32 / track_count as f32
            },
        })
        .collect::<Vec<_>>();
    quality_mix.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.label.cmp(&right.label))
    });

    let mut genre_totals: HashMap<String, usize> = HashMap::new();
    for track in tracks {
        if let Some(genres) = context.genres_for_track(track.id) {
            for genre in genres {
                *genre_totals.entry(genre.clone()).or_default() += 1;
            }
        }
    }
    let mut genre_breakdown = genre_totals
        .into_iter()
        .map(|(name, count)| GenreShare { name, count })
        .collect::<Vec<_>>();
    genre_breakdown.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.name.cmp(&right.name))
    });
    genre_breakdown.truncate(10);

    LibraryAnalytics {
        track_count,
        playlist_count,
        smart_playlist_count,
        favorite_track_count,
        total_play_time_ms,
        total_play_count,
        average_track_duration_ms,
        top_artists,
        top_tracks,
        quality_mix,
        genre_breakdown,
    }
}

fn compare_ranked_tracks(left: &RankedTrack, right: &RankedTrack) -> Ordering {
    right
        .play_count
        .cmp(&left.play_count)
        .then(right.last_played_at.cmp(&left.last_played_at))
        .then(right.date_added.cmp(&left.date_added))
        .then(left.title.cmp(&right.title))
}

fn quality_label(value: Option<&str>) -> &'static str {
    match value.map(|raw| raw.trim().to_ascii_uppercase()) {
        Some(label) if label.contains("HI_RES") || label.contains("MASTER") => "Hi-Res",
        Some(label) if label.contains("LOSSLESS") || label.contains("FLAC") => "Lossless",
        Some(label) if label.contains("HIGH") || label.contains("AAC") || label.contains("MP3") => {
            "Compressed"
        }
        Some(_) => "Other",
        None => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(
        id: i64,
        title: &str,
        artist_name: &str,
        play_count: i32,
        best_quality: Option<&str>,
        duration_ms: Option<i64>,
        date_added: Option<&str>,
    ) -> Track {
        Track {
            id,
            title: title.to_string(),
            artist_id: id * 10,
            artist_name: Some(artist_name.to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms,
            isrc: None,
            tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: best_quality.map(str::to_string),
            best_source: None,
            fidelity_score: 0,
            is_favorite: id == 1,
            play_count,
            last_played_at: None,
            date_added: date_added.map(str::to_string),
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    #[test]
    fn summarizes_library_across_track_and_playlist_counts() {
        let tracks = vec![
            track(
                1,
                "One",
                "Artist A",
                12,
                Some("LOSSLESS"),
                Some(180_000),
                Some("2025-03-01 10:00:00"),
            ),
            track(
                2,
                "Two",
                "Artist A",
                2,
                Some("HI_RES"),
                Some(200_000),
                Some("2025-03-02 10:00:00"),
            ),
            track(
                3,
                "Three",
                "Artist B",
                8,
                Some("AAC"),
                Some(240_000),
                Some("2025-03-03 10:00:00"),
            ),
        ];
        let playlists = vec![
            Playlist {
                id: 1,
                tidal_uuid: None,
                name: "Mix".into(),
                description: None,
                is_smart: false,
                smart_rules: None,
                is_synced: true,
                track_count: 3,
            },
            Playlist {
                id: 2,
                tidal_uuid: None,
                name: "Deep Cuts".into(),
                description: None,
                is_smart: true,
                smart_rules: None,
                is_synced: true,
                track_count: 2,
            },
        ];
        let context = AnalyticsContext::new()
            .with_track_genres(1, ["Electronic > Ambient"])
            .with_track_genres(2, ["Electronic > IDM"]);

        let summary = summarize_library(&tracks, &playlists, &context);
        assert_eq!(summary.track_count, 3);
        assert_eq!(summary.playlist_count, 2);
        assert_eq!(summary.smart_playlist_count, 1);
        assert_eq!(summary.favorite_track_count, 1);
        assert_eq!(summary.top_artists[0].name, "Artist A");
        assert_eq!(summary.quality_mix[0].label, "Compressed");
        assert_eq!(summary.genre_breakdown.len(), 2);
    }
}
