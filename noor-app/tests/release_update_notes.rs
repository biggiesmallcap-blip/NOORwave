const RELEASE_WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");

#[test]
fn updater_manifest_uses_the_github_release_notes() {
    assert!(RELEASE_WORKFLOW.contains("$release = Invoke-RestMethod"));
    assert!(RELEASE_WORKFLOW.contains("notes = $releaseNotes"));
    assert!(!RELEASE_WORKFLOW.contains("notes = \"${env:GITHUB_REF_NAME}\""));
}
