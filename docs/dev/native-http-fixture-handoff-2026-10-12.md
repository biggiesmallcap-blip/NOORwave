# Native HTTP fixture read handoff

Task: correct the Windows-only silent PCM recovery fixture's HTTP request handling.

Finding: local all-target Clippy rejected an ignored socket read count. A single read also assumed complete headers, risking unread request bytes and a connection reset when the fixture closed. Linux CI does not compile this Windows-only test.

Change: consume headers in bounded reads, reject early EOF or requests larger than the existing 4 KiB buffer, and retain the existing socket timeout. This changes test code only; production playback and dependency pins are unchanged. Performance outcome: no production performance change, and no new timing measurement claimed.

Before: Windows all-target Clippy failed with unused_io_amount. After: the native and decoder fixture corrections together passed cargo clippy -p noor-server --all-targets --locked on Windows. Existing advisory warnings remain. The decoder fixture's two HTTP WAV integration tests also passed.

The native fixture itself was not executed: a noor-server process was active, and the test requires idle audio hardware. Its Windows code was compiled by all-target Clippy. This does not prove native audio recovery or the new hardware/performance acceptance targets.

The focused commit, fix(test): consume complete native fixture request headers, belongs to draft #352. The decoder fixture correction belongs to #354. Next: run the silent native fixture when hardware is idle, then continue reference-system measurements and recovery ownership review. Follow-ups added: none.
