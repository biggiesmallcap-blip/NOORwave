use madmom_beats_port_core::{analyze, CoreConfig};
use serde::Deserialize;
use std::path::PathBuf;

const FRAME_TOLERANCE_SEC: f32 = 0.005;
const CONFIDENCE_TOLERANCE: f32 = 0.01;

#[derive(Debug, Deserialize)]
struct GoldenEvents {
    beats: Vec<GoldenBeat>,
}

#[derive(Debug, Deserialize)]
struct GoldenBeat {
    time_sec: f32,
    beat_in_bar: u32,
    confidence: f32,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
}

fn repo_root() -> PathBuf {
    fixtures_root().parent().unwrap().to_path_buf()
}

fn load_wav_mono(path: &PathBuf) -> (Vec<f32>, u32) {
    let mut reader = hound::WavReader::open(path).expect("failed to open wav");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 44_100, "unexpected sample rate");
    let channels = spec.channels as usize;
    let mut samples = Vec::new();

    match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            let mut frame = Vec::with_capacity(channels);
            for s in reader.samples::<i32>() {
                let v = s.expect("invalid sample") as f32 / max;
                frame.push(v);
                if frame.len() == channels {
                    let sum: f32 = frame.iter().sum();
                    samples.push(sum / channels as f32);
                    frame.clear();
                }
            }
        }
        hound::SampleFormat::Float => {
            let mut frame = Vec::with_capacity(channels);
            for s in reader.samples::<f32>() {
                let v = s.expect("invalid sample");
                frame.push(v);
                if frame.len() == channels {
                    let sum: f32 = frame.iter().sum();
                    samples.push(sum / channels as f32);
                    frame.clear();
                }
            }
        }
    }
    (samples, spec.sample_rate)
}

fn load_events(path: &PathBuf) -> GoldenEvents {
    let data = std::fs::read_to_string(path).expect("events.json missing");
    serde_json::from_str(&data).expect("invalid events.json")
}

fn compare_event_times(expected: &[f32], actual: &[f32], label: &str) {
    assert_eq!(
        expected.len(),
        actual.len(),
        "event count mismatch for {}",
        label
    );
    for (i, (e, a)) in expected.iter().zip(actual.iter()).enumerate() {
        let diff = (e - a).abs();
        assert!(
            diff <= FRAME_TOLERANCE_SEC,
            "event {} in {} differs by {}s",
            i,
            label,
            diff
        );
    }
}

fn compare_confidences(expected: &[f32], actual: &[f32]) {
    assert_eq!(expected.len(), actual.len(), "confidence count mismatch");
    for (i, (e, a)) in expected.iter().zip(actual.iter()).enumerate() {
        let diff = (e - a).abs();
        assert!(
            diff <= CONFIDENCE_TOLERANCE,
            "confidence {} differs by {}",
            i,
            diff
        );
    }
}

#[test]
fn golden_parity_all() {
    let fixtures = fixtures_root();
    let golden_root = fixtures.join("golden");
    let audio_root = fixtures.join("audio");
    let mut config = CoreConfig::default();
    let models_root = repo_root().join("models");
    config.model.model_json = models_root
        .join("downbeats_blstm.json")
        .to_string_lossy()
        .to_string();
    config.model.weights_npz = models_root
        .join("downbeats_blstm_weights.npz")
        .to_string_lossy()
        .to_string();

    let filter = std::env::var("FIXTURE").ok();

    for entry in std::fs::read_dir(&golden_root).expect("golden dir missing") {
        let entry = entry.expect("bad dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().expect("bad dir entry type");
        if !file_type.is_dir() {
            continue;
        }
        if let Some(ref wanted) = filter {
            if name != *wanted {
                continue;
            }
        }
        let golden_dir = golden_root.join(&name);
        let audio_path = audio_root.join(format!("{}.wav", name));
        let events_path = golden_dir.join("events.json");

        println!("fixture {}: loading audio", name);
        let (samples, sample_rate) = load_wav_mono(&audio_path);
        let duration_sec = samples.len() as f32 / sample_rate as f32;
        println!(
            "fixture {}: analyzing ({} samples, {:.1}s)",
            name,
            samples.len(),
            duration_sec
        );
        let t0 = std::time::Instant::now();
        let output = analyze(&samples, config.feature.sample_rate, &config)
            .unwrap_or_else(|e| panic!("analysis failed for {}: {}", name, e));
        println!("fixture {}: analyze done in {:.2?}", name, t0.elapsed());
        assert_eq!(
            output.fps,
            config.feature.fps.round() as u32,
            "fps mismatch"
        );

        println!("fixture {}: loading goldens", name);
        let events = load_events(&events_path);
        let beat_times_gold: Vec<f32> = events.beats.iter().map(|b| b.time_sec).collect();
        compare_event_times(&beat_times_gold, &output.beat_times, "beats");

        let beat_numbers_gold: Vec<u32> = events.beats.iter().map(|b| b.beat_in_bar).collect();
        assert_eq!(
            beat_numbers_gold, output.beat_numbers,
            "beats beat_in_bar mismatch for {}",
            name
        );

        let beat_conf_gold: Vec<f32> = events.beats.iter().map(|b| b.confidence).collect();
        compare_confidences(&beat_conf_gold, &output.beat_confidences);

        assert_eq!(
            output.beat_times.len(),
            output.beat_numbers.len(),
            "beat_times/beat_numbers length mismatch for {}",
            name
        );
        assert_eq!(
            output.beat_times.len(),
            output.beat_confidences.len(),
            "beat_times/beat_confidences length mismatch for {}",
            name
        );

        for i in 1..output.beat_times.len() {
            assert!(
                output.beat_times[i] > output.beat_times[i - 1],
                "non-increasing beat_times at index {} for {}",
                i,
                name
            );
        }
        for beat_number in &output.beat_numbers {
            assert!(
                *beat_number >= 1,
                "invalid beat_number {} in {}",
                beat_number,
                name
            );
        }
        for confidence in &output.beat_confidences {
            assert!(
                (0.0..=1.0).contains(confidence),
                "invalid confidence {} in {}",
                confidence,
                name
            );
        }
        println!("fixture {}: parity ok", name);
    }
}
