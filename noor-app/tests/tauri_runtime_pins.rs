use std::fs;
use std::path::Path;

const TAURI_VERSION: &str = "2.10.3";
const TAURI_BUILD_VERSION: &str = "2.5.6";
const TAURI_RUNTIME_WRY_VERSION: &str = "2.10.1";
const WRY_VERSION: &str = "0.54.4";
const AUTOSTART_VERSION: &str = "2.5.1";
const SINGLE_INSTANCE_VERSION: &str = "2.4.5";

#[test]
fn tauri_runtime_stays_on_known_good_windows_stack() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_manifest =
        fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("read noor-app Cargo.toml");
    let lockfile = fs::read_to_string(
        manifest_dir
            .parent()
            .expect("workspace root")
            .join("Cargo.lock"),
    )
    .expect("read workspace Cargo.lock");

    assert!(
        app_manifest.contains(&format!(
            "tauri-build = {{ version = \"={TAURI_BUILD_VERSION}\""
        )),
        "tauri-build must be exact-pinned so cargo cannot drift the windowing stack"
    );
    assert!(
        app_manifest.contains(&format!("tauri = {{ version = \"={TAURI_VERSION}\"")),
        "tauri must be exact-pinned so cargo cannot drift the windowing stack"
    );

    assert_lock_version(&lockfile, "tauri", TAURI_VERSION);
    assert_lock_version(&lockfile, "tauri-build", TAURI_BUILD_VERSION);
    assert_lock_version(&lockfile, "tauri-runtime-wry", TAURI_RUNTIME_WRY_VERSION);
    assert_lock_version(&lockfile, "wry", WRY_VERSION);
    assert_lock_version(&lockfile, "tauri-plugin-autostart", AUTOSTART_VERSION);
    assert_lock_version(
        &lockfile,
        "tauri-plugin-single-instance",
        SINGLE_INSTANCE_VERSION,
    );
}

#[test]
fn single_instance_is_registered_before_sidecar_owning_plugins_and_setup() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("src/main.rs")).expect("read main.rs");
    let single = main
        .find(".plugin(tauri_plugin_single_instance::init")
        .expect("single-instance plugin");
    let autostart = main
        .find(".plugin(tauri_plugin_autostart::init")
        .expect("autostart plugin");
    let setup = main.find(".setup(move |app|").expect("app setup");
    assert!(single < autostart && autostart < setup);
    assert!(main.contains("startup::AUTOSTART_MARKER"));
}

#[test]
fn native_capability_remains_scoped_to_the_exact_loopback_origin() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capability =
        fs::read_to_string(manifest_dir.join("capabilities/default.json")).expect("capability");
    let parsed: serde_json::Value = serde_json::from_str(&capability).expect("valid capability");
    assert_eq!(
        parsed["remote"]["urls"],
        serde_json::json!(["http://127.0.0.1:17600/**"])
    );
    assert!(!capability.contains("noorwave.local"));
    assert!(!capability.contains("0.0.0.0"));
    assert!(!capability.contains("http://*/"));
}

fn assert_lock_version(lockfile: &str, package: &str, expected: &str) {
    let found = lockfile.split("[[package]]").find_map(|block| {
        if block.contains(&format!("name = \"{package}\"")) {
            block.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("version = \"")
                    .and_then(|value| value.strip_suffix('"'))
                    .map(str::to_owned)
            })
        } else {
            None
        }
    });

    assert_eq!(
        found.as_deref(),
        Some(expected),
        "{package} must stay pinned at {expected}"
    );
}
