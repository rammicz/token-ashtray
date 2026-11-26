# Native Windows 11 acceptance

## Graphical setup

Use the exact final `Token-Ashtray-Setup.exe` and checksum from the draft
or published GitHub release. Build details and payload rules are in
[packaging/windows/README.md](../packaging/windows/README.md). The setup and
MSVC binary are unsigned; a locally copied file without a download mark does
not establish SmartScreen behavior. Do not change Windows protection or
execution-policy settings. In an isolated Windows 11 standard-user profile,
record PASS/FAIL/UNVERIFIED and evidence for each check:

1. Record OS edition/build, user identity and standard-user membership,
   executable/ZIP/setup SHA-256 values, setup Authenticode status, the MSVC
   build command and CRT import check. Inspect the installed licenses: app
   `LICENSE`, generated `THIRD-PARTY.md`, `LICENSE-REVIEW.md`, and
   `INNO-LICENSE.txt`. Check that no Rust or VC runtime installation is needed.
2. Open the downloaded setup through File Explorer. Record every displayed
   SmartScreen, Smart App Control or UAC dialog verbatim, with screenshots.
   If offered, check the hash before choosing **More info > Run anyway**.
   Install using the wizard without elevation or typing a PowerShell command;
   launch from Start. Check the stable per-user target and one Installed apps
   entry. Compare Windows protection settings before and after; they must be
   unchanged. A blocked launch remains unverified until resolved without
   disabling protection.
3. Install with the older PowerShell helper (`install-windows.ps1`) at its
   default location, save config containing unknown fields, provider order and panel settings, and create
   disposable provider-file canaries. Upgrade using graphical setup and
   compare all bytes. Test absent, enabled and explicitly Windows-disabled
   autostart; compare both Run and raw StartupApproved bytes before/after.
4. Install a separately built previous graphical version, upgrade to the final
   setup, then repeat the final setup. Repeat in a custom path containing
   spaces. Check the single shortcut, single Installed apps entry, settings,
   provider canaries and all startup variants. Keep the graphical install path
   stable. Cancel setup before Install and verify the prior files stay intact.
5. Try setup and uninstall while the app is running; both must refuse without
   changing owned files or registration. Test unrelated installation files,
   replacement shortcuts and Run entries, and a foreign `.setup` directory.
   Setup must refuse conflicting owned names; unrelated sibling files and
   unrelated registry values must survive. Test a file-copy failure and verify
   Inno's rollback keeps the prior application usable.
6. Quit the app and uninstall through **Settings > Apps > Installed apps**.
   Confirm application, owned shortcut, startup entries and registration are
   removed, while config/provider canaries and unrelated files remain byte for
   byte. Reinstall and uninstall again. A helper refusal must retain setup
   resources and Installed apps registration for retry.

Keep artifact-specific observations with the result; do not substitute the
legacy ZIP's dialogs or a compatibility-runtime run for this setup's evidence.

## Automatic update after Quit

The real N to N+1 path through the public channel needs two owner-signed,
published releases, and N must embed the production root. Until then, check
the apply half with a QA build. The `qa-offline-update` Cargo feature trusts
the public TEST root from `tests/fixtures/updates/client_vectors`, makes no
update request, and reports only what the `update_seed_probe` example staged.
Everything from the staged file onward is production code: the offline
re-verification at Quit, the eligibility check, the journal, the silent setup
start and setup's own wait, checks and rollback. Never deliver a QA build.

Builds. Use the MSVC steps in [packaging/windows/README.md](../packaging/windows/README.md),
adding `--features qa-offline-update` to the app build:

1. N: build the QA app at the source version, package the ZIP and build
   Setup N.
2. N+1: in a scratch copy of the same source, raise `version` in both
   `Cargo.toml` and the `token-ashtray` entry of `Cargo.lock`, then build the
   QA app, ZIP and Setup N+1 the same way. Copy Setup N+1 and its
   `token-ashtray.exe` to the QA machine.
3. Build the probe from the N source:
   `cargo build --release --locked --features qa-offline-update --example update_seed_probe`.
4. A release build must not contain the test root. For the production
   `token-ashtray.exe` and for the QA build, run
   `Select-String -Path token-ashtray.exe -SimpleMatch -Pattern 04279c682d363f3f8dd4457c5d146448e8b57146c95c6e8b063c2efb2a03b974`.
   The production executable must give no match; the QA one gives a match.

