use anyhow::Context;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// An article or news item from an RSS feed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub title: String,
    pub link: String,
    pub description: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub image_url: Option<String>,
    pub source: String,   // e.g. "AllMusic", "Billboard"
    pub category: String, // "article", "news", "release"
}

/// Cached feed data with timestamp for TTL management
#[derive(Debug, Clone)]
struct CachedFeed {
    items: Vec<FeedItem>,
    fetched_at: DateTime<Utc>,
}

/// RSS Feed aggregator with caching
pub struct FeedAggregator {
    http_client: Client,
    cache: Arc<RwLock<HashMap<String, CachedFeed>>>,
    cache_ttl: std::time::Duration,
}

/// Feed source configuration
#[derive(Clone)]
struct FeedSource {
    url: &'static str,
    name: &'static str,
    category: &'static str,
}

/// AllMusic RSS feeds
const ALLMUSIC_FEEDS: [FeedSource; 1] = [FeedSource {
    url: "https://www.allmusic.com/rss/all",
    name: "AllMusic",
    category: "mixed",
}];

/// Music news RSS feeds
const NEWS_FEEDS: &[FeedSource] = &[
    FeedSource {
        url: "https://www.billboard.com/feed",
        name: "Billboard",
        category: "news",
    },
    FeedSource {
        url: "https://www.nme.com/news/music/feed",
        name: "NME",
        category: "news",
    },
    FeedSource {
        url: "https://spin.com/feed",
        name: "SPIN",
        category: "news",
    },
    FeedSource {
        url: "https://pitchfork.com/rss/news/",
        name: "Pitchfork",
        category: "news",
    },
    FeedSource {
        url: "https://www.rollingstone.com/music/feed",
        name: "Rolling Stone",
        category: "news",
    },
    FeedSource {
        url: "https://consequenceofsound.net/feed",
        name: "Consequence",
        category: "news",
    },
    FeedSource {
        url: "https://www.theguardian.com/music/rss",
        name: "The Guardian Music",
        category: "news",
    },
    FeedSource {
        url: "https://www.factmag.com/feed/",
        name: "FACT Magazine",
        category: "news",
    },
    FeedSource {
        url: "https://daily.bandcamp.com/feed",
        name: "Bandcamp Daily",
        category: "news",
    },
    FeedSource {
        url: "https://mixmag.net/rss-category/news",
        name: "Mixmag",
        category: "news",
    },
    FeedSource {
        url: "https://www.stereogum.com/feed/",
        name: "Stereogum",
        category: "news",
    },
];

impl FeedAggregator {
    pub fn new(http_client: Client) -> Self {
        Self {
            http_client,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: std::time::Duration::from_secs(86400), // 24 hours
        }
    }

