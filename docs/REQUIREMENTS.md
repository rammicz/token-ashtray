# Token Ashtray — Requirements

> Status: living document. Written before any code so that decisions are not
> re-litigated later. If you change a decision here, add it to the Decision log
> with the reason rather than editing history silently.

---

## 1. What it is

A desktop app that answers one question at a glance: **how much of each AI
subscription do I have left this period?**

It lives in the system tray. Clicking the icon opens a small board of provider
cards, each showing plan, price, one or more meters (percentage used), and when
the meter resets. One native board can be positioned and pinned always-on-top.

Tagline: *The tray app for token maxxers.*
Subtitle: *All your limits. One tray.*

---

## 2. Goals

| # | Goal | Concretely |
|---|------|-----------|
| G1 | Lightweight | One executable per platform; standard OS and graphics libraries are allowed. Target < 20 MB on disk, < 15 MB RAM when only the tray runs. |
| G2 | Cross-platform | One codebase. **Linux and Windows ship in v1. macOS is deferred** — there is no Mac to verify on; see §7 and Q2. |
| G3 | Tray-first | The tray icon is the primary surface, not an afterthought bolted onto a web page. |
| G4 | One quota board | The tray opens or raises the same native quota window. About remains an auxiliary dialog. |
| G5 | Always-on-top | Main-board toggle, persisted. |
| G6 | Zero configuration | No API keys, no logins inside the app. It reads credentials the user's CLIs already wrote to disk. |
| G7 | Private by default | No telemetry, no accounts, no cloud sync. Measurements contact only their providers. Explicit user actions may open external links and fetch the author's bounded public YouTube feed. Update checks are off by default. After explicit enable, anonymous HTTPS GET requests may retrieve only signed update metadata and platform artifacts from canonical GitHub Releases and explicitly allowed asset redirects. No updater authentication, accounts, tokens or cookies. Disabling updates cancels work and stops further updater requests. |
| G8 | Honest failure | A provider that cannot be read says so and says why. It never shows a stale or invented number as if it were live. |

### G7 network scope and implementation status

Existing explicit actions open provider usage pages and author links in the
system browser. The main-board “Watch the author’s latest video” action first
fetches the author's public [YouTube Atom feed](https://www.youtube.com/feeds/videos.xml?channel_id=UC9xyobUju3jFwbwT-Kbo-IA).
The [feed implementation](../src/app/author_youtube.rs) allows one lookup in
flight, with a 5-second deadline, 256 KiB body limit and no redirects. It sends
the application/version User-Agent and no provider credentials, authentication
or cookies. YouTube can observe the client IP and request time/version. The
result opens in the system browser; failure opens the author's videos page.
Opening the board/About or hovering does not trigger the feed request. External
pages opened by the user follow the system browser's own session/privacy policy.

The updater is implemented but **not active**. No production signing root is
embedded, so every build reports updates as unavailable and sends no update
request. Its canonical source is GitHub Releases for `rammicz/token-ashtray`,
after owner-controlled public publication. The **Updates** setting is off by
default and explains the source and request data. Only after enable may
anonymous HTTPS GETs retrieve signed metadata and the selected platform artifact,
including necessary explicitly allowed GitHub release-asset redirects: one check
at once, then at most one per 24 hours. A verified update applies after Quit,
only to an installation made by the Windows graphical setup or to the Linux
per-user installation in `~/.local/opt/token-ashtray/`, whether the graphical
setup or the ZIP helper's default install made it. Updates require no account, login, API key, token, cookie or
provider data. Disable cancels checks/downloads, prevents prepared apply and
stops further updater traffic.
G6 is unchanged. The [update contract](updates/public-channel-contract.md) keeps
exact request paths, redirect boundary, limits and signing custody under review;
this policy records no production configuration or live acceptance.

---

## 3. Non-goals

Explicit. These were considered and deliberately dropped — do not "add them back
for completeness".

