//! Tidal match-quality probe.
//!
//! For each seed track, calls last.fm `track.getsimilar` to get 50 candidates,
//! then searches Tidal by `{artist} {title}` and compares the top result's
//! artist name to the last.fm artist using Jaro-Winkler.
//!
//! Purpose: validate whether the proposed 0.85 JW threshold for accepting a
//! Tidal match is conservative, too loose, or about right before hardcoding
//! it in the production radio resolver.
//!
//! Usage:
//!   cargo run --release -- [path/to/noor.db]
//!
//! Reads Tidal and last.fm credentials from noor.db (same tables noor-server uses).
//! Token bytes are never printed.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

const TIDAL_API_URL: &str = "https://api.tidal.com/v1";
const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

// Representative seeds spanning genres likely to appear as radio seeds.
// Chosen because they're popular enough that last.fm returns a full 50-track similar list.
const SEEDS: &[(&str, &str)] = &[
    ("Amy Shark", "I Said Hi"),
    ("Lizzo", "Truth Hurts"),
    ("The Weeknd", "Blinding Lights"),
    ("Phoebe Bridgers", "Funeral"),
    ("Vampire Weekend", "A-Punk"),
    ("Drake", "God's Plan"),
    ("Lorde", "Royals"),
    ("Bon Iver", "Skinny Love"),
    ("Kendrick Lamar", "HUMBLE."),
    ("Fleetwood Mac", "The Chain"),
];

#[derive(Debug, Deserialize)]
struct TidalTokens {
    access_token: String,
    #[allow(dead_code)]
    user_id: Option<String>,
    country_code: String,
}

#[derive(Debug, Deserialize)]
struct LastFmCredentials {
    api_key: String,
}

#[derive(Debug)]
struct SimilarTrack {
    artist: String,
    title: String,
    match_score: f64,
}

#[derive(Debug)]
struct MatchResult {
    lastfm_artist: String,
    lastfm_title: String,
    lastfm_match: f64,
    tidal_artist: Option<String>,
    tidal_title: Option<String>,
    artist_jw: f64,
    title_jw: f64,
    verdict: Verdict,
}

#[derive(Debug, PartialEq)]
enum Verdict {
    ExactArtist,  // normalized strings equal
    HighJW,       // artist JW > 0.9
    PassThreshold, // artist JW 0.85–0.9 (would pass the proposed filter)
    BelowThreshold, // artist JW 0.7–0.85 (would be rejected)
    Weak,         // artist JW < 0.7 (clearly wrong)
    NoResult,     // Tidal returned no tracks
}