    /// Fetch and parse a single RSS feed
    async fn fetch_feed(&self, source: &FeedSource) -> anyhow::Result<Vec<FeedItem>> {
        debug!(url = %source.url, "Fetching RSS feed");

        let resp = self
            .http_client
            .get(source.url)
            .header("User-Agent", "NOORwave/1.0")
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .context("Failed to fetch RSS feed")?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("RSS feed returned HTTP {}", resp.status()));
        }

        let body = resp.text().await.context("Failed to read RSS body")?;
        // Strip UTF-8 BOM and leading whitespace — some feeds (e.g. Bandcamp Daily)
        // prepend a BOM that causes the RSS parser to fail with "input did not begin with rss tag".
        let body_trimmed = body.trim_start_matches('\u{FEFF}').trim_start();

        let cursor = std::io::Cursor::new(body_trimmed.as_bytes());
        let channel = match rss::Channel::read_from(cursor) {
            Ok(channel) => channel,
            Err(e) => {
                warn!(error = %e, "Failed to parse RSS, trying alternative parsing");
                return self.parse_xml_fallback(body_trimmed, source);
            }
        };

        let items = channel
            .items
            .into_iter()
            .filter_map(|item| {
                let title = item.title.unwrap_or_else(|| "Untitled".to_string());
                let link = item.link.unwrap_or_default();
                let description = item.description.unwrap_or_default();
                let author = item.author.or_else(|| {
                    item.itunes_ext
                        .as_ref()
                        .and_then(|ext| ext.author.as_ref())
                        .cloned()
                });

                let published_at = item.pub_date;

                let image_url = item
                    .itunes_ext
                    .as_ref()
                    .and_then(|ext| ext.image.as_ref())
                    .cloned()
                    .or_else(|| {
                        // media:thumbnail url="..."
                        item.extensions
                            .get("media")
                            .and_then(|ns| ns.get("thumbnail"))
                            .and_then(|v| v.first())
                            .and_then(|ext| ext.attrs.get("url").cloned())
                    })
                    .or_else(|| {
                        // media:content url="..." medium="image"
                        item.extensions
                            .get("media")
                            .and_then(|ns| ns.get("content"))
                            .and_then(|v| {
                                v.iter().find(|e| {
                                    e.attrs.get("medium").map(|m| m == "image").unwrap_or(false)
                                        || e.attrs
                                            .get("type")
                                            .map(|t| t.starts_with("image/"))
                                            .unwrap_or(false)
                                })
                            })
                            .and_then(|ext| ext.attrs.get("url").cloned())
                    })
                    .or_else(|| {
                        // First <img src="..."> in content:encoded HTML
                        item.extensions
                            .get("content")
                            .and_then(|ns| ns.get("encoded"))
                            .and_then(|v| v.first())
                            .and_then(|ext| ext.value.as_deref())
                            .and_then(|html| {
                                let lower = html.to_ascii_lowercase();
                                let img_pos = lower.find("<img ")?;
                                let src_pos = lower[img_pos..].find("src=\"")? + img_pos + 5;
                                let end = lower[src_pos..].find('"')? + src_pos;
                                Some(html[src_pos..end].to_string())
                            })
                    })
                    .or_else(|| {
                        // <enclosure> with image mime type
                        item.enclosure
                            .as_ref()
                            .filter(|enc| enc.mime_type.starts_with("image/"))
                            .map(|enc| enc.url.clone())
                    });

                if link.is_empty() {
                    None
                } else {
                    Some(FeedItem {
                        title,
                        link,
                        description: Self::truncate_desc(&description, 280),
                        author,
                        published_at,
                        image_url,
                        source: source.name.to_string(),
                        category: source.category.to_string(),
                    })
                }
            })
            .collect();

        Ok(items)
    }

    /// Fallback XML parsing for feeds that don't strictly follow RSS spec
    fn parse_xml_fallback(&self, body: &str, source: &FeedSource) -> anyhow::Result<Vec<FeedItem>> {
        warn!("Using fallback XML parsing for {}", source.name);

        // Basic fallback: try to extract items with regex-like approach
        // This handles Atom feeds and other non-standard formats
        let mut items = Vec::new();

        // Simple Atom feed parsing
        if body.contains("<feed") || body.contains("<entry") {
            let entry_re = regex::Regex::new(r#"(?s)<entry\b[^>]*>(.*?)</entry>"#)
                .context("Failed to compile Atom entry parser")?;
            let title_re = regex::Regex::new(r#"(?s)<title[^>]*>(.*?)</title>"#)
                .context("Failed to compile Atom title parser")?;
            let link_re = regex::Regex::new(r#"(?s)<link\b[^>]*href="([^"]+)""#)
                .context("Failed to compile Atom link parser")?;
            let summary_re =
                regex::Regex::new(r#"(?s)<(?:summary|description|content)[^>]*>(.*?)</(?:summary|description|content)>"#)
                    .context("Failed to compile Atom summary parser")?;

            for entry_cap in entry_re.captures_iter(body) {
                let entry = entry_cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                let title = title_re
                    .captures(entry)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_else(|| "Untitled".to_string());
                let link = link_re
                    .captures(entry)
                    .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()));
                let summary = summary_re
                    .captures(entry)
                    .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()));

                if let Some(link) = link {
                    items.push(FeedItem {
                        title,
                        link,
                        description: Self::truncate_desc(&summary.unwrap_or_default(), 280),
                        author: None,
                        published_at: None,
                        image_url: None,
                        source: source.name.to_string(),
                        category: source.category.to_string(),
                    });
                }
            }
        }

        if items.is_empty() && body.contains("<feed") {
            let title_re = regex::Regex::new(r#"(?s)<title[^>]*>(.*?)</title>"#)
                .context("Failed to compile fallback title parser")?;
            let link_re = regex::Regex::new(r#"(?s)<link\b[^>]*href="([^"]+)""#)
                .context("Failed to compile fallback link parser")?;
            let summary_re =
                regex::Regex::new(r#"(?s)<(?:summary|description|content)[^>]*>(.*?)</(?:summary|description|content)>"#)
                    .context("Failed to compile fallback summary parser")?;

            for title_cap in title_re.captures_iter(body) {
                let title = title_cap
                    .get(1)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                let link = link_re
                    .captures_iter(body)
                    .next()
                    .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()));
                let summary = summary_re
                    .captures_iter(body)
                    .next()
                    .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()));

                if let Some(link) = link {
                    items.push(FeedItem {
                        title,
                        link,
                        description: Self::truncate_desc(&summary.unwrap_or_default(), 280),
                        author: None,
                        published_at: None,
                        image_url: None,
                        source: source.name.to_string(),
                        category: source.category.to_string(),
                    });
                }
            }
        }

        if items.is_empty() {
            warn!("Fallback parsing returned no items for {}", source.name);
        }

        Ok(items)
    }

    fn truncate_desc(s: &str, max: usize) -> String {
        let stripped = html_cleaner::clean_html(s);
        if stripped.len() <= max {
            return stripped;
        }
        let mut end = max;
        while end > 0 && !stripped.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &stripped[..end])
    }

    /// Fetch all feeds for a category (articles or news)
    async fn fetch_category_feeds(&self, sources: Vec<FeedSource>) -> Vec<FeedItem> {
        let mut all_items = Vec::new();

        // Fetch feeds in parallel with timeout
        let fetches: Vec<_> = sources
            .into_iter()
            .map(|source| async move {
                match self.fetch_feed(&source).await {
                    Ok(items) => {
                        debug!(source = %source.name, count = items.len(), "Fetched RSS feed");
                        Some(items)
                    }
                    Err(e) => {
                        warn!(source = %source.name, error = %e, "Failed to fetch RSS feed");
                        None
                    }
                }
            })
            .collect();

        // Await all fetches
        let results = futures::future::join_all(fetches).await;

        for items in results.into_iter().flatten() {
            all_items.extend(items);
        }

        // Sort by published date (newest first), fallback to title
        all_items.sort_by(|a, b| {
            let a_date = a
                .published_at
                .as_ref()
                .and_then(|d| DateTime::parse_from_rfc2822(d).ok());
            let b_date = b
                .published_at
                .as_ref()
                .and_then(|d| DateTime::parse_from_rfc2822(d).ok());

            match (a_date, b_date) {
                (Some(a), Some(b)) => b.cmp(&a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.title.cmp(&b.title),
            }
        });

        all_items
    }

    /// Get articles (AllMusic weekly articles)
    pub async fn get_articles(&self) -> Vec<FeedItem> {
        let cache_key = "allmusic_all";
        let all_items = self
            .get_cached_or_fetch(cache_key, ALLMUSIC_FEEDS.to_vec())
            .await;

        // Filter for article-like content (longer titles, non-album items)
        all_items
            .into_iter()
            .filter(|item| {
                // Articles typically have longer titles and don't look like "Artist - Album"
                item.title.len() > 20 && !item.title.contains(" - ")
            })
            .take(15)
            .collect()
    }

    /// Get music news (aggregated from multiple sources)
    pub async fn get_news(&self) -> Vec<FeedItem> {
        let cache_key = "music_news";
        self.get_cached_or_fetch(cache_key, NEWS_FEEDS.to_vec())
            .await
    }

    async fn get_cached_or_fetch(
        &self,
        cache_key: &str,
        sources: Vec<FeedSource>,
    ) -> Vec<FeedItem> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(cache_key) {
                let age = Utc::now().signed_duration_since(cached.fetched_at);
                if age < chrono::Duration::from_std(self.cache_ttl).unwrap_or_default() {
                    return cached.items.clone();
                }
            }
        }

        // Fetch fresh data
        let items = self.fetch_category_feeds(sources).await;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.insert(
            cache_key.to_string(),
            CachedFeed {
                items: items.clone(),
                fetched_at: Utc::now(),
            },
        );

        items
    }
}

