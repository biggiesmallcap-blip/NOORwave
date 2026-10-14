//! Scene families: the dial groups its scene channels under a few named
//! families (Hip Hop and R&B, Latin and Caribbean, ...) read from each
//! scene's genre tags. The frontend owns the labels and the order; this
//! module only says which family a scene belongs to.

/// Families by key, with the tag words that vote for them. A tag votes once
/// per family when it is a word or contains one ("pop punk" votes rock and
/// pop); earlier tags weigh more, as the scene stage ranks them.
const FAMILIES: &[(&str, &[&str])] = &[
    (
        "hiphop",
        &[
            "hip hop",
            "rap",
            "trap",
            "r&b",
            "grime",
            "drill",
            "gangsta",
            "dirty south",
            "afrobeats",
            "crunk",
        ],
    ),
    (
        "latin",
        &[
            "latin",
            "reggaeton",
            "spanish",
            "salsa",
            "cumbia",
            "bachata",
            "merengue",
            "mexico",
            "mexican",
            "brazilian",
            "mpb",
            "samba",
            "bossa nova",
            "sertanejo",
            "pagode",
            "colombia",
            "puerto rico",
            "cuban",
            "reggae",
            "dancehall",
            "dub",
            "ska",
            "tango",
        ],
    ),
    (
        "rock",
        &[
            "rock",
            "metal",
            "punk",
            "grunge",
            "emo",
            "britpop",
            "alternative",
            "indie",
            "blues",
            "hardcore",
            "shoegaze",
        ],
    ),
    (
        "pop",
        &[
            "pop",
            "dance",
            "house",
            "electronic",
            "edm",
            "trance",
            "techno",
            "synthpop",
            "new wave",
            "drum and bass",
            "dubstep",
            "jungle",
            "eurodance",
            "electro",
        ],
    ),
    (
        "country",
        &[
            "country",
            "americana",
            "folk",
            "singer-songwriter",
            "acoustic",
            "honky tonk",
            "bluegrass",
        ],
    ),
    (
        "soul",
        &[
            "soul",
            "funk",
            "disco",
            "jazz",
            "swing",
            "classical",
            "opera",
            "piano",
            "soundtrack",
            "instrumental",
            "downtempo",
            "trip hop",
            "chillout",
            "oldies",
            "gospel",
            "lounge",
        ],
    ),
    (
        "world",
        &[
            "french",
            "chanson",
            "italian",
            "german",
            "indian",
            "bollywood",
            "finnish",
            "swedish",
            "norwegian",
            "danish",
            "dutch",
            "polish",
            "russian",
            "turkish",
            "arabic",
            "greek",
            "k-pop",
            "j-pop",
            "japanese",
            "korean",
            "african",
            "schlager",
        ],
    ),
];

/// Scenes no family claims.
pub const OTHER: &str = "other";

/// Whether `word` appears in `tag` as a whole word (or words).
fn has_word(tag: &str, word: &str) -> bool {
    tag.match_indices(word).any(|(at, _)| {
        let before = tag[..at].chars().next_back();
        let after = tag[at + word.len()..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        boundary(before) && boundary(after)
    })
}

/// The family of a scene from its tags, best first.
pub fn family_of(tags: &[String]) -> &'static str {
    let mut best = (0.0, OTHER);
    for (key, words) in FAMILIES {
        let score: f64 = tags
            .iter()
            .enumerate()
            .filter(|(_, tag)| {
                let tag = tag.to_lowercase();
                words.iter().any(|word| has_word(&tag, word))
            })
            .map(|(index, _)| 1.0 / (1.0 + index as f64 * 0.5))
            .sum();
        // Ties keep the family listed first.
        if score > best.0 {
            best = (score, key);
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(tags: &[&str]) -> &'static str {
        family_of(&tags.iter().map(|t| t.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn scenes_fall_into_their_families() {
        assert_eq!(of(&["grime", "hip hop", "rap", "soul"]), "hiphop");
        assert_eq!(of(&["reggae", "dancehall", "roots reggae", "dub"]), "latin");
        assert_eq!(of(&["brazilian", "trap", "funk", "dance"]), "latin");
        assert_eq!(of(&["punk rock", "pop punk", "punk", "emo"]), "rock");
        assert_eq!(of(&["new wave", "pop", "rock", "synthpop"]), "pop");
        assert_eq!(
            of(&["folk", "indie", "singer-songwriter", "indie rock"]),
            "country"
        );
        assert_eq!(
            of(&["electronic", "downtempo", "trip hop", "chillout"]),
            "soul"
        );
        assert_eq!(
            of(&["italian", "italian pop", "minimal", "minimal techno"]),
            "world"
        );
        assert_eq!(
            of(&[
                "gospel",
                "christian rock",
                "christian",
                "contemporary gospel"
            ]),
            "soul"
        );
    }

    #[test]
    fn words_match_whole_and_unclaimed_scenes_are_other() {
        assert!(has_word("trip hop", "trip hop"));
        assert!(!has_word("trip hop", "hip hop"), "not inside another word");
        assert!(has_word("modern country", "country"));
        assert!(!has_word("popular", "pop"));
        assert_eq!(of(&[]), OTHER);
        assert_eq!(of(&["vaporwave"]), OTHER);
    }
}
