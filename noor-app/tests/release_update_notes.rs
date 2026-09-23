const RELEASE_WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");

#[test]
fn updater_manifest_uses_prepared_release_notes() {
    assert!(RELEASE_WORKFLOW.contains("docs\\releases\\${env:GITHUB_REF_NAME}.md"));
    assert!(RELEASE_WORKFLOW.contains("Get-Content -Raw -LiteralPath $releaseNotesPath"));
    assert!(RELEASE_WORKFLOW.contains("Write release notes before pushing the tag"));
    assert!(RELEASE_WORKFLOW.contains("notes = $releaseNotes"));
    assert!(!RELEASE_WORKFLOW.contains("notes = \"${env:GITHUB_REF_NAME}\""));
}