The app starts the setup as `<staged setup> /VERYSILENT /SUPPRESSMSGBOXES
/NORESTART /NOCANCEL /SP- /DIR="<installation folder>" /LOG="<state
folder>\apply-<N+1>-<attempt>.log" /WAITAPP`. The state folder is
`%LOCALAPPDATA%\rammi\token-ashtray\data\updates`, staging is under
`%LOCALAPPDATA%\rammi\token-ashtray\cache\updates`, and
`apply-journal.json` in the state folder records the version and attempt.
Each attempt keeps its own log. At every Quit, the logs and markers of
versions up to the running one are removed where possible; one that is held
open is removed at a later Quit and never blocks the update.
With `/WAITAPP`, setup waits up to 60 seconds until the app's instance guard
`Local\TokenAshtray-{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}` no longer
exists; at the timeout it changes nothing and fails. Only an installation
made by the graphical setup is updated: the running `token-ashtray.exe`
must sit beside `.token-ashtray-install` and `.setup\.token-ashtray-setup`
with their exact texts. A helper-only installation without `.setup` is not
updated automatically. Three failed starts of the same release stop the
automatic retries. Only one setup runs at a time: setup holds
`Local\TokenAshtraySetup-{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}`, and a second
setup started meanwhile exits at once without changing anything. It
creates the guard and learns whether it already existed in one Windows call.
A refused setup writes `apply-<N+1>-<attempt>.refused` beside its log, and
the app does not count that start towards the three failed attempts.

Run each check as the standard QA user and record PASS/FAIL/UNVERIFIED with
the setup log, the journal and file hashes:

1. Install Setup N interactively and launch from Start. With the app not
   running, stage N+1:
   `update_seed_probe --installed <N> --version <N+1> --setup <Setup N+1> --app <N+1 token-ashtray.exe>`.
2. Launch N, turn on **Check for updates automatically** and check that the
   menu says version N+1 is downloaded and verified. Record network activity
   for the app; a QA build makes no update request.
3. With the board open, choose **Quit**. Expect no UAC prompt. Check that
   setup waited for both app processes (log), that the installed
   `token-ashtray.exe` hash and the Installed apps version are N+1, and that
   a Start launch runs N+1 and says it was updated to N+1. Compare config,
   provider canaries, the Start shortcut, and Run plus raw StartupApproved
   bytes for absent, enabled and disabled autostart. Repeat in a folder
   whose path has spaces.
4. Keep `token-ashtray.exe` open from a second process (for example
   PowerShell `[IO.File]::Open(<exe>, 'Open', 'Read', 'None')`) during the
   Quit. Expect the setup to fail without changes or roll back, N to stay
   runnable, the menu at the next launch to report the failed attempt, and
   the next Quit to retry. After the third failure the menu asks for a
   manual setup.
5. Change one byte of the staged setup before Quit: nothing starts and the
   journal is unchanged.
6. Turn updates off after staging, then Quit: nothing starts and staging is
   emptied.
7. Run a copy of the app from outside the installation as the only
   instance: it may report N+1 as ready, but Quit never starts the setup.
8. Install the old helper at a custom path, then upgrade it with Setup N
   interactively; with `.setup` present, the update keeps that path
   (`/DIR`).
9. Start N again while the setup waits or installs. Expect the setup to fail
   or roll back, and the next Quit to retry. Then start N while a setup
   waits and Quit it again: the second setup must exit without changes and
   leave its `.refused` marker, the first one finishes, and the refused
   start is not shown as a failed attempt.
10. Record Smart App Control state and any SmartScreen dialog; the staged
    file is downloaded by the app and has no download mark.

## Portable helper and application compatibility

Use the locally prepared `dist/token-ashtray-windows-gnu.zip`. This is a GNU
cross-build; the release workflow separately targets MSVC. Wine checks in
RELEASE.md do not establish native Windows 11 acceptance.

For the per-user installation flow, use the exact final Windows ZIP from
the draft or published GitHub release. Record its executable and archive hashes separately;
the GNU compatibility package above does not establish installation
acceptance. Follow `INSTALL-WINDOWS.md` from the ZIP root with a disposable
Windows user profile. Record the Windows version and each result:

1. Install without Rust or elevation, launch from the Start menu, and confirm
   the shortcut targets the stable per-user executable. Repeat setup; confirm
   there is one shortcut and no Start at login Run value unless the user opted in.
2. Opt in to Start at login, quit the app, update in place from a new extracted
   ZIP, then move the installation to a path with spaces using `-PreviousDir`.
   Confirm the single Run value points to the new executable. Repeat with an
   explicitly disabled startup state and confirm it stays disabled.