// We need html_cleaner - let's use a simple approach instead
mod html_cleaner {
    /// Strip HTML tags from a string (very basic implementation)
    pub fn clean_html(html: &str) -> String {
        let mut in_tag = false;
        let mut result = String::with_capacity(html.len());

        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => result.push(c),
                _ => {}
            }
        }

        // Collapse whitespace
        let mut cleaned = result.split_whitespace().collect::<Vec<_>>().join(" ");

        // Decode common HTML entities
        cleaned = cleaned
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
            .replace("&#x27;", "'")
            .replace("&nbsp;", " ");

        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source() -> FeedSource {
        FeedSource {
            url: "https://example.test/feed",
            name: "Example",
            category: "news",
        }
    }

    #[test]
    fn atom_fallback_uses_entry_scoped_links_and_summaries() {
        let aggregator = FeedAggregator::new(Client::new());
        let body = r#"
            <feed>
              <title>Example feed</title>
              <entry>
                <title>First article</title>
                <link href="https://example.test/first"/>
                <summary>First summary &amp; detail</summary>
              </entry>
              <entry>
                <title>Second article</title>
                <link href="https://example.test/second"/>
                <summary>Second summary</summary>
              </entry>
            </feed>
        "#;

        let items = aggregator
            .parse_xml_fallback(body, &test_source())
            .expect("fallback parses Atom entries");

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "First article");
        assert_eq!(items[0].link, "https://example.test/first");
        assert_eq!(items[0].description, "First summary & detail");
        assert_eq!(items[1].title, "Second article");
        assert_eq!(items[1].link, "https://example.test/second");
        assert_eq!(items[1].description, "Second summary");
    }
}
