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
    device
        .description()
        .ok()
        .map(|description| description.name().to_string())
}

pub(super) fn resolve_device(selection: &OutputDeviceSelection) -> Option<cpal::Device> {
    let host = cpal::default_host();
    match selection {
        OutputDeviceSelection::Default => host.default_output_device(),
        OutputDeviceSelection::Named(name) => host
            .output_devices()
            .ok()
            .and_then(|mut iter| {
                iter.find(|device| {
                    device_display_name_opt(device).as_deref() == Some(name.as_str())
                })
            })
            .or_else(|| host.default_output_device()),
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
    let default_name = host
        .default_output_device()
        .and_then(|device| device_display_name_opt(&device));

    host.output_devices()
        .map(|iter| {
            iter.filter_map(|dev| {
                let name = device_display_name_opt(&dev)?;
                let configs: Vec<_> = dev.supported_output_configs().ok()?.collect();
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
                    id: name.clone(),
                    name: name.clone(),
                    is_default: default_name.as_deref() == Some(name.as_str()),
                    max_channels,
                    supported_sample_rates: rates,
                })
            })
            .collect()
        })
        .unwrap_or_default()
}
