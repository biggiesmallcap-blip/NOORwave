pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

pub fn check() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION");
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("NOORwave")
        .build()
        .ok()?
        .get("https://api.github.com/repos/biggiesmallcap-blip/NOORwave/releases/latest")
        .send()
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().ok()?;
    let tag = body["tag_name"].as_str()?;
    let latest = tag.trim_start_matches('v');
    let url = body["html_url"].as_str()?.to_owned();

    if is_newer(latest, current) {
        Some(UpdateInfo { version: latest.to_owned(), url })
    } else {
        None
    }
}

fn is_newer(latest: &str, current: &str) -> bool {
    let to_parts = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|n| n.parse().ok()).collect()
    };
    to_parts(latest) > to_parts(current)
}