impl Verdict {
    fn label(&self) -> &'static str {
        match self {
            Verdict::ExactArtist => "EXACT",
            Verdict::HighJW => "HIGH (>0.90)",
            Verdict::PassThreshold => "PASS (0.85–0.90)",
            Verdict::BelowThreshold => "BELOW (0.70–0.85)",
            Verdict::Weak => "WEAK (<0.70)",
            Verdict::NoResult => "NO_RESULT",
        }
    }

    fn is_acceptable(&self) -> bool {
        matches!(self, Verdict::ExactArtist | Verdict::HighJW | Verdict::PassThreshold)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let db_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../../noor.db".to_string());
    eprintln!("Opening DB: {}", db_path);

    let conn = rusqlite::Connection::open(&db_path).context("open noor.db")?;

    // Load Tidal token
    let tidal_token_bytes: Vec<u8> = conn
        .query_row(
            "SELECT access_token_enc FROM service_auth WHERE service='tidal'",
            [],
            |row| row.get(0),
        )
        .context("read tidal token — is the server authenticated to Tidal?")?;
    let tidal_json = String::from_utf8(tidal_token_bytes).context("tidal token not utf-8")?;
    let tidal: TidalTokens = serde_json::from_str(&tidal_json).context("parse tidal token JSON")?;
    eprintln!("Tidal token loaded (country={})", tidal.country_code);

    // Load last.fm API key
    let lastfm_extra: String = conn
        .query_row(
            "SELECT extra_data FROM service_auth WHERE service='lastfm'",
            [],
            |row| row.get(0),
        )
        .context("read lastfm credentials — configure last.fm in Settings first")?;
    let lastfm: LastFmCredentials =
        serde_json::from_str(&lastfm_extra).context("parse lastfm credentials JSON")?;
    eprintln!("last.fm API key loaded");

    let http = reqwest::Client::builder()
        .user_agent("TIDAL_ANDROID/1039 okhttp/3.14.9")
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    let mut grand_total = 0usize;
    let mut grand_acceptable = 0usize;
    let mut grand_by_verdict = [0usize; 6];

    for (seed_artist, seed_title) in SEEDS {
        println!("\n╔══════════════════════════════════════════════════════════");
        println!("║ Seed: {} — {}", seed_artist, seed_title);
        println!("╚══════════════════════════════════════════════════════════");

        let similar = match lastfm_similar(&http, &lastfm.api_key, seed_artist, seed_title, 50).await {
            Ok(v) => v,
            Err(e) => {
                println!("  last.fm error: {e}");
                continue;
            }
        };

        if similar.is_empty() {
            println!("  (no similar tracks returned by last.fm)");
            continue;
        }

        println!("  last.fm returned {} similar tracks", similar.len());

        let mut results: Vec<MatchResult> = Vec::new();

        for track in &similar {
            let result = probe_one(&http, &tidal.access_token, &tidal.country_code, track).await;
            results.push(result);
            // Small delay to avoid hammering Tidal search
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        }

        // Per-seed summary table
        let seed_total = results.len();
        let seed_acceptable = results.iter().filter(|r| r.verdict.is_acceptable()).count();
        let mut by_verdict = [0usize; 6];
        for r in &results {
            let idx = verdict_idx(&r.verdict);
            by_verdict[idx] += 1;
            grand_by_verdict[idx] += 1;
        }
        grand_total += seed_total;
        grand_acceptable += seed_acceptable;

        println!(
            "\n  Results ({}/{} acceptable at ≥0.85 JW threshold):",
            seed_acceptable, seed_total
        );
        println!("  {:<28} {:<28} {:>6}  {:>5}  {:>5}  {}",
            "last.fm artist", "Tidal top result", "artist", "title", "lfm", "verdict");
        println!("  {}", "-".repeat(100));

        for r in &results {
            let tidal_artist = r.tidal_artist.as_deref().unwrap_or("—");
            let _tidal_title = r.tidal_title.as_deref().unwrap_or("—");
            let marker = if r.verdict.is_acceptable() { " " } else { "!" };
            println!(
                "{} {:<28} {:<28} {:>6.3}  {:>5.3}  {:>5.3}  {}",
                marker,
                truncate(&r.lastfm_artist, 27),
                truncate(tidal_artist, 27),
                r.artist_jw,
                r.title_jw,
                r.lastfm_match,
                r.verdict.label(),
            );
            if !r.verdict.is_acceptable() {
                // Print the full last.fm title and Tidal title for rejected entries
                println!(
                    "    last.fm: {:?} — {:?}",
                    r.lastfm_artist, r.lastfm_title
                );
                if let (Some(ta), Some(tt)) = (&r.tidal_artist, &r.tidal_title) {
                    println!("    Tidal:   {:?} — {:?}", ta, tt);
                }
            }
        }

        println!();
        println!("  Verdict breakdown:");
        let labels = ["EXACT", "HIGH>0.90", "PASS0.85-0.90", "BELOW0.70-0.85", "WEAK<0.70", "NO_RESULT"];
        for (i, label) in labels.iter().enumerate() {
            if by_verdict[i] > 0 {
                println!("    {:>15}: {}", label, by_verdict[i]);
            }
        }
    }

    // Grand summary
    println!("\n╔══════════════════════════════════════════════════════════");
    println!("║ GRAND TOTAL — {} seeds × up to 50 tracks = {} probed", SEEDS.len(), grand_total);
    println!("║ Acceptable at ≥0.85 JW: {}/{} ({:.1}%)",
        grand_acceptable, grand_total,
        if grand_total > 0 { grand_acceptable as f64 / grand_total as f64 * 100.0 } else { 0.0 });
    println!("╠══════════════════════════════════════════════════════════");
    let labels = ["EXACT", "HIGH >0.90", "PASS 0.85–0.90", "BELOW 0.70–0.85", "WEAK <0.70", "NO_RESULT"];
    for (i, label) in labels.iter().enumerate() {
        println!("║ {:>18}: {:>4}  ({:.1}%)", label, grand_by_verdict[i],
            if grand_total > 0 { grand_by_verdict[i] as f64 / grand_total as f64 * 100.0 } else { 0.0 });
    }
    println!("╚══════════════════════════════════════════════════════════");

    let pass_plus = grand_by_verdict[0] + grand_by_verdict[1] + grand_by_verdict[2];
    let reject = grand_by_verdict[3] + grand_by_verdict[4];
    if grand_total > 0 {
        println!();
        println!("Interpretation:");
        println!("  Tracks that PASS the 0.85 threshold: {}/{} ({:.1}%)", pass_plus, grand_total,
            pass_plus as f64 / grand_total as f64 * 100.0);
        println!("  Tracks that would be REJECTED/skipped: {}/{} ({:.1}%)", reject, grand_total,
            reject as f64 / grand_total as f64 * 100.0);
        println!("  Tracks with no Tidal result: {}", grand_by_verdict[5]);
        println!();
        if pass_plus as f64 / grand_total as f64 > 0.90 {
            println!("  ✓  0.85 threshold looks CONSERVATIVE — high pass rate, few false negatives.");
        } else if pass_plus as f64 / (grand_total as f64) < 0.70 {
            println!("  ⚠  0.85 threshold may be TOO STRICT — consider lowering to 0.80.");
        } else {
            println!("  ~  0.85 threshold is in a reasonable range. Review the BELOW entries.");
        }
    }

    Ok(())
}

