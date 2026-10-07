//! The crawler's view of TIDAL and Last.fm, behind a trait so tests run
//! offline against `FakeSource`.

use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;

use super::harvest;
use crate::metadata::lastfm::LastFmClient;
use crate::services::tidal::client::{TidalArtistVideo, TidalClient};
use crate::services::video_sets::VideoCandidate;

pub const ARTIST_PAGE_SIZE: i32 = 50;
const CALL_TIMEOUT: Duration = Duration::from_secs(8);
const LASTFM_SPACING: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistRef {
    pub id: i64,
    pub name: String,
    pub popularity: Option<i32>,
    pub mix_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VideoPage {
    pub videos: Vec<TidalArtistVideo>,
    pub total: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct EditorialModule {
    pub key: String,
    pub videos: Vec<VideoCandidate>,
    pub playlist_ids: Vec<String>,
}

pub trait DiscoverySource: Send + Sync {
    fn artist_videos(
        &self,
        artist_id: i64,
        offset: i64,
    ) -> impl Future<Output = Result<VideoPage>> + Send;
    fn similar_artists(
        &self,
        artist_id: i64,
    ) -> impl Future<Output = Result<Vec<ArtistRef>>> + Send;
    fn artist(&self, artist_id: i64) -> impl Future<Output = Result<ArtistRef>> + Send;
    fn search_artists(&self, name: &str) -> impl Future<Output = Result<Vec<ArtistRef>>> + Send;
    /// `Ok(None)` when Last.fm is not configured.
    fn lastfm_similar(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<Vec<(String, Option<f64>)>>>> + Send;
    fn lastfm_tags(&self, name: &str) -> impl Future<Output = Result<Option<Vec<String>>>> + Send;
    fn video(&self, video_id: i64) -> impl Future<Output = Result<VideoCandidate>> + Send;
    fn search_videos(
        &self,
        query: &str,
    ) -> impl Future<Output = Result<Vec<VideoCandidate>>> + Send;
    fn video_mix_ids(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
    fn mix_videos(&self, mix_id: &str) -> impl Future<Output = Result<Vec<VideoCandidate>>> + Send;
    fn editorial_modules(&self) -> impl Future<Output = Result<Vec<EditorialModule>>> + Send;
    fn playlist_videos(
        &self,
        uuid: &str,
    ) -> impl Future<Output = Result<Vec<VideoCandidate>>> + Send;
}

async fn timed<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(CALL_TIMEOUT, future)
        .await
        .map_err(|_| anyhow::anyhow!("request timed out"))?
}

fn artist_ref(id: i64, name: &str, extra: &HashMap<String, Value>) -> ArtistRef {
    let (popularity, mix_id) = harvest::artist_facts(extra);
    ArtistRef {
        id,
        name: name.to_string(),
        popularity,
        mix_id,
    }
}

pub struct LiveSource {
    tidal: TidalClient,
    lastfm: Option<LastFmClient>,
    lastfm_last: tokio::sync::Mutex<Option<Instant>>,
}

impl LiveSource {
    pub fn new(tidal: TidalClient, lastfm: Option<LastFmClient>) -> Self {
        Self {
            tidal,
            lastfm,
            lastfm_last: tokio::sync::Mutex::new(None),
        }
    }

    async fn lastfm_turn(&self) {
        let mut last = self.lastfm_last.lock().await;
        if let Some(at) = *last {
            let since = at.elapsed();
            if since < LASTFM_SPACING {
                tokio::time::sleep(LASTFM_SPACING - since).await;
            }
        }
        *last = Some(Instant::now());
    }
}

impl DiscoverySource for LiveSource {
    async fn artist_videos(&self, artist_id: i64, offset: i64) -> Result<VideoPage> {
        let offset = i32::try_from(offset).unwrap_or(i32::MAX);
        let page = timed(
            self.tidal
                .get_artist_videos(artist_id, ARTIST_PAGE_SIZE, offset),
        )
        .await?;
        Ok(VideoPage {
            videos: page.items,
            total: page.total_number_of_items,
        })
    }

    async fn similar_artists(&self, artist_id: i64) -> Result<Vec<ArtistRef>> {
        let page = timed(self.tidal.get_artist_similar(artist_id, 10, 0)).await?;
        Ok(page
            .items
            .iter()
            .map(|a| artist_ref(a.id, &a.name, &a.extra))
            .collect())
    }

    async fn artist(&self, artist_id: i64) -> Result<ArtistRef> {
        let artist = timed(self.tidal.get_artist(artist_id)).await?;
        Ok(artist_ref(artist.id, &artist.name, &artist.extra))
    }

    async fn search_artists(&self, name: &str) -> Result<Vec<ArtistRef>> {
        let found = timed(self.tidal.search_artists(name, 5)).await?;
        Ok(found
            .iter()
            .map(|a| artist_ref(a.id, &a.name, &a.extra))
            .collect())
    }

    async fn lastfm_similar(&self, name: &str) -> Result<Option<Vec<(String, Option<f64>)>>> {
        let Some(lastfm) = &self.lastfm else {
            return Ok(None);
        };
        self.lastfm_turn().await;
        let similar = timed(lastfm.artist_get_similar(name, 20)).await?;
        Ok(Some(
            similar
                .into_iter()
                .map(|a| (a.name, a.match_score))
                .collect(),
        ))
    }

    async fn lastfm_tags(&self, name: &str) -> Result<Option<Vec<String>>> {
        let Some(lastfm) = &self.lastfm else {
            return Ok(None);
        };
        self.lastfm_turn().await;
        let tags = timed(lastfm.artist_top_tags(name)).await?;
        Ok(Some(tags.into_iter().take(5).map(|(tag, _)| tag).collect()))
    }

    async fn video(&self, video_id: i64) -> Result<VideoCandidate> {
        let video = timed(self.tidal.get_video(video_id)).await?;
        Ok(VideoCandidate::from(&video))
    }

    async fn search_videos(&self, query: &str) -> Result<Vec<VideoCandidate>> {
        let found = timed(self.tidal.search_videos(query, 20, 0)).await?;
        Ok(found.iter().map(VideoCandidate::from).collect())
    }

    async fn video_mix_ids(&self) -> Result<Vec<String>> {
        let mixes = timed(self.tidal.get_my_mixes()).await?;
        Ok(mixes
            .into_iter()
            .filter(|mix| mix.is_video_mix)
            .map(|mix| mix.id)
            .collect())
    }

    async fn mix_videos(&self, mix_id: &str) -> Result<Vec<VideoCandidate>> {
        let items = timed(self.tidal.get_video_mix_items(mix_id)).await?;
        Ok(items.iter().map(VideoCandidate::from).collect())
    }

    async fn editorial_modules(&self) -> Result<Vec<EditorialModule>> {
        let modules = timed(self.tidal.get_page_modules("pages/videos")).await?;
        Ok(modules
            .into_iter()
            .map(|module| EditorialModule {
                key: module.id.clone(),
                videos: module
                    .items
                    .iter()
                    .filter_map(harvest::candidate_from_home_item)
                    .collect(),
                playlist_ids: module
                    .items
                    .iter()
                    .filter(|item| item.kind == "playlist")
                    .map(|item| item.id.clone())
                    .collect(),
            })
            .collect())
    }

    async fn playlist_videos(&self, uuid: &str) -> Result<Vec<VideoCandidate>> {
        let items = timed(self.tidal.get_playlist_video_items(uuid)).await?;
        Ok(items.iter().map(VideoCandidate::from).collect())
    }
}

#[cfg(test)]
pub mod fake {
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use anyhow::Result;

    use super::*;
    use crate::services::tidal::client::TidalArtist;

    #[derive(Default)]
    pub struct FakeSource {
        pub artist_videos: HashMap<i64, Vec<TidalArtistVideo>>,
        pub failing_artists: HashSet<i64>,
        pub similar: HashMap<i64, Vec<ArtistRef>>,
        pub failing_similar: HashSet<i64>,
        pub artists: HashMap<i64, ArtistRef>,
        pub search: HashMap<String, Vec<ArtistRef>>,
        pub lastfm: Option<HashMap<String, Vec<(String, Option<f64>)>>>,
        pub videos: HashMap<i64, VideoCandidate>,
        pub mixes: HashMap<String, Vec<VideoCandidate>>,
        pub editorial: Vec<EditorialModule>,
        pub playlists: HashMap<String, Vec<VideoCandidate>>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeSource {
        fn log(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    pub fn artist_video(id: i64, artist_id: i64, artist: &str, title: &str) -> TidalArtistVideo {
        TidalArtistVideo {
            id,
            title: title.into(),
            duration: 200,
            image_id: None,
            artist: Some(TidalArtist {
                id: artist_id,
                name: artist.into(),
                picture: None,
                extra: HashMap::new(),
            }),
            album: None,
            extra: HashMap::new(),
        }
    }

    pub fn artist(id: i64, name: &str, popularity: Option<i32>) -> ArtistRef {
        ArtistRef {
            id,
            name: name.into(),
            popularity,
            mix_id: None,
        }
    }

    pub fn candidate(id: i64, artist_id: i64) -> VideoCandidate {
        VideoCandidate {
            tidal_id: id,
            title: format!("Video {id}"),
            artist_id: Some(artist_id),
            artist_name: Some(format!("Artist {artist_id}")),
            ..Default::default()
        }
    }

    impl DiscoverySource for FakeSource {
        async fn artist_videos(&self, artist_id: i64, offset: i64) -> Result<VideoPage> {
            self.log(format!("artist_videos:{artist_id}:{offset}"));
            if self.failing_artists.contains(&artist_id) {
                anyhow::bail!("TIDAL API error 500 Internal Server Error");
            }
            let all = self
                .artist_videos
                .get(&artist_id)
                .cloned()
                .unwrap_or_default();
            let total = all.len() as i64;
            let videos = all
                .into_iter()
                .skip(offset as usize)
                .take(ARTIST_PAGE_SIZE as usize)
                .collect();
            Ok(VideoPage {
                videos,
                total: Some(total),
            })
        }

        async fn similar_artists(&self, artist_id: i64) -> Result<Vec<ArtistRef>> {
            self.log(format!("similar:{artist_id}"));
            if self.failing_similar.contains(&artist_id) {
                anyhow::bail!("TIDAL API error 503");
            }
            Ok(self.similar.get(&artist_id).cloned().unwrap_or_default())
        }

        async fn artist(&self, artist_id: i64) -> Result<ArtistRef> {
            self.log(format!("artist:{artist_id}"));
            self.artists
                .get(&artist_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown artist"))
        }

        async fn search_artists(&self, name: &str) -> Result<Vec<ArtistRef>> {
            self.log(format!("search_artists:{name}"));
            Ok(self.search.get(name).cloned().unwrap_or_default())
        }

        async fn lastfm_similar(&self, name: &str) -> Result<Option<Vec<(String, Option<f64>)>>> {
            Ok(self
                .lastfm
                .as_ref()
                .map(|all| all.get(name).cloned().unwrap_or_default()))
        }

        async fn lastfm_tags(&self, _name: &str) -> Result<Option<Vec<String>>> {
            Ok(self.lastfm.as_ref().map(|_| Vec::new()))
        }

        async fn video(&self, video_id: i64) -> Result<VideoCandidate> {
            self.log(format!("video:{video_id}"));
            self.videos
                .get(&video_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown video"))
        }

        async fn search_videos(&self, query: &str) -> Result<Vec<VideoCandidate>> {
            self.log(format!("search_videos:{query}"));
            Ok(Vec::new())
        }

        async fn video_mix_ids(&self) -> Result<Vec<String>> {
            self.log("video_mix_ids".into());
            let mut ids: Vec<String> = self.mixes.keys().cloned().collect();
            ids.sort();
            Ok(ids)
        }

        async fn mix_videos(&self, mix_id: &str) -> Result<Vec<VideoCandidate>> {
            self.log(format!("mix:{mix_id}"));
            Ok(self.mixes.get(mix_id).cloned().unwrap_or_default())
        }

        async fn editorial_modules(&self) -> Result<Vec<EditorialModule>> {
            self.log("editorial".into());
            Ok(self.editorial.clone())
        }

        async fn playlist_videos(&self, uuid: &str) -> Result<Vec<VideoCandidate>> {
            self.log(format!("playlist:{uuid}"));
            Ok(self.playlists.get(uuid).cloned().unwrap_or_default())
        }
    }
}