3. Confirm Used/Remaining, provider order, board window settings and unknown config
   fields survive the update and move. Uninstall while retaining preferences,
   reinstall, then uninstall with `-RemoveSettings`. Confirm only Token
   Ashtray's config file is removed; provider CLI credentials and unrelated
   Start menu or Run entries remain. Repeat removal and record any errors.

For the compatibility checks below, extract the GNU package to a stable
user-owned directory. Record Windows version, display scaling, executable
SHA-256 (`Get-FileHash .\token-ashtray.exe`), and
each observed result. Do not copy CLI tokens into reports or screenshots.

1. Start `.\token-ashtray.exe` (or double-click it). Confirm a native board
   and burning-token/ashtray tray icon appear. With no supported login, cards must
   explain the disconnected state rather than show fabricated quotas. Confirm
   that no console window flashes when the app starts.
2. Left-click the tray icon; verify the board opens at its saved
   position and size and gains focus. Click the desktop and another application;
   the board must stay visible. Left-click again while visible; it must come to
   the foreground without hiding or moving. Right-click and
   exercise Open board, Refresh, About, and Quit; confirm New panel is absent. Repeat at 100% and
   150% scaling, including a secondary monitor if available.
3. Confirm the toolbar has no Panel button. Load a legacy configuration with
   ID 0, two detached entries and custom fields. Confirm only the board opens
   at ID 0's saved geometry with its pin and compact settings. Pin the board,
   close it with Alt+F4, and confirm the UI child exits while polling continues.
   Reopen the board three times and confirm each activation uses one quota
   window. Repeat with only detached legacy entries; the board uses defaults.
   Save and restart twice; retained preferences and custom data must survive.
4. Reorder cards using their header handles, select and clear Primary, hide and
   restore a card in Settings, and change refresh interval. Check
   restart persistence. Verify primary buttons and
   usage links still work; Tab and Enter/Space must activate focused controls.
5. For existing supported CLI logins, compare returned quotas,
   reset times and explicit plans with the providers' usage pages. Check the
   documented Windows credential paths in DATA-SOURCES.md. Do not create
   accounts or export secrets solely for this check. Mark absent accounts
   unverified. Refresh must preserve valid data on errors and show its age.
   If the Antigravity CLI is installed, watch its first poll, a scheduled poll,
   and tray Refresh with Antigravity visible. Record whether any console window
   flashes during those polls. With a global stdio MCP server configured in the
   normal Antigravity profile, verify the usage command starts no configured MCP
   processes (including `cmd.exe`, `npx.cmd` or `node.exe`), uses a temporary
   `USERPROFILE` and working directory, and leaves the normal MCP configuration
   unchanged. Verify the isolated command still returns usage through the
   existing login with zero model turns. Mark these checks unverified if the
   CLI, configured MCP server or existing login is absent.
6. Toggle Launch at login on and off. Check the user Run entry reflects the
   executable path, including directories with spaces. If authorized to test
   a sign-out/sign-in, verify enabled startup creates only the tray, then
   disable it again unless the user wants it retained.
7. Quit through the tray. Confirm both host and UI child exit. Restart, close
   the board, wait for the first poll, and record the host working set. Then
   open/dismiss three times and sample again after settling. The UI child must
   be gone; sum any remaining application processes. Targets are 20,000,000
   binary bytes and 15,000,000 tray-only resident bytes.

8. On the exact candidate build, test the board with active repaints. Drag it
   partly beyond every monitor edge while its titlebar remains reachable, wait
   several repaints, resize, Snap, maximize by button and titlebar double-click,
   restore, minimize and restore. Confirm the prior normal frame is preserved.
   Close with X and Alt+F4; the UI child must exit while the host keeps polling.
   Reopen from the tray and verify the saved normal bounds, including after
   closing a maximized or minimized board. Record OS/build SHA, executable hash,
   display scale, native outer/client frame and placement state before/after
   each interaction, plus captures for off-screen placement and maximize/restore.
   Repeat launch, hidden-board remap and inaccessible-frame recovery at 100%,
   150% and 200% (including 1920×1080 at 200%), and across mixed-DPI monitors
   where available. Change the scale of the hidden board's monitor, reopen,
   restore the scale and reopen again: the board must stay on that monitor and
   return at its saved size. Capture the first visible frame separately: passing
   geometry assertions or later settled bounds do not establish native
   first-show containment.

Record PASS/FAIL/UNVERIFIED per step with actual evidence. Restore original
preferences and autostart state. A missing login or unavailable second monitor
is an evidence limitation, not a passing result.
