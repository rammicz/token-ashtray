# Install Token Ashtray on Windows

Verified Windows packages are published on
[GitHub Releases](https://github.com/rammicz/token-ashtray/releases) with a
`SHA256SUMS` file. Releases publish the portable Windows x64 ZIP and, when
built and published for a release, the graphical setup
(`Token-Ashtray-Setup.exe`). Pick the matching asset on the release page and
check its bundled `LICENSE` and notices; current source is MIT-licensed.

## Graphical setup (Windows 11 x64)

When included in a release, download the Windows setup and the SHA-256 checksum
published with it from the release. Compare the file's SHA-256 with that
checksum, then double-click it in File Explorer. Follow the setup wizard. It
installs only for the current user, without administrator access, Rust, manual
extraction, or a PowerShell command. Open **Token Ashtray** from Start. If the
release provides only the portable ZIP, follow
[Portable ZIP and legacy PowerShell helper](#portable-zip-and-legacy-powershell-helper)
below.

The default location is `%LOCALAPPDATA%\Programs\Token Ashtray`. You can
choose another user-writable location, including a path containing spaces.
Keep that location for future updates. Quit the app from its tray menu before
running a new setup. Setup recognizes the old PowerShell installation at the
default location and upgrades it in place; subsequent graphical setups use
the same Installed apps entry. It refuses a running application, an unrelated
Start shortcut or Run entry, and an unowned installation directory.

Setup does not enable Start at login. Existing enabled, absent, or
Windows-disabled startup settings remain unchanged, including Windows' stored
disabled state. Preferences at
`%APPDATA%\rammi\token-ashtray\config\config.toml` and provider CLI logins
stay outside the installation and are preserved. The installed `.setup`
directory contains licenses, measurements and this guide.

To uninstall, quit the app and open **Settings > Apps > Installed apps >
Token Ashtray > Uninstall**. Confirm removal. This removes the installed
application, its owned Start shortcut, startup entry and setup resources.
It keeps preferences, provider credentials and unrelated files. Unrelated
replacement shortcuts or startup entries cause removal to stop so they can
be resolved first. Do not use the legacy helper to remove a graphical
installation, because that helper does not remove its Installed apps entry.

The setup and application are **unsigned**. SmartScreen behavior depends on
the exact downloaded file and Windows settings. If Windows shows **Windows
protected your PC**, verify the checksum before using **More info > Run
anyway** if available. If a protection blocks launch without that option,
leave it enabled and send the maintainer the exact message and setup hash.
The maintainer's native acceptance report records what was actually observed
for the delivered artifact; a local build without download marking does not
establish SmartScreen behavior. Setup does not change SmartScreen,
Smart App Control, Defender, exclusions, or saved PowerShell execution policy.
The helper's execution-policy option applies only to its hidden child process.

The graphical package uses the MSVC build with the C runtime linked into the
application; no separate Visual C++ runtime installation is required. Windows
system graphics and desktop libraries are still required.

## Portable ZIP and legacy PowerShell helper

Use the Windows x64 ZIP from the release and extract it to a folder you own.
Keep this guide, `install-windows.ps1`, and `token-ashtray.exe` together. Rust and administrator
access are not needed.
Open Windows PowerShell in the extracted folder and run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install-windows.ps1 -Action Install
```

Setup copies the executable to `%LOCALAPPDATA%\Programs\Token Ashtray\` and
adds **Token Ashtray** to the Start menu. Open it from there. The execution
policy option applies to this PowerShell process only. Setup does not turn
on Start at login; use **Start at login** in the app's tray menu if you want
it, and turn it off there later. The tray menu's check mark also reflects the
Token Ashtray switch in **Settings > Apps > Startup** and Task Manager. Turning
Start at login on from the tray menu also turns that switch back on.

This build is not code-signed, so on first launch Microsoft Defender
SmartScreen shows **Windows protected your PC** with only **Don't run**
visible. This is expected. Before you continue, run
`(Get-FileHash .\token-ashtray.exe).Hash` in the extracted folder and compare
the result with `binary_sha256` in the included `RELEASE-METRICS.json` and with
the SHA-256 recorded by the maintainer for that release. A checksum inside the
same ZIP alone detects corruption; it does not verify a trusted publisher.
If they match, choose **More info**, check that the application is `token-ashtray.exe`, and choose
**Run anyway**. Administrator access is not needed. Alternatively, before
extracting, right-click the downloaded ZIP, choose **Properties**, select
**Unblock**, and choose **OK**; this may remove its downloaded-file prompt.
It does not bypass Smart App Control or establish publisher identity. Leave
SmartScreen turned on.

Windows 11 Smart App Control may block an unfamiliar or unsigned executable
before it opens. If it blocks Token Ashtray, leave the protection enabled and
contact the maintainer with the app version, executable SHA-256 and the exact
Windows message. This means launch verification is still incomplete. See
[Microsoft's Smart App Control FAQ](https://support.microsoft.com/en-us/windows/security/threat-malware-protection/smart-app-control-frequently-asked-questions).

For an update, quit Token Ashtray from its tray menu, extract the new ZIP,
and run the same setup command from the new extracted folder. If Token
Ashtray is still running from the installed folder, setup stops without
changing anything. Running setup again is safe. Keeping
the installed path fixed keeps a previously enabled Start at login entry
pointed at the installed executable. Your settings are stored outside the
installed folder at
`%APPDATA%\rammi\token-ashtray\config\config.toml`; the update preserves
the Used/Remaining choice, provider order, board window settings, and other
preferences. Provider CLI logins are stored separately and stay intact.

For a custom per-user location, add
`-InstallDir 'C:\path with spaces\Token Ashtray'`. To move an existing
installation, quit the app and run setup from the extracted ZIP with both the
new `-InstallDir` and `-PreviousDir 'C:\old\Token Ashtray'`. Setup retargets
the Start menu shortcut and a previously enabled Start at login entry without
silently enabling it. If an entry is stale or belongs to something else,
setup stops with an error. Keep the extracted ZIP or download it again for
future updates and removal.

To remove the app, quit it and run from the extracted ZIP:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install-windows.ps1 -Action Uninstall
```

For a custom location, pass the same `-InstallDir` used to install it. If the
app is still running, removal stops without changing anything.
Removal deletes the app-owned Start menu shortcut, its Start at login Run
value and its Windows Startup apps switch, and the installed executable, while
keeping your settings. To remove
Token Ashtray's settings too, add `-RemoveSettings`. This deletes only the
`config.toml` path shown above. It does not delete any provider CLI
credentials. Repeat removal is safe.

If Windows reports `VCRUNTIME140.dll` missing, install Microsoft's current
[Visual C++ x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).
The runtime is not included in the ZIP. If
the tray is unavailable, run the installed executable with `--no-tray` to
open a visible board; a failed tray registration during Start at login also
opens the board with an explanation.

## Provider sign-in and updates

Setup does not sign into providers. Follow the provider's reconnect message in
its own CLI or editor. Cursor/Grok Bot use Cursor desktop sign-in; Copilot checks
existing environment credentials before editor/CLI files and a read-only GitHub
CLI (`gh`) `hosts.yml` fallback. A tray-launched app inherits the environment of
its launcher, which may differ from a terminal. Antigravity requires CLI 1.1.11+
and a completed `agy` sign-in; the CLI owns its Windows Credential Manager login.
Mistral Vibe uses `MISTRAL_API_KEY` (environment or `~/.vibe/.env`) or the
installed `vibe` / `vibe-app-server` CLI with its existing sign-in (`vibe --setup`),
where the CLI owns its OS credential store; native Windows validation of
`vibe-app-server.exe` remains deferred to a Windows machine. Never copy
credentials into Token Ashtray or a bug report. A failing provider does not stop
the others; check its message, network access and login, and allow rate-limit
backoff to finish.

Automatic updates are off by default. **Check for updates automatically**
under **Updates** in the board's **Window settings** lets the app check GitHub
Releases once a day and download a signed, verified update. It installs
silently after you quit Token Ashtray, and at the next launch
**Updates** names the new version. A failed install is tried again after
later Quits, three attempts in all.
Only an installation made by the graphical setup is eligible; a ZIP or
PowerShell-helper copy never updates itself. No release is signed yet, so
current builds show that updates are unavailable and send no update request.
Until then, update with the setup or ZIP steps above.
