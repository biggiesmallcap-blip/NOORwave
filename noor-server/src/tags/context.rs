use crate::genre::mappings::normalize_key;

const HARD_NOISE_TAGS: &[&str] = &[
    "classical",
    "instrumental",
    "live",
    "cover",
    "remix",
    "acoustic",
    "unplugged",
    "male vocalists",
    "female vocalists",
    "male vocalist",
    "female vocalist",
];

const SOFT_NOISE_TAGS: &[&str] = &[
    "seen live",
    "seen-live",
    "favorite",
    "favorites",
    "favourite",
    "favourites",
    "owned",
    "albums i own",
    "spotify",
    "youtube",
    "soundcloud",
    "mp3",
    "download",
    "streaming",
    "playlist",
    "albums",
    "artist",
    "band",
    "classic",
    "classics",
    "amazing",
    "awesome",
    "cool",
    "good",
    "great",
    "best",
    "epic",
    "legendary",
    "underrated",
    "overrated",
];

const MOOD_TAGS: &[&str] = &[
    "happy",
    "sad",
    "angry",
    "melancholy",
    "melancholic",
    "emotional",
    "depressing",
    "uplifting",
    "euphoric",
    "romantic",
    "sexy",
    "dark",
    "beautiful",
    "bittersweet",
    "nostalgic",
    "dreamy",
];

const ENERGY_TAGS: &[&str] = &[
    "chill",
    "chilled",
    "relaxing",
    "relaxed",
    "mellow",
    "calm",
    "peaceful",
    "energetic",
    "upbeat",
    "hype",
    "intense",
    "aggressive",
    "hard",
    "soft",
    "heavy",
];

const OCCASION_TAGS: &[&str] = &[
    "birthday",
    "party",
    "wedding",
    "christmas",
    "halloween",
    "summer",
    "winter",
    "rainy day",
    "road trip",
    "workout",
    "studying",
    "study",
    "sleep",
    "sex",
    "club",
    "festival",
];

const TIME_OF_DAY_TAGS: &[&str] = &[
    "night",
    "night music",
    "late night",
    "midnight",
    "morning",
    "sunrise",
    "sunset",
    "afternoon",
    "evening",
];

const ACTIVITY_TAGS: &[&str] = &[
    "driving",
    "running",
    "dancing",
    "dance",
    "study music",
    "focus",
    "gaming",
    "walking",
    "cooking",
    "cleaning",
];

const PSYCHEDELIC_TAGS: &[&str] = &[
    "tripping",
    "trip",
    "psychedelic",
    "stoned",
    "high",
    "druggy",
    "spacey",
    "cosmic",
];

