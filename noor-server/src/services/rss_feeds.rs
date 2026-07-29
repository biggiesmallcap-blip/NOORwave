use anyhow::Context;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

const RSS_ACCEPT_HEADER: &str =
    "application/rss+xml, application/xml;q=0.9, text/xml;q=0.8, */*;q=0.1";
const RSS_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; NOORwave/1.0; +https://github.com/felix/noorwave)";

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

/// Long-form music writing, for the "Weekly articles" shelf.
///
/// This was a single AllMusic feed, which now answers 403 to everything -
/// every path, every User-Agent, so it is an edge block rather than
/// something a header can talk its way past, and the shelf had been empty
/// for as long as that has been true. These four replace it. All were
/// checked for a 200 and a non-empty item list before being added, and none
/// duplicate `NEWS_FEEDS` (which already carries Bandcamp Daily, Stereogum
/// and Pitchfork's news feed - this uses Pitchfork's separate features
/// feed, which does not overlap).
///
/// URLs are the post-redirect destinations, so a fetch costs one hop.
/// Keep these music-only; see the note below the list.
const ARTICLE_FEEDS: &[FeedSource] = &[
    FeedSource {
        url: "https://thequietus.com/feed/",
        name: "The Quietus",
        category: "article",
    },
    FeedSource {
        url: "https://pitchfork.com/feed/feed-features/rss",
        name: "Pitchfork Features",
        category: "article",
    },
    FeedSource {
        url: "https://aquariumdrunkard.com/feed/",
        name: "Aquarium Drunkard",
        category: "article",
    },
];
// Louder (loudersound.com/feeds.xml) was a candidate and was rejected: it is
// Future's site-wide feed, so a third of it is film and TV ("Every Spider-Man
// movie ranked"). The three above yield 73 items past the filter and the
// shelf shows 15, so there is no need to buy volume with relevance.

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
            .header("Accept", RSS_ACCEPT_HEADER)
            .header("User-Agent", RSS_USER_AGENT)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .context("Failed to fetch RSS feed")?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("RSS feed returned HTTP {}", resp.status()));
        }

        let body = resp.text().await.context("Failed to read RSS body")?;
        // Strip UTF-8 BOM and leading whitespace. Some feeds, including Bandcamp Daily,
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

        let mut items = Vec::new();

        if body.contains("<item") {
            items.extend(Self::parse_rss_item_fallback(body, source)?);
        }

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

        if items.is_empty() && source.name == "Bandcamp Daily" {
            items.extend(Self::parse_bandcamp_plaintext_fallback(body, source)?);
        }

        if items.is_empty() {
            warn!("Fallback parsing returned no items for {}", source.name);
        }

        Ok(items)
    }

    fn parse_rss_item_fallback(body: &str, source: &FeedSource) -> anyhow::Result<Vec<FeedItem>> {
        let item_re = regex::Regex::new(r#"(?s)<item\b[^>]*>(.*?)</item>"#)
            .context("Failed to compile RSS item parser")?;
        let title_re =
            regex::Regex::new(r#"(?s)<title[^>]*>(?:<!\[CDATA\[)?(.*?)(?:\]\]>)?</title>"#)
                .context("Failed to compile RSS title parser")?;
        let link_re = regex::Regex::new(r#"(?s)<link[^>]*>(?:<!\[CDATA\[)?(.*?)(?:\]\]>)?</link>"#)
            .context("Failed to compile RSS link parser")?;
        let description_re =
            regex::Regex::new(r#"(?s)<(?:description|content:encoded)[^>]*>(?:<!\[CDATA\[)?(.*?)(?:\]\]>)?</(?:description|content:encoded)>"#)
                .context("Failed to compile RSS description parser")?;
        let author_re =
            regex::Regex::new(r#"(?s)<(?:dc:creator|author)[^>]*>(?:<!\[CDATA\[)?(.*?)(?:\]\]>)?</(?:dc:creator|author)>"#)
                .context("Failed to compile RSS author parser")?;
        let pub_date_re = regex::Regex::new(r#"(?s)<pubDate[^>]*>(.*?)</pubDate>"#)
            .context("Failed to compile RSS pubDate parser")?;
        let image_attr_re = regex::Regex::new(
            r#"(?s)<(?:media:thumbnail|media:content|enclosure)\b[^>]*\burl="([^"]+)""#,
        )
        .context("Failed to compile RSS image parser")?;

        let mut items = Vec::new();
        for item_cap in item_re.captures_iter(body) {
            let item = item_cap.get(1).map(|m| m.as_str()).unwrap_or_default();
            let Some(link) = capture_cleaned(&link_re, item).filter(|value| !value.is_empty())
            else {
                continue;
            };

            items.push(FeedItem {
                title: capture_cleaned(&title_re, item).unwrap_or_else(|| "Untitled".to_string()),
                link,
                description: Self::truncate_desc(
                    &capture_raw(&description_re, item).unwrap_or_default(),
                    280,
                ),
                author: capture_cleaned(&author_re, item).filter(|value| !value.is_empty()),
                published_at: capture_cleaned(&pub_date_re, item).filter(|value| !value.is_empty()),
                image_url: image_attr_re
                    .captures(item)
                    .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
                    .filter(|value| !value.is_empty()),
                source: source.name.to_string(),
                category: source.category.to_string(),
            });
        }

        Ok(items)
    }

    fn parse_bandcamp_plaintext_fallback(
        body: &str,
        source: &FeedSource,
    ) -> anyhow::Result<Vec<FeedItem>> {
        let article_link_re = regex::Regex::new(r#"https://daily\.bandcamp\.com/[^\s]+"#)
            .context("Failed to compile Bandcamp plaintext link parser")?;
        let pub_date_re = regex::Regex::new(
            r#"(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun),\s+\d{1,2}\s+\w+\s+\d{4}\s+\d{2}:\d{2}:\d{2}\s+[-+]\d{4}"#,
        )
        .context("Failed to compile Bandcamp plaintext date parser")?;
        let author_tail_re =
            regex::Regex::new(r#"\s+\d+\s+.+?\s+\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\s*$"#)
                .context("Failed to compile Bandcamp plaintext author parser")?;

        let links: Vec<_> = article_link_re.find_iter(body).collect();
        let mut items = Vec::new();
        for (index, link_match) in links.iter().enumerate() {
            let link = link_match.as_str().trim().to_string();
            let title_source = &body[..link_match.start()];
            let title = title_source
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_string();
            if title.is_empty() || title == "Bandcamp Updates" {
                continue;
            }

            let body_end = links
                .get(index + 1)
                .map(|next| next.start())
                .unwrap_or(body.len());
            let raw_body = &body[link_match.end()..body_end];
            let published_at = pub_date_re
                .find(raw_body)
                .map(|m| m.as_str().trim().to_string());
            let description_source = pub_date_re
                .split(raw_body)
                .next()
                .map(|value| value.replace("Read full story on the Bandcamp Daily .", ""))
                .unwrap_or_default();
            let description = author_tail_re.replace(&description_source, "");

            items.push(FeedItem {
                title,
                link,
                description: Self::truncate_desc(description.trim(), 280),
                author: None,
                published_at,
                image_url: None,
                source: source.name.to_string(),
                category: source.category.to_string(),
            });
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

    /// Get long-form music writing for the "Weekly articles" shelf.
    pub async fn get_articles(&self) -> Vec<FeedItem> {
        // Key changed with the sources; the old one would have served a day of
        // cached emptiness from the feed that is now gone.
        let cache_key = "music_articles_v2";
        let all_items = self
            .get_cached_or_fetch(cache_key, ARTICLE_FEEDS.to_vec())
            .await;

        // The old filter existed to strip AllMusic's "Artist - Album" review
        // stubs out of a mixed feed. These sources publish articles only, and
        // the rule actively hurt them: it dropped anything with a hyphen
        // between spaces, which is ordinary punctuation in a headline. Keep
        // only the guard against a stub-length title.
        all_items
            .into_iter()
            .filter(|item| item.title.len() > 20)
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

fn capture_raw(re: &regex::Regex, haystack: &str) -> Option<String> {
    re.captures(haystack)
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
}

fn capture_cleaned(re: &regex::Regex, haystack: &str) -> Option<String> {
    capture_raw(re, haystack).map(|value| html_cleaner::clean_html(&value))
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

    #[test]
    fn fallback_parses_rss_items_when_channel_parser_rejects_root() {
        let aggregator = FeedAggregator::new(Client::new());
        let body = r#"
            <unexpected>
              <channel>
                <item>
                  <title><![CDATA[Bandcamp article]]></title>
                  <link>https://daily.bandcamp.com/lists/example</link>
                  <description><![CDATA[<p>Article summary &amp; context.</p>]]></description>
                  <dc:creator><![CDATA[Bandcamp Daily Staff]]></dc:creator>
                  <pubDate>Mon, 11 May 2026 17:50:05 -0000</pubDate>
                  <media:thumbnail url="https://f4.bcbits.com/img/example.jpg"/>
                </item>
              </channel>
            </unexpected>
        "#;

        let items = aggregator
            .parse_xml_fallback(body, &test_source())
            .expect("fallback parses RSS items");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Bandcamp article");
        assert_eq!(items[0].link, "https://daily.bandcamp.com/lists/example");
        assert_eq!(items[0].description, "Article summary & context.");
        assert_eq!(items[0].author.as_deref(), Some("Bandcamp Daily Staff"));
        assert_eq!(
            items[0].published_at.as_deref(),
            Some("Mon, 11 May 2026 17:50:05 -0000")
        );
        assert_eq!(
            items[0].image_url.as_deref(),
            Some("https://f4.bcbits.com/img/example.jpg")
        );
    }

    #[test]
    fn bandcamp_plaintext_fallback_returns_items() {
        let aggregator = FeedAggregator::new(Client::new());
        let source = FeedSource {
            url: "https://daily.bandcamp.com/feed",
            name: "Bandcamp Daily",
            category: "news",
        };
        let body = r#"
            Bandcamp Updates https://daily.bandcamp.com Bandcamp Daily is your guide to the artists, fans and labels on Bandcamp. en-US Tue, 12 May 2026 10:37:22 +0000
            The Polish Composers Pushing the Boundaries of Classical Music https://daily.bandcamp.com/lists/contemporary-polish-classical-album-guide
            A new crop of Polish composers are bringing a cinematic, imaginative outlook to their music.
            Read full story on the Bandcamp Daily .
            ]]> Lists Mon, 11 May 2026 17:50:05 -0000 192389 Michal Wieczorek 2026-05-11T17:50:05Z
            Nagoya's Electronic Scene Is Hiding in Plain Sight https://daily.bandcamp.com/scene-report/nagoya-electronic-scene-report
            In clubs, bars, and DIY spaces, a tight-knit community pushes electronic music in new directions.
            Read full story on the Bandcamp Daily .
            ]]> Scene Report Mon, 11 May 2026 13:43:18 -0000 192430 James Gui, Chau Luong 2026-05-11T13:43:18Z
        "#;

        let items = aggregator
            .parse_xml_fallback(body, &source)
            .expect("fallback parses Bandcamp plaintext");

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].title,
            "The Polish Composers Pushing the Boundaries of Classical Music"
        );
        assert_eq!(
            items[0].link,
            "https://daily.bandcamp.com/lists/contemporary-polish-classical-album-guide"
        );
        assert!(items[0].description.contains("Polish composers"));
        assert_eq!(
            items[0].published_at.as_deref(),
            Some("Mon, 11 May 2026 17:50:05 -0000")
        );
        assert_eq!(
            items[1].link,
            "https://daily.bandcamp.com/scene-report/nagoya-electronic-scene-report"
        );
    }
}