- **No HTTP server.** Nothing listens on a port. The UI is native.
- **No web UI.** No HTML, no browser, no webview.
- **No mobile, no remote access.** The app shows the machine it runs on.
- **No browser cookie scraping.** No reading of Chrome/Firefox cookie stores, no
  DPAPI / Keychain / gnome-keyring decryption, no `browser-cookie3` equivalent.
  Direct credential discovery uses only readable first-party CLI files.
  For the resumed Antigravity integration, the app may invoke the installed
  CLI's documented read-only JSON quota command using its existing login;
  the app does not access its credential store. Require CLI 1.1.11 or newer,
  bounded execution, and no model turns or interactive authentication.
- **No browser automation.** No Playwright, Puppeteer, CDP, or bookmarklets.
- **No user-supplied API keys.** If a provider cannot be read from an existing
  CLI login, that provider is simply not supported.
- **No billing management.** It reports usage. It does not buy credits, change
  plans, or touch money.
- **No writes to provider files.** Credential files are opened read-only. The one
  permitted exception is an OAuth refresh that the provider's own protocol
  requires, and only where the CLI itself would do the same (see DATA-SOURCES).

---

## 4. Product surface

### 4.1 Tray icon

- A burning token resting in an ashtray. **The token burns down from its top as
  the primary provider's remaining quota falls**: a full disc at 100%, a lower
  half at 50%, a small live shard at 10%, and cold ash with no flame at zero.
  Positive fractions retain a live ember. Geometry is intentionally coarser at
  16 px; full, mid, low and exhausted still have different silhouettes.
- The token and ember also go green (>50%), amber (>20–50%), then red (>0–20%).
  Unknown quota uses a hollow crossed-out token with no flame; unavailable or
  non-finite measurements must not look full or exhausted. Dark outlines and
  pale rims preserve contrast on light and dark tray backgrounds at 16, 24, 32
  and 48 px. The shared procedural renderer supplies native tray pixels, window
  icons and the Windows EXE resource.
- Artwork always represents actual **remaining** quota, independent of the
  Used/Remaining display preference. Tooltip on hover: one line per provider,
  e.g. `Name 42% Remaining · resets in 3d`, with the selected display meaning;
  unreadable providers are marked unavailable.
- Left click: open the board as a normal window at its saved position and size,
  or bring the visible board to the foreground. Repeated clicks never hide it.
- Right click: native context menu — Refresh now, Open board,
  Autostart on/off, Quit.

### 4.2 Board window

The board keeps its saved geometry and remains open when focus moves to another
application or the desktop. It is not anchored to the tray icon. A display
scale change keeps it on the same monitor. When a work area is temporarily too
small, the fitted size is not saved; the saved size returns once it fits.
Provider cards, each with:

- short name, plan label, price
- one or more clickable quota gauges: label, percent remaining, reset time
- status: `live`, `stale`, `reconnect` (credential missing/expired), `error`
- clicking a gauge opens the provider's own usage page in the default browser;
  the provider name provides the same action when no meters are available

The user supplied a dark multi-provider dashboard reference and requested a
denser, more informative redesign (2026-09-05). Acceptance: a compact toolbar,
responsive card grid (single column at narrow board widths, two or more as the
window grows), and compact unavailable-provider rows. Keep settings and
secondary actions out of each card's main data area. Remaining quota and reset
times must be readable without hovering; stale/error states must remain
explicit. Charts must support mouse and keyboard activation. Preserve drag
ordering, primary selection, provider visibility, pin and compact controls.
Use only provider-supplied windows and values; the reference does not authorize
invented counts, plans, categories, history or navigation features.

### 4.3 Board window

- One quota window displays the shared provider snapshot.
- Position, size, always-on-top and compact mode persist for the main board.
- Provider visibility and card order remain global board preferences.
- Legacy ID 0 settings migrate to `window`; detached entries never reopen.
  Unknown detached fields remain inert under `panels`, associated with their IDs.

### 4.4 Board interaction

- Drag to reorder cards. Order persists.
- Hide/show individual providers.
- Manual refresh; automatic refresh on an interval (default 5 min, configurable,
  respecting provider rate limits).

---

## 5. Architecture

```
main
 ├── tray        tray-icon + winit    icon, tooltip, native menu
 ├── ui          eframe / egui        one board window + About dialog
 ├── state       Arc<RwLock<Board>>   one cached provider snapshot
 ├── poll        tokio                scheduler, one task per provider, timeouts
 └── providers   trait Provider       creds -> request -> normalised meters
```