const TEMPO_TAGS: &[&str] = &[
    "fast",
    "fast bpm",
    "slow",
    "slow bpm",
    "mid tempo",
    "midtempo",
    "uptempo",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagContext {
    Genre,
    Mood,
    Energy,
    Occasion,
    TimeOfDay,
    Activity,
    Psychedelic,
    Tempo,
    Era,
    Noise,
}

impl TagContext {
    pub fn as_str(self) -> &'static str {
        match self {
            TagContext::Genre => "genre",
            TagContext::Mood => "mood",
            TagContext::Energy => "energy",
            TagContext::Occasion => "occasion",
            TagContext::TimeOfDay => "time_of_day",
            TagContext::Activity => "activity",
            TagContext::Psychedelic => "psychedelic",
            TagContext::Tempo => "tempo",
            TagContext::Era => "era",
            TagContext::Noise => "noise",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassifiedTag {
    pub raw: String,
    pub normalized: String,
    pub context: TagContext,
}

fn in_list(list: &[&str], tag: &str) -> bool {
    list.iter().any(|&item| item == tag)
}

fn looks_like_era(tag: &str) -> bool {
    matches!(
        tag,
        "60s"
            | "70s"
            | "80s"
            | "90s"
            | "00s"
            | "10s"
            | "20s"
            | "1960s"
            | "1970s"
            | "1980s"
            | "1990s"
            | "2000s"
            | "2010s"
            | "2020s"
    )
}

pub fn classify_tag_context(raw: &str, is_known_genre: bool) -> ClassifiedTag {
    let normalized = normalize_key(raw);
    let context = if normalized.is_empty() || normalized.len() > 50 {
        TagContext::Noise
    } else if in_list(HARD_NOISE_TAGS, &normalized) {
        TagContext::Noise
    } else if is_known_genre {
        TagContext::Genre
    } else if in_list(SOFT_NOISE_TAGS, &normalized) {
        TagContext::Noise
    } else if looks_like_era(&normalized) {
        TagContext::Era
    } else if in_list(MOOD_TAGS, &normalized) {
        TagContext::Mood
    } else if in_list(ENERGY_TAGS, &normalized) {
        TagContext::Energy
    } else if in_list(OCCASION_TAGS, &normalized) {
        TagContext::Occasion
    } else if in_list(TIME_OF_DAY_TAGS, &normalized) {
        TagContext::TimeOfDay
    } else if in_list(ACTIVITY_TAGS, &normalized) {
        TagContext::Activity
    } else if in_list(PSYCHEDELIC_TAGS, &normalized) {
        TagContext::Psychedelic
    } else if in_list(TEMPO_TAGS, &normalized) {
        TagContext::Tempo
    } else {
        TagContext::Noise
    };

    ClassifiedTag {
        raw: raw.to_string(),
        normalized,
        context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mood_tags_classified_correctly() {
        assert_eq!(
            classify_tag_context("happy", false).context,
            TagContext::Mood
        );
        assert_eq!(classify_tag_context("sad", false).context, TagContext::Mood);
        assert_eq!(
            classify_tag_context("angry", false).context,
            TagContext::Mood
        );
        assert_eq!(
            classify_tag_context("nostalgic", false).context,
            TagContext::Mood
        );
    }

    #[test]
    fn energy_tags_classified_correctly() {
        assert_eq!(
            classify_tag_context("chill", false).context,
            TagContext::Energy
        );
        assert_eq!(
            classify_tag_context("mellow", false).context,
            TagContext::Energy
        );
        assert_eq!(
            classify_tag_context("energetic", false).context,
            TagContext::Energy
        );
    }

    #[test]
    fn occasion_and_time_tags() {
        assert_eq!(
            classify_tag_context("birthday", false).context,
            TagContext::Occasion
        );
        assert_eq!(
            classify_tag_context("night music", false).context,
            TagContext::TimeOfDay
        );
        assert_eq!(
            classify_tag_context("late night", false).context,
            TagContext::TimeOfDay
        );
    }

    #[test]
    fn psychedelic_and_tempo() {
        assert_eq!(
            classify_tag_context("tripping", false).context,
            TagContext::Psychedelic
        );
        assert_eq!(
            classify_tag_context("fast bpm", false).context,
            TagContext::Tempo
        );
    }

    #[test]
    fn known_genre_beats_vague_match() {
        assert_eq!(
            classify_tag_context("dark", false).context,
            TagContext::Mood
        );
        assert_eq!(
            classify_tag_context("dark ambient", true).context,
            TagContext::Genre
        );
    }

    #[test]
    fn psychedelic_rock_as_genre_vs_bare_psychedelic() {
        assert_eq!(
            classify_tag_context("psychedelic", false).context,
            TagContext::Psychedelic
        );
        assert_eq!(
            classify_tag_context("psychedelic rock", true).context,
            TagContext::Genre
        );
    }

    #[test]
    fn noise_tags_discarded() {
        assert_eq!(
            classify_tag_context("seen live", false).context,
            TagContext::Noise
        );
        assert_eq!(
            classify_tag_context("spotify", false).context,
            TagContext::Noise
        );
        assert_eq!(
            classify_tag_context("classical", false).context,
            TagContext::Noise
        );
    }

    #[test]
    fn era_tags_classified() {
        assert_eq!(classify_tag_context("90s", false).context, TagContext::Era);
        assert_eq!(
            classify_tag_context("1980s", false).context,
            TagContext::Era
        );
    }

    #[test]
    fn known_genre_wins_before_context_lists() {
        assert_eq!(
            classify_tag_context("hard", true).context,
            TagContext::Genre
        );
        assert_eq!(
            classify_tag_context("chillout", true).context,
            TagContext::Genre
        );
    }

    #[test]
    fn vague_terms_remain_context_when_not_known_genres() {
        assert_eq!(
            classify_tag_context("hard", false).context,
            TagContext::Energy
        );
        assert_eq!(
            classify_tag_context("chill", false).context,
            TagContext::Energy
        );
    }

    #[test]
    fn hard_noise_beats_known_genre_status() {
        assert_eq!(
            classify_tag_context("classical", true).context,
            TagContext::Noise
        );
        assert_eq!(
            classify_tag_context("instrumental", true).context,
            TagContext::Noise
        );
        assert_eq!(
            classify_tag_context("live", true).context,
            TagContext::Noise
        );
    }
}
