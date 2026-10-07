Desktop hi-fi player for your TIDAL library. Windows installer plus portable recovery zip, and portable macOS and Linux builds.

## Downloads

| Platform | File |
|---|---|
| Windows (installer) | `NOORwave-{{TAG}}-windows-x64-setup.exe` |
| Windows (portable) | `NOORwave-{{TAG}}-windows-x64.zip` |
| macOS (Apple Silicon) | `NOORwave-{{TAG}}-macos-arm64.tar.gz` |
| macOS (Intel) | `NOORwave-{{TAG}}-macos-x64.tar.gz` |
| Linux | `NOORwave-{{TAG}}-linux-x64.tar.gz` |

**Windows:** run the installer (installs to `%LOCALAPPDATA%\Programs\NOORwave`, auto-updates), or unzip and double-click `NOORwave.exe`.
**macOS / Linux:** unzip, then run `./NOORwave`.

Each build bundles the app, the local `noor-server`, and the UI in `www/` (don't delete it). The Windows portable zip is your recovery path if the installer or updater is ever blocked.

<details>
<summary><b>Windows blocked at first launch?</b></summary>

Builds are not CA-signed yet, so SmartScreen or Smart App Control may warn on or block the first launch on strict systems. The installed updater payload is still signed with the project's Tauri key, so updates stay verified. The portable zip is the simplest fallback. To build your own copy from source:
```powershell
.\scripts\build-windows11-release.cmd
```
The result is still unsigned, so strict Smart App Control may still block it.
</details>

<details>
<summary><b>macOS blocked by Gatekeeper?</b></summary>

```
xattr -cr NOORwave noor-server
```
</details>

<details>
<summary><b>Verify your download</b></summary>

```
sha256sum -c sha256sums.txt
```
On Windows, compare the setup exe and portable zip against `sha256sums.txt`.
</details>
