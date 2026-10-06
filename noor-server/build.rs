// A private reviewed recovery payload can travel inside an ordinary installer
// patch. Public/CI builds contain null; no personal library data lives in git.
fn main() {
    println!("cargo:rerun-if-env-changed=NOOR_CATALOGUE_RECOVERY_MANIFEST");
    let payload = match std::env::var_os("NOOR_CATALOGUE_RECOVERY_MANIFEST") {
        Some(path) => {
            let path = std::path::PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            assert!(
                std::fs::metadata(&path)
                    .expect("reviewed recovery manifest")
                    .len()
                    <= 4 * 1024 * 1024,
                "Recovery payload exceeds size limit"
            );
            std::fs::read(path).expect("read reviewed recovery manifest")
        }
        None => b"null".to_vec(),
    };
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    std::fs::write(output.join("tidal-recovery.json"), payload)
        .expect("write packaged recovery manifest");
}