- **One poll, one board.** Opening the board uses the cached `Arc<RwLock<Board>>`
  snapshot without adding provider traffic.
- **Providers are a trait, not a match arm.** Each provider owns its credential
  discovery, request, and parsing, and returns the same normalised shape.
- **Normalised meter shape** (the contract every provider maps onto):

```rust
struct Meter {
    label: String,          // "Weekly", "Requests", ...
    used_percent: f32,      // 0.0 ..= 100.0
    resets_at: Option<OffsetDateTime>,
}

struct ProviderCard {
    short_name: String,
    plan: Option<String>,
    price: Option<String>,
    meters: Vec<Meter>,
    status: Status,         // Live | Stale | Reconnect | Error
    link: Option<String>,
}
```

- **A provider that fails never fails the app.** Each poll task is independently
  timed out and caught; one dead provider shows a `reconnect`/`error` card while
  the rest stay live.

### 5.1 Stack

| Crate | Why |
|-------|-----|
| `eframe` / `egui` | MIT/Apache-2.0. `with_always_on_top()` gives the board pin. No system webview dependency. |
| `tray-icon` | Tray on Windows and Linux (and macOS when it is added); the crate Tauri uses. |
| `reqwest` | `default-features = false`, `features = ["rustls-tls", "json"]`. rustls, **not** native-tls — native-tls drags in OpenSSL and breaks static Linux builds. |
| `rusqlite` | `features = ["bundled"]` so SQLite compiles in and there is no system library to find. Needed for Cursor. |
| `tokio` | Poll scheduler and timeouts. |
| `serde` / `serde_json` | Credential files and API responses. |
| `directories` | Per-OS config/data paths. |
| `time` | Reset timestamps. |

Deliberately **not** in the dependency list: `axum` (no server), `keyring` (no
OS keychain reads — see non-goals), any browser automation crate, any Python.

---

## 6. Licensing and distribution

- **Clean-room.** No code, no file, no structure is copied from any existing
  project. Documents in `docs/` record observed behaviour — credential paths,
  endpoints, response field names — which are facts, not protected expression.
- **MIT.** See `LICENSE`. The owner approved standard MIT on 2026-11-21,
  retaining Jiri Hernik and the original repository URL in the copyright
  notice. Copies or substantial portions must retain the copyright and
  permission notices. This supersedes the original proprietary/patron-exclusivity
  requirement.
- **Distribution.** Packages are published on GitHub Releases in
  `rammicz/token-ashtray`; the earlier patron delivery is historical. The
  maintainer builds and verifies packages locally and publishes the release
  manually. Actions are disabled; local builds alone do not deliver packages.
  Packages built before the MIT decision retain their bundled licenses.
- Third-party crates and assets keep their own licences; preserve their notices
  in `THIRD-PARTY.md` (`cargo-about`) and the applicable asset/vendor files. The
  separate GNOME host extension remains GPL-2.0-or-later.

---

## 7. Decision log

Alternatives that were considered and rejected. Reasons are recorded so they are
not re-argued.

