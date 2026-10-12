# Issue tracker: local markdown

Issues and PRDs live under `.scratch/<feature-slug>/`.

- PRD: `PRD.md`.
- Implementation issues: `issues/<NN>-<slug>.md`, numbered from `01`.
- Record triage state as a `Status:` line near the top of each issue.
- Append discussion under `## Comments`.

When a skill requests publishing to the issue tracker, create a local markdown
file following this convention. When it requests a ticket, read the referenced
file. Do not create a GitHub issue unless the user requests that publication.
Use the status vocabulary in `triage-labels.md`.
