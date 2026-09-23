pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub notes: Option<String>,
}

const RELEASES_API: &str = "https://api.github.com/repos/biggiesmallcap-blip/NOORwave/releases";

fn github_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("NOORwave")
        .build()
        .ok()
}

pub fn check() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION");
    let resp = github_client()?
        .get(format!("{RELEASES_API}/latest"))
        .send()
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().ok()?;
    let tag = body["tag_name"].as_str()?;
    let latest = tag.trim_start_matches('v');
    let url = body["html_url"].as_str()?.to_owned();
    let notes = normalize_release_notes(latest, body["body"].as_str());

    if is_newer(latest, current) {
        Some(UpdateInfo {
            version: latest.to_owned(),
            url,
            notes,
        })
    } else {
        None
    }
}

pub fn resolve_release_notes(version: &str, manifest_notes: Option<&str>) -> Option<String> {
    fetch_release_notes(version).or_else(|| normalize_release_notes(version, manifest_notes))
}

fn fetch_release_notes(version: &str) -> Option<String> {
    let tag = if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    };
    let resp = github_client()?
        .get(format!("{RELEASES_API}/tags/{tag}"))
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let release: serde_json::Value = resp.json().ok()?;
    normalize_release_notes(version, release["body"].as_str())
}

fn normalize_release_notes(version: &str, notes: Option<&str>) -> Option<String> {
    let notes = notes?.trim();
    if notes.is_empty() || notes.trim_start_matches('v') == version.trim_start_matches('v') {
        return None;
    }

    extract_section(
        notes,
        "## What's new in ",
        &["\nDesktop hi-fi player", "\n## Downloads"],
    )
    .or_else(|| {
        extract_section(
            notes,
            "## What's Changed",
            &["\n**Full Changelog**", "\n## Downloads"],
        )
    })
    .or_else(|| Some(notes.to_owned()))
}

fn extract_section(body: &str, heading: &str, end_markers: &[&str]) -> Option<String> {
    let heading_start = body.find(heading)?;
    let after_heading = &body[heading_start..];
    let content_start = after_heading.find('\n')? + 1;
    let content = &after_heading[content_start..];
    let content_end = end_markers
        .iter()
        .filter_map(|marker| content.find(marker))
        .min()
        .unwrap_or(content.len());
    let section = content[..content_end].trim();
    (!section.is_empty()).then(|| section.to_owned())
}

fn is_newer(latest: &str, current: &str) -> bool {
    let to_parts = |s: &str| -> Vec<u32> { s.split('.').filter_map(|n| n.parse().ok()).collect() };
    to_parts(latest) > to_parts(current)
}

#[cfg(test)]
mod tests {
    use super::normalize_release_notes;

    #[test]
    fn version_only_manifest_notes_are_not_treated_as_a_changelog() {
        assert_eq!(normalize_release_notes("0.15.9", Some("v0.15.9")), None);
        assert_eq!(normalize_release_notes("0.15.9", Some("0.15.9")), None);
    }

    #[test]
    fn extracts_the_curated_release_summary_without_download_boilerplate() {
        let body = r#"## What's new in v0.15.9

### Continuous Video Radio

- Start radio from the Videos page.

### Queue and playback

- The queue follows the current song.

Desktop hi-fi player for your TIDAL library.

## Downloads

| Platform | File |
|---|---|
| Windows | setup.exe |"#;

        let notes = normalize_release_notes("0.15.9", Some(body)).expect("release summary");
        assert!(notes.contains("Continuous Video Radio"));
        assert!(notes.contains("queue follows the current song"));
        assert!(!notes.contains("Desktop hi-fi player"));
        assert!(!notes.contains("Downloads"));
    }
}
