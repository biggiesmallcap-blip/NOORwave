# Decoder HTTP fixture read handoff

Task: fix draft #354's Clippy failure and make its HTTP WAV fixture handle split request headers.

Root cause: the fixture ignored the byte count returned by socket.read and assumed one read received the complete HTTP request. CI job 114618202121 rejected this with unused_io_amount.

Change: bounded reads consume complete headers before the WAV response, with explicit early-EOF and 4 KiB request-limit checks. Existing socket timeouts remain. No lint is suppressed. This is test-only code; production audio logic, dependency pins, and performance are unchanged by the patch. No new CPU/latency improvement is claimed.

Before: #354 Clippy failed. After: Windows cargo clippy -p noor-server --all-targets --locked passed with the paired native fixture correction; both affected HTTP WAV decoder tests passed. Formatting and diff checks passed. Existing advisory warnings remain.

The inherited Windows-only fixture correction is committed separately as f2411ed4, fix(test): consume complete native fixture request headers, in #352 and synchronized into this draft's base. The decoder correction is 33485bef, fix(test): consume complete decoder fixture request headers.

Outstanding: fresh GitHub CI confirmation; native smoke execution requires idle audio hardware and was skipped while a backend was active. These checks do not establish the updated reference-system performance or endurance targets. Next: confirm CI, then resume recovery ownership review and measurement baselines. Follow-ups added: none.
