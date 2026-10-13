use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Debug, Clone)]
pub enum OutputDeviceSelection {
    Default,
    Named(String),
}

impl OutputDeviceSelection {
    pub fn from_pref(pref: Option<&str>) -> Self {
        match pref {
            None => Self::Default,
            Some("default") => Self::Default,
            Some(name) => Self::Named(name.to_string()),
        }
    }
}

pub(super) fn device_display_name(device: &cpal::Device) -> String {
    device_display_name_opt(device).unwrap_or_else(|| "default output device".to_string())
}

fn device_display_name_opt(device: &cpal::Device) -> Option<String> {
    let description = device.description().ok()?;
    // CPAL 0.18 prefers FriendlyName. Existing preferences used DeviceDesc;
    // preserve that alias while new selections use an unambiguous endpoint ID.
    #[cfg(target_os = "windows")]
    if let Ok(id) = device.id()
        && let Ok(enumerator) = wasapi::DeviceEnumerator::new()
        && let Ok(endpoint) = enumerator.get_device(id.id())
        && let Ok(name) = endpoint.get_description()
        && !name.is_empty()
    {
        return Some(name);
    }
    Some(description.name().to_string())
}

fn named_device_index(pref: &str, candidates: &[(String, String)]) -> Option<usize> {
    let exact = candidates.iter().position(|(id, _)| id == pref);
    if exact.is_some() {
        return exact;
    }
    let mut matches = candidates
        .iter()
        .enumerate()
        .filter(|(_, (_, name))| name == pref);
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

pub(super) fn resolve_device(selection: &OutputDeviceSelection) -> Option<cpal::Device> {
    let host = cpal::default_host();
    match selection {
        OutputDeviceSelection::Default => host.default_output_device(),
        OutputDeviceSelection::Named(pref) => {
            let candidates: Vec<_> = host
                .output_devices()
                .ok()?
                .filter_map(|device| {
                    let id = device.id().ok()?.to_string();
                    let name = device_display_name_opt(&device)?;
                    Some((device, (id, name)))
                })
                .collect();
            let identities: Vec<_> = candidates
                .iter()
                .map(|(_, identity)| identity.clone())
                .collect();
            let index = named_device_index(pref, &identities)?;
            candidates.into_iter().nth(index).map(|(device, _)| device)
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub max_channels: u16,
    pub supported_sample_rates: Vec<u32>,
}

pub fn enumerate_output_devices() -> Vec<OutputDeviceInfo> {
    let host = cpal::default_host();
    let default_id = host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());

    host.output_devices()
        .map(|iter| {
            iter.filter_map(|dev| {
                let name = device_display_name_opt(&dev)?;
                let id = dev.id().ok()?.to_string();
                // List every endpoint resolve_device considers, even one whose
                // formats cannot be read right now; otherwise a saved name can
                // look unique here while the server finds it ambiguous.
                let configs: Vec<_> = dev
                    .supported_output_configs()
                    .map(|configs| configs.collect())
                    .unwrap_or_default();
                let max_channels = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
                let mut rates: Vec<u32> = configs
                    .iter()
                    .flat_map(|c| {
                        let min = c.min_sample_rate();
                        let max = c.max_sample_rate();
                        // Common audio rates that fall within the supported range.
                        [44_100, 48_000, 88_200, 96_000, 176_400, 192_000]
                            .into_iter()
                            .filter(move |r| *r >= min && *r <= max)
                    })
                    .collect();
                rates.sort_unstable();
                rates.dedup();
                Some(OutputDeviceInfo {
                    is_default: default_id.as_deref() == Some(id.as_str()),
                    id,
                    name,
                    max_channels,
                    supported_sample_rates: rates,
                })
            })
            .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_missing_output_never_uses_default() {
        assert!(
            resolve_device(&OutputDeviceSelection::Named(
                "noor-missing-output-65a7a9e1".to_owned()
            ))
            .is_none(),
            "missing explicit endpoint resolved to the default output"
        );
    }

    #[test]
    fn stable_id_selects_one_of_two_identically_named_devices() {
        let candidates = vec![
            ("Wasapi:a".into(), "Speakers".into()),
            ("Wasapi:b".into(), "Speakers".into()),
        ];
        assert_eq!(named_device_index("Wasapi:b", &candidates), Some(1));
        assert_eq!(named_device_index("Speakers", &candidates), None);
    }

    #[test]
    fn unique_legacy_name_is_accepted_but_missing_name_is_rejected() {
        let candidates = vec![("Wasapi:a".into(), "Realtek Digital Output".into())];
        assert_eq!(
            named_device_index("Realtek Digital Output", &candidates),
            Some(0)
        );
        assert_eq!(named_device_index("Disconnected DAC", &candidates), None);
    }
}