| Decision | Rejected alternative | Why |
|---|---|---|
| Anonymous opt-in public updates and explicit author actions (2026-11-21, #50) | Provider-only network wording and historical patron updater | G7 now records existing user-triggered external links/bounded public YouTube feed and the approved future anonymous signed GitHub Releases path, off by default with cancellation on disable. G6 remains unchanged; exact profile, signing custody and live acceptance remain pending. |
| MIT application license (2026-11-21, #60) | Original proprietary/patron-exclusive requirement | Owner chose permissive reuse with the author name and original repository URL in the standard MIT copyright notice, without extra display or branding conditions. Third-party notices remain mandatory; publication remains an owner action. |
| Clean-room reimplementation | Fork an existing MIT dashboard | Historical rationale: MIT permits the fork, but its copyright notice must be retained and recipients gain republication rights. Patron exclusivity was the original goal; that licensing rationale was superseded by the MIT decision on 2026-11-21. The independent implementation remains. |
| Rust | Node / Bun | Cross-platform tray needs per-platform native modules, which defeats the single-binary goal. ~50–90 MB binary, ~70 MB RAM for an always-on daemon. |
| Rust | Go | Genuinely viable and faster to write, but a larger binary and heavier idle footprint. Revisit only if Rust velocity becomes the bottleneck. |
| `egui` | `Slint` | Historical licensing rationale: Slint is GPLv3 or paid; GPL source obligations conflicted with the original proprietary §6. MIT adoption on 2026-11-21 supersedes that rationale, but does not change the selected UI stack or accepted dependency licenses. |
| `egui` | `Tauri` | Reintroduces a webview; on Linux it needs `webkit2gtk` as a system dependency, breaking the "one binary, no deps" goal. |
| Native windows | Local web server + browser | A native board supports remembered placement, always-on-top and compact decorations without a local server. |
| Read CLI credential files | Read browser cookies | Cookie decryption needs three different OS-specific paths (DPAPI, Keychain with a user consent prompt, gnome-keyring/kwallet), is the single largest source of breakage, and requires extra runtime dependencies. |
| Read CLI credential files | Bookmarklet posting to localhost | Requires a listening socket, which is the only remaining reason to run a server. Dropping it removes the server entirely. |
| No mobile view | Serve the board to a phone | Requires the HTTP server. Explicitly not wanted. |
| Ship Linux + Windows in v1, defer macOS | Claim three platforms | No Mac is available to develop or test on. A `cargo check` on a CI runner proves compilation, not that a tray icon appears or a window pins. Claiming support that has never been run once is exactly the dishonesty G8 forbids. macOS is kept cheap to add later via the platform module and the main-thread invariant (§8.1). |
| Develop on Linux | Develop on Windows | The main development environment and most provider logins are on the Linux machine. Windows becomes the second test target, and is where the messiest credential-path logic lives (`%APPDATA%` vs `%LOCALAPPDATA%`, five candidate directories for Copilot). Side benefit: the hardest tray platform is hit on day one instead of at milestone 6. |
| Linux StatusNotifierItem adapter for the spike | Rely on `tray-icon` alone on Linux | `tray-icon` documents Linux tooltip and tray click events as unsupported, while the spike must prove both. The Linux adapter uses the native StatusNotifierItem protocol for activation, tooltip, and menu; `tray-icon` remains the Windows tray backend. This does not relax the desktop-host requirement: GNOME still needs its AppIndicator extension. |
| Honest Wayland degradation | Pretend app-managed pins and placement work on every Linux desktop | Wayland compositors control window stacking and placement. Enable the app pin only on X11; elsewhere disable it with an explanation and recommend X11 for the full feature set. KDE users may add compositor rules, but the app does not claim to control them. |

---

## 8. Open questions

| # | Question | Blocks |
|---|---|---|
| Q1 | **Resolved: CLI route, live since 2026-11-18.** Antigravity CLI 1.1.11+ owns login and provides a read-only, standalone JSON quota command: `agy -p /usage --output-format json --print-timeout 15s`, with bounded execution and no model turn. The [captured, timestamp-redacted fixture](../tests/fixtures/antigravity.json) shows `command.data.groups[].buckets[]`: `Gemini Models` and `Claude and GPT models` each have a `weekly` bucket with `remaining_fraction: 1` (rendered as `100%`). A native Windows CLI 1.5.9 sample matched this shape. The parser reports `(1 − remaining_fraction) × 100` as used per bucket and skips states outside that contract; the response has no numeric limit, used amount or tier. See [DATA-SOURCES §9](DATA-SOURCES.md#9-antigravity--live-q1). | Antigravity provider |
| Q2 | Does `tray-icon` coexist cleanly with `eframe`'s winit event loop on macOS? **Unanswerable for now — no Mac available.** Contained by an invariant rather than an experiment; see §8.1. | macOS only. No longer blocks the spike. |
| Q3 | Linux tray requires `libayatana-appindicator`, and GNOME hides tray icons without a shell extension. What is the fallback? Proposed: `--no-tray` mode that opens a normal window instead. | Linux support |
| Q4 | **Decided:** the app owns Linux XDG autostart and the Windows per-user HKCU Run value; macOS launchd remains deferred with macOS support. The installed entry determines the menu check state. | Decision resolved; native login QA remains |
| Q5 | Where do board geometry and preferences live? Proposed: one TOML under `directories::ProjectDirs`. | Persistence |
| Q6 | **Wayland cannot deliver G4 and G5.** See §8.2. Which sessions are supported, and what does the app do when it detects one that cannot honour a pin? | Linux support, and the honesty of the feature list |

### 8.1 macOS without a Mac (Q2)

There is no macOS machine to develop or test on, so macOS is **out of scope for
v1** and the app must not claim to support it.

Q2 was originally a blocking experiment. It is downgraded to a design invariant,
because `winit` already requires the event loop to own the main thread on macOS
and recommends it on every platform. Adopting that as a rule from the first
commit costs nothing on Linux and removes most of the macOS risk:

> **Invariant.** Each native event loop is created on, and owns, its process's
> main thread. The tray host pumps native events on its main thread; provider
> polling runs on Tokio workers. The UI child owns its winit/eframe loop on its
> own main thread and receives normalized shared state through IPC. Dedicated
> IPC threads are permitted; no platform code may move a native loop off its
> process's main thread.

To keep macOS cheaply addable later:

- OS-specific inputs for tray construction, always-on-top, autostart, and
  credential base directories sit behind a thin platform module. Provider
  adapters retain their credential filenames, search order, and CLI-specific
  overrides. Adding macOS should mean writing one platform implementation,
  not changing native-loop ownership or provider discovery rules.
- CI includes a `macos-latest` `cargo check` from the first commit. It proves the
  code compiles; it proves nothing about tray or window behaviour, and must not
  be cited as macOS support. Note that macOS runner minutes bill at 10× on
  private repositories, so keep the macOS job to a check, not a full build.
- When macOS is attempted, the first task is a tester with a Mac running the
  spike binary, before any release is promised.

### 8.2 Wayland constraint (Q6)

By design, a Wayland client cannot set its own window position and cannot raise
itself above other windows. This is a deliberate property of the protocol, not a
missing feature. `wlr-layer-shell` provides an escape hatch, but it is a wlroots
extension and **GNOME does not implement it**.

Consequences for this app, which are severe because they hit two of the three
features that motivated the rewrite:

| Capability | X11 | Wayland (GNOME) | Wayland (KDE Plasma) |
|---|---|---|---|
| Tray icon | works | works via SNI | works |
| Always-on-top (G5) | works | **not possible from the client** | possible via KWin window rules; KDE-specific |
| Remembered board position (G4) | works | **not possible from the client** | compositor-side rules only |

GNOME additionally hides tray icons entirely unless the AppIndicator shell
extension is installed (this is Q3).

Decision: **degrade honestly and recommend X11 for the complete Linux feature
set.** At runtime, `XDG_SESSION_TYPE=x11` enables the per-window always-on-top
control. On every other session the control is disabled and explains that the
compositor owns stacking; the board still opens and users place them by hand. KDE
users may choose a KWin rule themselves, but the app does not claim to control
it. A pin saved on X11 appears inactive on unsupported sessions without erasing
the preference, so it takes effect again on X11. Release notes must recommend
X11 to people who require persistent placement or the app-managed pin. G8
applies to the UI's own promises too.

---

## 9. Milestones

1. **Spike** — tray icon + one egui window + always-on-top toggle,
   dummy data. Resolves Q6. Nothing else starts until this holds on the
   Linux development session; Windows is verified before milestone 4.
2. **First provider** — Claude end to end: read credentials, refresh if expired,
   request, parse, render a real meter.
3. **Remaining providers** — Cursor, Codex, Grok, Copilot, Kimi.
4. **Persistence** — card order, hidden providers, board geometry, always-on-top.
5. **Polish** — the burning-token icon, tooltip, context menu, autostart.
6. **Platforms** — Linux is the development target from day one, so this
   milestone is Windows parity plus the Q3 tray fallback. macOS is not in v1.
7. **Release** — CI matrix, `THIRD-PARTY.md`, GitHub Releases publication.


## Implementation decisions and verification (2026-09-05)

- The GNOME X11 development session has a functioning StatusNotifierItem host.
  Native tray activation, separate panels and ABOVE pinning were exercised.
  This establishes the Linux spike gate; it does not establish Windows parity.
- Provider failures must remain isolated. The originally proposed release
  `panic = "abort"` conflicts with that requirement, so release builds retain
  unwinding with LTO, one codegen unit and stripping. A release-mode provider
  panic probe confirmed that a subsequent task remains healthy.
- Remaining-window percentages use the least remaining known window for the
  tray indicator. Unknown windows do not become zero. Plan prices are displayed
  only when the provider supplies them; missing prices are omitted.
- Card order is shared across windows, following the explicit acceptance in
  the board ordering decision. This resolves
  the earlier per-window wording in §4.3; geometry, pin, compact mode and panel
  provider filters remain per-window.
- Autostart uses native XDG desktop entries or the Windows user Run key and
  derives its checked state from the installed entry. It requests tray-only
  startup, with a visible-board fallback if the host is absent.
- Native Windows runtime, live accounts for providers absent on this machine,
  and public release publication remain
  separate evidence or delivery steps. Local implementation and checks proceed
  without pretending those external checks have happened.

- Stock Ubuntu AppIndicators 42 maps single-left click to its native menu and
  double-left click to SNI Activate. The separate [GNOME 42 host route](../integrations/gnome42/README.md)
  supplies the Token Ashtray Id-specific single-left Activate/right-menu split
  for GNOME 42.9 X11. It is an optional source-complete GPL extension archive,
  outside the application package; the app never installs it automatically.
  Native host dispatch, normal-board open/raise and close-to-tray each need
  their own native evidence; the earlier popup behavior is superseded. The
  GNOME integration still needs its remaining native acceptance after those.


### Tray-only memory lifecycle

Measurements on the development software-rendered GNOME desktop showed about
108 MB RSS even for an unmapped eframe root. Destroying the graphics window in
that process retained about 105 MB, while real polling plus SNI alone used
10.26 MB. The UI therefore runs as a transient child of the same executable.

The persistent tray process owns the sole scheduler, CLI credentials and
authoritative `Arc<RwLock<Board>>`. It never initializes graphics. A native UI
child receives normalized snapshots and actions through inherited stdin/stdout
pipes, keeps one read-only board snapshot, and sends refresh
or preference controls back. There is no network listener, web server, token
transport, or second scheduler. Opening the board cannot trigger a provider poll.
Each process keeps its native event handling on its own main thread.

When the board closes in tray mode, the child reports `quit: false` and exits,
releasing its graphics mappings while the tray host and polling continue.
Open board or a left tray click opens or raises the single board, starting a
child from cached data when necessary. Only Quit stops both
processes. Parent pipe closure also closes an orphaned UI; Quit waits for a
bounded UI shutdown and then lets any bounded OAuth refresh save complete.
The no-tray mode and missing-host fallback remain a normal standalone native
window whose root cross exits the application. This is one distributed executable, with two processes while windows
are open and one while only the tray remains.

### User-approved release scope (2026-09-05)

The user explicitly accepted standard operating-system libraries, resolving
the original literal-static wording of G1. Keep eframe and the native graphics
stack; distribute one application executable with required dependency notices.
The disk-size and tray-only memory targets remain unchanged.

Native Windows testing is deferred to the user's other machine and is outside
the current acceptance scope. CI must still build and test Windows; a Windows
build is not a claim of native tray or credential-path verification. Kimi's
captured-account fixture remains deferred. Antigravity integration resumed
when the user installed its CLI and became live after successful signed-in
quota responses on Linux and native Windows.

Historical scope before the 2026-11-21 MIT decision: the user authorized
committing, pushing and releasing only in the private repository, with a
proprietary application and no source in patron packages. The application
license and patron-exclusivity requirement are superseded by §6 and the dated
MIT decision above. Publication remains an owner action; the accepted
dependency-license policy and mandatory notices remain in force.

### Single-board decision (2026-11-21)

The single-board decision removes detached
quota windows at the owner's request. The active board/window requirements
above supersede the original multi-panel design; dated implementation and
evidence records retain their original scope.
