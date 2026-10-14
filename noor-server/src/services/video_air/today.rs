//! Today: three short pieces under the dial, one artist each, written with
//! fixed sentences from what is known, so everything they say is true: the
//! catalog (videos, years, cuts, concerts, collaborators, where they sit on
//! the map), the listener's own history, a biography (stored, TIDAL, or
//! Last.fm) and origin from Wikidata. No generated prose.
//!
//! - today: the day's Spotlight artist.
//! - new: someone right next to what the listener watches, never watched.
//! - archive: someone watched before and not for weeks; with no such history
//!   yet, elsewhere: a leading artist of a scene the listener rarely visits.
//!
//! This module is the pure part (the writing, the bio excerpt, Wikidata's
//! answer); the route gathers the facts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Today,
    New,
    Archive,
    Elsewhere,
}

impl Kind {
    pub fn kicker(self) -> &'static str {
        match self {
            Kind::Today => "Today's artist",
            Kind::New => "Someone new",
            Kind::Archive => "From your archive",
            Kind::Elsewhere => "Elsewhere on the map",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginKind {
    /// A group: where and when it formed.
    Formed,
    /// A person: where and when they were born.
    Born,
    /// Only when they started making music.
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub kind: OriginKind,
    pub place: Option<String>,
    pub year: Option<i32>,
}

/// What the catalog and the listener's history say about one artist.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub name: String,
    pub videos: i64,
    pub hours: f64,
    /// Earliest and latest year among songs whose year is known with
    /// confidence.
    pub first_year: Option<i32>,
    pub last_year: Option<i32>,
    /// The decade most of those songs are from, when it is most of them.
    pub peak_decade: Option<i32>,
    pub live: i64,
    pub sessions: i64,
    pub concerts: i64,
    pub collabs: Vec<String>,
    pub scene: Option<String>,
    pub neighbours: Vec<String>,
    pub plays: i64,
    pub days_since: Option<i64>,
    /// The watched artist a new one sits next to.
    pub via: Option<String>,
    pub origin: Option<Origin>,
}

/// "A", "A and B", "A, B and C".
pub fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "once", "twice", "5 times".
pub fn times(n: i64) -> String {
    match n {
        1 => "once".to_string(),
        2 => "twice".to_string(),
        _ => format!("{n} times"),
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "last week", "3 weeks ago", "2 months ago", "over a year ago".
pub fn ago(days: i64) -> String {
    match days {
        ..=1 => "yesterday".to_string(),
        2..=6 => format!("{days} days ago"),
        7..=13 => "last week".to_string(),
        14..=55 => format!("{} weeks ago", days / 7),
        56..=364 => format!("{} months ago", (days + 15) / 30),
        _ => "over a year ago".to_string(),
    }
}

fn origin_sentence(origin: &Origin) -> Option<String> {
    let place = origin.place.as_deref().filter(|p| !p.trim().is_empty());
    let year = origin.year;
    let sentence = match (origin.kind, place, year) {
        (OriginKind::Formed, Some(p), Some(y)) => format!("Formed in {p} in {y}."),
        (OriginKind::Formed, Some(p), None) => format!("Formed in {p}."),
        (OriginKind::Formed, None, Some(y)) => format!("Formed in {y}."),
        (OriginKind::Born, Some(p), Some(y)) => format!("Born in {p} in {y}."),
        (OriginKind::Born, Some(p), None) => format!("Born in {p}."),
        (OriginKind::Born, None, Some(y)) => format!("Born in {y}."),
        (OriginKind::Active, _, Some(y)) => format!("Making music since {y}."),
        _ => return None,
    };
    Some(sentence)
}

/// The piece's paragraphs: why this artist today, then what is here, then
/// who they are near. Only sentences whose facts are known.
pub fn write(kind: Kind, facts: &Facts) -> Vec<String> {
    let mut paragraphs = Vec::new();

    let mut lead = match kind {
        Kind::Today => "On Spotlight all day.".to_string(),
        Kind::New => match &facts.via {
            Some(via) => format!("Right next to {via}, and new to you."),
            None => "Right next to what you watch, and new to you.".to_string(),
        },
        Kind::Archive => match facts.days_since {
            Some(days) => format!(
                "You watched them {}, the last {}.",
                times(facts.plays),
                ago(days)
            ),
            None => format!("You watched them {}.", times(facts.plays)),
        },
        Kind::Elsewhere => match &facts.scene {
            Some(scene) => format!("A corner you rarely visit: {scene}."),
            None => "A corner of the map you rarely visit.".to_string(),
        },
    };
    if let Some(sentence) = facts.origin.as_ref().and_then(origin_sentence) {
        lead.push(' ');
        lead.push_str(&sentence);
    }
    paragraphs.push(lead);

    let mut here = Vec::new();
    if facts.videos > 0 {
        let span = match (facts.first_year, facts.last_year) {
            (Some(a), Some(b)) if b > a => format!(", from {a} to {b}"),
            (Some(a), _) => format!(", from {a}"),
            _ => String::new(),
        };
        here.push(format!(
            "{} here{span}.",
            plural(facts.videos, "video", "videos")
        ));
    }
    if let Some(decade) = facts.peak_decade {
        here.push(format!("Most are from the {decade}s."));
    }
    let mut stage = Vec::new();
    if facts.live > 0 {
        let videos = if facts.live == 1 { "video" } else { "videos" };
        stage.push(format!("{} live {videos}", facts.live));
    }
    if facts.sessions > 0 {
        stage.push(plural(facts.sessions, "session", "sessions"));
    }
    if facts.concerts > 0 {
        stage.push(plural(facts.concerts, "whole concert", "whole concerts"));
    }
    if !stage.is_empty() {
        here.push(format!("On stage: {}.", join_and(&stage)));
    }
    if !here.is_empty() {
        paragraphs.push(here.join(" "));
    }

    let mut near = Vec::new();
    if !facts.collabs.is_empty() {
        near.push(format!("On video with {}.", join_and(&facts.collabs)));
    }
    if kind != Kind::Elsewhere
        && let Some(scene) = &facts.scene
    {
        if facts.neighbours.is_empty() {
            near.push(format!("On the map they sit in {scene}."));
        } else {
            near.push(format!(
                "On the map they sit in {scene}, near {}.",
                join_and(&facts.neighbours)
            ));
        }
    } else if !facts.neighbours.is_empty() {
        near.push(format!("Near {}.", join_and(&facts.neighbours)));
    }
    if !near.is_empty() {
        paragraphs.push(near.join(" "));
    }
    paragraphs
}

/// The decade most of these years fall in, when that is at least `share`
/// of them and there are at least `min` of them.
pub fn peak_decade(years: &[i32], min: usize, share: f64) -> Option<i32> {
    if years.len() < min {
        return None;
    }
    let mut counts: Vec<(i32, usize)> = Vec::new();
    for year in years {
        let decade = year - year.rem_euclid(10);
        match counts.iter_mut().find(|(d, _)| *d == decade) {
            Some((_, n)) => *n += 1,
            None => counts.push((decade, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let (decade, n) = counts[0];
    (n as f64 / years.len() as f64 >= share).then_some(decade)
}

const BIO_MAX_CHARS: usize = 420;

/// A biography as plain prose: TIDAL's [wimpLink] markup and Last.fm's HTML
/// and "Read more" link removed, whitespace collapsed.
pub fn clean_bio(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // [wimpLink artistId="1"]Name[/wimpLink] keeps "Name".
            '[' => {
                let mut tag = String::new();
                for next in chars.by_ref() {
                    if next == ']' {
                        break;
                    }
                    tag.push(next);
                }
                let lower = tag.to_lowercase();
                if !(lower.starts_with("wimplink") || lower.starts_with("/wimplink")) {
                    text.push('[');
                    text.push_str(&tag);
                    text.push(']');
                }
            }
            '<' => {
                for next in chars.by_ref() {
                    if next == '>' {
                        break;
                    }
                }
            }
            _ => text.push(c),
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let text = match text.find("Read more on Last.fm") {
        Some(at) => text[..at].to_string(),
        None => text,
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first sentences of a biography, whole sentences only, up to about
/// `BIO_MAX_CHARS`. None when there is nothing worth showing.
pub fn bio_excerpt(raw: &str) -> Option<String> {
    let text = clean_bio(raw);
    if text.len() < 40 {
        return None;
    }
    // "There are several artists with this name: 1) ..." is about someone
    // else as often as not.
    let opening = text.chars().take(120).collect::<String>().to_lowercase();
    if opening.starts_with("there are")
        || opening.starts_with("there is more than one")
        || opening.contains("1)")
    {
        return None;
    }
    let mut out = String::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (at, ch) in text.char_indices() {
        let end = at + ch.len_utf8();
        let sentence_end =
            matches!(ch, '.' | '!' | '?') && (end == text.len() || bytes.get(end) == Some(&b' '));
        if !sentence_end {
            continue;
        }
        let sentence = text[start..end].trim();
        if !out.is_empty() && out.len() + sentence.len() + 1 > BIO_MAX_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(sentence);
        start = end;
        if out.len() >= BIO_MAX_CHARS / 2 {
            break;
        }
    }
    if out.is_empty() {
        // One long sentence: cut at a word and say so.
        let cut = text
            .char_indices()
            .take_while(|(at, _)| *at < BIO_MAX_CHARS)
            .filter(|(_, c)| *c == ' ')
            .map(|(at, _)| at)
            .last()?;
        out = format!("{}...", text[..cut].trim_end_matches([',', ';', ':']));
    }
    Some(out)
}

/// The SPARQL that finds one music artist by exact English name (a
/// MusicBrainz artist id marks it as one) with where and when they began.
pub fn wikidata_query(name: &str) -> String {
    let literal = name.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "SELECT ?item ?inception ?birth ?start ?formedLabel ?birthplaceLabel WHERE {{ \
           ?item rdfs:label \"{literal}\"@en ; wdt:P434 ?mb . \
           OPTIONAL {{ ?item wdt:P571 ?inception . }} \
           OPTIONAL {{ ?item wdt:P569 ?birth . }} \
           OPTIONAL {{ ?item wdt:P2031 ?start . }} \
           OPTIONAL {{ ?item wdt:P740 ?formed . }} \
           OPTIONAL {{ ?item wdt:P19 ?birthplace . }} \
           SERVICE wikibase:label {{ bd:serviceParam wikibase:language \"en\". }} \
         }} LIMIT 8"
    )
}

fn year_of(value: Option<&str>) -> Option<i32> {
    let value = value?.trim_start_matches('+');
    value.get(..4)?.parse().ok()
}

/// Wikidata's answer as an origin: none when no artist matched, or when the
/// name matched more than one (better nothing than someone else's facts).
/// A birth makes it a person, an inception a group, else when they started.
pub fn parse_wikidata(json: &serde_json::Value) -> Option<Origin> {
    let rows = json.pointer("/results/bindings")?.as_array()?;
    let value = |row: &serde_json::Value, key: &str| -> Option<String> {
        row.get(key)?.get("value")?.as_str().map(str::to_string)
    };
    let mut items: Vec<String> = rows.iter().filter_map(|row| value(row, "item")).collect();
    items.sort();
    items.dedup();
    if items.len() != 1 {
        return None;
    }
    let first = |key: &str| rows.iter().find_map(|row| value(row, key));
    // Unlabelled places come back as their Q id: no place then.
    let place = |key: &str| {
        first(key).filter(|p| !(p.starts_with('Q') && p[1..].chars().all(|c| c.is_ascii_digit())))
    };
    if let Some(birth) = first("birth") {
        return Some(Origin {
            kind: OriginKind::Born,
            place: place("birthplaceLabel"),
            year: year_of(Some(&birth)),
        });
    }
    if let Some(inception) = first("inception") {
        return Some(Origin {
            kind: OriginKind::Formed,
            place: place("formedLabel"),
            year: year_of(Some(&inception)),
        });
    }
    first("start").map(|start| Origin {
        kind: OriginKind::Active,
        place: None,
        year: year_of(Some(&start)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts() -> Facts {
        Facts {
            name: "Dire Straits".into(),
            videos: 42,
            hours: 3.2,
            first_year: Some(1978),
            last_year: Some(1991),
            peak_decade: Some(1980),
            live: 12,
            sessions: 1,
            concerts: 2,
            collabs: vec!["Sting".into()],
            scene: Some("Classic Rock".into()),
            neighbours: vec!["Eric Clapton".into(), "Toto".into()],
            plays: 5,
            days_since: Some(40),
            via: Some("Toto".into()),
            origin: Some(Origin {
                kind: OriginKind::Formed,
                place: Some("London".into()),
                year: Some(1977),
            }),
        }
    }

    #[test]
    fn a_piece_says_only_what_is_known() {
        let today = write(Kind::Today, &facts());
        assert_eq!(
            today,
            vec![
                "On Spotlight all day. Formed in London in 1977.",
                "42 videos here, from 1978 to 1991. Most are from the 1980s. On stage: 12 live videos, 1 session and 2 whole concerts.",
                "On video with Sting. On the map they sit in Classic Rock, near Eric Clapton and Toto.",
            ]
        );
        let bare = Facts {
            name: "X".into(),
            videos: 1,
            ..Facts::default()
        };
        assert_eq!(
            write(Kind::New, &bare),
            vec![
                "Right next to what you watch, and new to you.",
                "1 video here."
            ]
        );
    }

    #[test]
    fn each_kind_leads_with_why_it_is_here() {
        let f = facts();
        assert_eq!(
            write(Kind::New, &f)[0],
            "Right next to Toto, and new to you. Formed in London in 1977."
        );
        assert_eq!(
            write(Kind::Archive, &f)[0],
            "You watched them 5 times, the last 5 weeks ago. Formed in London in 1977."
        );
        let elsewhere = write(Kind::Elsewhere, &f);
        assert_eq!(
            elsewhere[0],
            "A corner you rarely visit: Classic Rock. Formed in London in 1977."
        );
        assert_eq!(
            elsewhere[2],
            "On video with Sting. Near Eric Clapton and Toto."
        );
    }

    #[test]
    fn small_words_read_right() {
        assert_eq!(
            join_and(&["A".into(), "B".into(), "C".into()]),
            "A, B and C"
        );
        assert_eq!(ago(1), "yesterday");
        assert_eq!(ago(9), "last week");
        assert_eq!(ago(30), "4 weeks ago");
        assert_eq!(ago(100), "3 months ago");
        assert_eq!(ago(400), "over a year ago");
        let born = Origin {
            kind: OriginKind::Born,
            place: Some("Toronto".into()),
            year: None,
        };
        assert_eq!(origin_sentence(&born).unwrap(), "Born in Toronto.");
        let active = Origin {
            kind: OriginKind::Active,
            place: None,
            year: Some(2004),
        };
        assert_eq!(
            origin_sentence(&active).unwrap(),
            "Making music since 2004."
        );
    }

    #[test]
    fn the_peak_decade_needs_most_of_enough_songs() {
        assert_eq!(peak_decade(&[1981, 1984, 1988, 1992], 4, 0.5), Some(1980));
        assert_eq!(peak_decade(&[1981, 1992, 2001, 2012], 4, 0.5), None);
        assert_eq!(peak_decade(&[1981, 1984], 4, 0.5), None);
    }

    #[test]
    fn biographies_lose_their_markup_and_keep_whole_sentences() {
        let tidal = "Formed by [wimpLink artistId=\"123\"]Mark Knopfler[/wimpLink] in 1977, the band rose fast. \
                     Their debut sold millions. They toured for years after that. And more.";
        let excerpt = bio_excerpt(tidal).unwrap();
        assert!(excerpt.starts_with("Formed by Mark Knopfler in 1977, the band rose fast."));
        assert!(!excerpt.contains("wimpLink"));
        assert!(excerpt.ends_with('.'));
        let lastfm = "A band from London &amp; beyond. <a href=\"https://www.last.fm/music/X\">Read more on Last.fm</a>";
        assert_eq!(clean_bio(lastfm), "A band from London & beyond.");
        assert_eq!(bio_excerpt("Too short."), None);
        assert_eq!(
            bio_excerpt(
                "There are multiple artists with this name: 1) A rock band from Ohio. 2) A DJ."
            ),
            None,
            "a disambiguation page is not their biography"
        );
    }

    #[test]
    fn wikidata_answers_one_artist_or_none() {
        let band = json!({ "results": { "bindings": [
            { "item": { "value": "http://www.wikidata.org/entity/Q50033" },
              "inception": { "value": "1977-01-01T00:00:00Z" },
              "formedLabel": { "value": "London" } }
        ] } });
        assert_eq!(
            parse_wikidata(&band),
            Some(Origin {
                kind: OriginKind::Formed,
                place: Some("London".into()),
                year: Some(1977)
            })
        );
        let person = json!({ "results": { "bindings": [
            { "item": { "value": "Q1" }, "birth": { "value": "1985-03-02T00:00:00Z" }, "birthplaceLabel": { "value": "Q999" } }
        ] } });
        assert_eq!(
            parse_wikidata(&person),
            Some(Origin {
                kind: OriginKind::Born,
                place: None,
                year: Some(1985)
            }),
            "an unlabelled place is no place"
        );
        let two = json!({ "results": { "bindings": [
            { "item": { "value": "Q1" }, "inception": { "value": "1990-01-01T00:00:00Z" } },
            { "item": { "value": "Q2" }, "inception": { "value": "2001-01-01T00:00:00Z" } }
        ] } });
        assert_eq!(
            parse_wikidata(&two),
            None,
            "two artists by that name: say nothing"
        );
        assert_eq!(
            parse_wikidata(&json!({ "results": { "bindings": [] } })),
            None
        );
        assert!(wikidata_query("Guns N' \"Roses\"").contains("\\\"Roses\\\""));
    }
}