async fn lastfm_similar(
    http: &reqwest::Client,
    api_key: &str,
    artist: &str,
    title: &str,
    limit: usize,
) -> Result<Vec<SimilarTrack>> {
    let url = reqwest::Url::parse_with_params(
        LASTFM_API_URL,
        &[
            ("method", "track.getsimilar"),
            ("artist", artist),
            ("track", title),
            ("limit", &limit.to_string()),
            ("api_key", api_key),
            ("format", "json"),
        ],
    )?;
    let body: Value = http
        .get(url)
        .send()
        .await?
        .json()
        .await
        .context("last.fm response not JSON")?;

    let items = body
        .get("similartracks")
        .and_then(|v| v.get("track"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for entry in items.iter().take(limit) {
        let title = entry.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let artist = entry
            .get("artist")
            .and_then(|a| a.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() || artist.is_empty() {
            continue;
        }
        let match_score = entry
            .get("match")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| entry.get("match").and_then(Value::as_f64))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        out.push(SimilarTrack { artist, title, match_score });
    }
    Ok(out)
}

async fn tidal_search_top(
    http: &reqwest::Client,
    access_token: &str,
    country_code: &str,
    artist: &str,
    title: &str,
) -> Result<Option<(String, String, i64)>> {
    // artist + title as a single query string — same as what the production resolver will use
    let query = format!("{} {}", artist, title);
    let url = reqwest::Url::parse_with_params(
        &format!("{}/search", TIDAL_API_URL),
        &[
            ("query", query.as_str()),
            ("types", "TRACKS"),
            ("countryCode", country_code),
            ("limit", "3"),
        ],
    )?;
    let resp = http
        .get(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept-Language", "en-US")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let body: Value = resp.json().await.context("Tidal search response not JSON")?;
    let items = body
        .get("tracks")
        .and_then(|t| t.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let first = match items.into_iter().next() {
        Some(v) => v,
        None => return Ok(None),
    };
    let obj = match first.as_object() {
        Some(o) => o,
        None => return Ok(None),
    };
    let tidal_title = match obj.get("title").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => return Ok(None),
    };
    let duration = obj.get("duration").and_then(Value::as_i64).unwrap_or(0);
    let primary_artist = obj.get("artist").or_else(|| {
        obj.get("artists")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
    });
    let tidal_artist = primary_artist
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    if tidal_artist.is_empty() {
        return Ok(None);
    }
    Ok(Some((tidal_artist, tidal_title, duration)))
}

async fn probe_one(
    http: &reqwest::Client,
    access_token: &str,
    country_code: &str,
    track: &SimilarTrack,
) -> MatchResult {
    match tidal_search_top(http, access_token, country_code, &track.artist, &track.title).await {
        Ok(Some((tidal_artist, tidal_title, _duration))) => {
            let artist_jw = strsim::jaro_winkler(
                &normalize(&track.artist),
                &normalize(&tidal_artist),
            );
            let title_jw = strsim::jaro_winkler(
                &normalize(&track.title),
                &normalize(&tidal_title),
            );
            let verdict = classify(artist_jw, &track.artist, &tidal_artist);
            MatchResult {
                lastfm_artist: track.artist.clone(),
                lastfm_title: track.title.clone(),
                lastfm_match: track.match_score,
                tidal_artist: Some(tidal_artist),
                tidal_title: Some(tidal_title),
                artist_jw,
                title_jw,
                verdict,
            }
        }
        Ok(None) | Err(_) => MatchResult {
            lastfm_artist: track.artist.clone(),
            lastfm_title: track.title.clone(),
            lastfm_match: track.match_score,
            tidal_artist: None,
            tidal_title: None,
            artist_jw: 0.0,
            title_jw: 0.0,
            verdict: Verdict::NoResult,
        },
    }
}

fn normalize(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn classify(artist_jw: f64, lfm_artist: &str, tidal_artist: &str) -> Verdict {
    if normalize(lfm_artist) == normalize(tidal_artist) {
        return Verdict::ExactArtist;
    }
    if artist_jw > 0.90 {
        Verdict::HighJW
    } else if artist_jw >= 0.85 {
        Verdict::PassThreshold
    } else if artist_jw >= 0.70 {
        Verdict::BelowThreshold
    } else {
        Verdict::Weak
    }
}

fn verdict_idx(v: &Verdict) -> usize {
    match v {
        Verdict::ExactArtist => 0,
        Verdict::HighJW => 1,
        Verdict::PassThreshold => 2,
        Verdict::BelowThreshold => 3,
        Verdict::Weak => 4,
        Verdict::NoResult => 5,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}
