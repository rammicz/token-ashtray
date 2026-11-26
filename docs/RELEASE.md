# Building and delivering Token Ashtray

The application and native setup code are MIT-licensed; retain `LICENSE` and
all third-party notices. Packages are published on GitHub Releases in
`rammicz/token-ashtray`. The maintainer builds and verifies the Linux x86_64
and Windows x64 packages locally from one clean source SHA, uploads them with a
`SHA256SUMS` file (and the signed update manifest once production signing
exists) to a draft release, and publishes it manually. A release without a
production signing key carries no update manifest, and its builds report that
updates are unavailable. Verify the exact candidate SHA, embedded notices and
package contents before publishing; earlier artifact receipts identify only
their own bytes.

## Build and run from source

A source build requires Rust and the platform development libraries. Runtime
desktop requirements are in [INSTALL-LINUX.md](../INSTALL-LINUX.md#desktop-requirements-and-tray-troubleshooting)
and [INSTALL-WINDOWS.md](../INSTALL-WINDOWS.md). Build and run locally on Linux:

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo about generate --locked about.hbs -o THIRD-PARTY.md
cargo build --release --locked --bin token-ashtray
./target/release/token-ashtray
```

Use `--no-tray` for a visible native board or `--autostart` for tray-only startup
with a visible fallback if tray registration fails. Windows development notes
cover [build scanning and Cargo output paths](#windows-build-scanning-developers-only).

GitHub Actions are disabled by owner decision as of 2026-10-23. Use the
[local validation checks](../CONTRIBUTING.md#validation) while they are off. The retained
workflow defines Windows `x86_64-pc-windows-msvc` and Linux
`x86_64-unknown-linux-gnu` builds; it will not run until the owner re-enables
Actions. macOS remains compile-only and is not distributed.
The Linux release workflow targets Ubuntu 22.04 to establish glibc 2.35 as the
runtime baseline when run. Keep that baseline separate from the latest-Ubuntu
compile checks: a binary linked on Ubuntu 24.04 can require glibc 2.39 and fail
to start on the development desktop. Release caches are isolated by that baseline.
Release builds use LTO, one codegen unit and stripping. Panic unwinding is kept
so one provider panic cannot kill the shared desktop process; see the decision
log for the resolution of the original conflicting abort-profile request.

Build every packaged executable with `scripts/release_build.py`, never with a
bare Cargo command. It runs `cargo build --release --locked` with rustc path
remapping, so the executable does not embed the build account's home
directory, Cargo registry, checkout or output paths, then scans the result.
`package_release.py`, `package_linux_setup.py` and `build_windows_setup.py`
run the same `scripts/path_leak.py` scan (raw and UTF-16LE) before writing
anything and refuse an executable that still contains such a path. Reports
show counts and redacted labels only. The build refuses a preset `RUSTFLAGS`,
`CARGO_ENCODED_RUSTFLAGS` or Cargo-config `rustflags`, because Cargo would
replace rather than merge them with the remapping.

```sh
python3 scripts/release_build.py --bin token-ashtray
```

To measure an already-running release app without stopping it:

```sh
python3 scripts/release_metrics.py target/release/token-ashtray --pid APP_PID --output RELEASE-METRICS.json
python3 scripts/package_release.py target/release/token-ashtray RELEASE-METRICS.json dist/token-ashtray-linux.zip
```

Build the standalone Linux graphical setup on the same Ubuntu 22.04 release
host after approving the app build and measurements:

```sh
python3 scripts/release_build.py --bin token-ashtray-setup
python3 scripts/package_linux_setup.py target/release/token-ashtray-setup target/release/token-ashtray RELEASE-METRICS.json dist/token-ashtray-setup-linux-x86_64 --source-sha "$(git rev-parse HEAD)"
dist/token-ashtray-setup-linux-x86_64 --inspect
sha256sum dist/token-ashtray-setup-linux-x86_64 target/release/token-ashtray
```

Package only a clean, reviewed release SHA; record that SHA, both executable
hashes and the setup hash with the acceptance report. `--inspect` validates
the complete appended payload and prints its manifest without opening a
window. It verifies the setup stub and each member's SHA-256, exact member
allowlist and app hash against `RELEASE-METRICS.json`. These checks detect
corruption; authenticity rests on the canonical GitHub release. The
standalone ELF includes the exact approved app bytes, icon, license,
generated third-party notices, license review, Linux guide and measurements.
It has no extractor/interpreter/FUSE dependency and installs a copy of itself
for graphical removal. No source or credentials are included. Use
`python3 tests/test_linux_setup.py` and `cargo test --locked --bin
token-ashtray-setup` for focused checks, then the final combined local gates.

On a fresh supported Ubuntu 22.04 desktop/profile, verify the downloaded file
through Nautilus (including its executable-permission Properties step),
installation, actual application-menu launch with icon, ZIP-to-GUI and
GUI-to-GUI upgrade, repeat setup and graphical removal. Compare unchanged
config, enabled/disabled autostart and offline provider canaries. Keep native
screenshots, process/build hashes and cleanup evidence. Headless fixture tests
alone do not establish desktop acceptance. `tests/linux_setup/README.md`
describes the isolated native acceptance harness.

With Actions disabled, the maintainer prepares and verifies this ELF locally,
then adds the verified file to the draft release and its SHA-256 to
`SHA256SUMS`. Users download the ELF from the published release; they need no
source archive or GitHub account. Publishing remains a separate manual
maintainer action. The retained Actions workflow packages portable ZIPs only;
it does not build this ELF.

Take the RSS sample with only the tray visible, no `--ui-child` process
remaining, and after the first provider cycle. Record both cold tray startup
and tray idle after opening and dismissing the UI; cold startup alone does not
prove sustained idle footprint. Record the desktop/session along with the
generated measurements. When enabled, CI also records a startup sample,
explicitly labelled as an autostart request;
headless/fallback measurements do not establish real tray-only footprint.
The targets are 20 MB on disk and 15 MB tray-only RSS. A recorded measurement is
not a claim that either target was met.

When Actions are enabled, a `v*` tag triggers the release workflow.
While they are disabled, a tag push creates no packages or draft release. Each
target first checks formatting, Clippy, Rust tests, release-script tests and
locked, deterministic third-party notice generation before packaging. Packages
contain only the executable, application license, generated third-party
notices, dependency review note, measurements, and platform-specific per-user
install resources. The Linux ZIP adds `INSTALL-LINUX.md`, `install-linux.py`
and `token-ashtray-256.png`. The Windows ZIP adds `INSTALL-WINDOWS.md` and
`install-windows.ps1`; its executable embeds the application icon. The
packager selects that exact allowlist from the executable basename and
rejects missing or empty members. The local `tests/test_release_scripts.py`
gate tests exact synthetic archive membership and Linux fixture setup flows.
Native installation acceptance is tracked separately. Linux graphical setup
also tests failed fresh/upgrade/removal commits with observable rollback and
same-payload retry. An OS lock on the stable installation parent serializes
setup actions; a conflicting action reports busy before changing files.

For the Windows graphical `Token-Ashtray-Setup.exe`, use the static-CRT MSVC
build and packaging instructions in [packaging/windows/README.md](../packaging/windows/README.md).
The same bytes are the Windows update artifact (`windows-setup-exe-v1`): publish
them on the public release as
`token-ashtray-setup-<X.Y.Z>-x86_64-pc-windows-msvc.exe`, and describe them with
`scripts/artifact_descriptor.py --target x86_64-pc-windows-msvc --payload-zip
<the ZIP the setup was built from>`. Only the name changes; the bytes and hash
are those of `Token-Ashtray-Setup.exe`.
This additional artifact embeds the ZIP's fixed payload plus the
installer engine's license; verify its final hash and native installation
separately from the ZIP. The Windows guide is embedded in both packages, so guide changes
require new package hashes; earlier hashes identify only their original source
SHA and artifact bytes. Actions remain disabled.

Packages produced by the retained workflow land in a draft release with their
`SHA256SUMS` and are published the same way. Publishing a tag or release is a
separate manual maintainer action; local builds do not publish anything.

Native Windows tray, popup, pin, credential-path and autostart checks are
run on a native Windows machine; see the Windows checklist. Cross
compilation and CI tests are not native runtime verification. On Linux, X11 supports app-managed positioning and
pinning; Wayland relies on compositor rules and the UI disables unsupported
controls. Antigravity quota is live through its CLI; see DATA-SOURCES §9 for
the states it does not yet parse.


### Windows build scanning (developers only)

Antivirus scanning can add file-I/O overhead to Rust builds; the impact varies
with the machine and workload. Measure before changing settings rather than
assuming every slow build is caused by Defender.

If an available, trusted [Windows Dev Drive](https://learn.microsoft.com/en-us/windows/dev-drive/)
meets your development needs, prefer its [Defender performance mode](https://learn.microsoft.com/en-us/defender-endpoint/microsoft-defender-endpoint-antivirus-performance-mode)
with real-time protection **on**. It scans asynchronously after file opens,
retaining more protection than a folder exclusion, which skips scans entirely.

A folder exclusion is an optional, manual developer choice. First identify the
actual Cargo output directory from the checkout, in the same environment used
for builds:

```powershell
$cargoMetadata = cargo metadata --locked --no-deps --format-version 1
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
$cargoBuildOutput = ($cargoMetadata | ConvertFrom-Json).target_directory
Get-Item -LiteralPath $cargoBuildOutput -ErrorAction Stop | Select-Object FullName, Attributes
Get-ChildItem -LiteralPath $cargoBuildOutput -Force | Select-Object Name, Mode
```

This respects `CARGO_TARGET_DIR` and Cargo's configured `target-dir`; do not
assume the folder is the checkout's `target/`. If a build passes a one-off
`--target-dir`, inspect that actual directory instead. Before excluding
anything, verify the absolute path and that it contains only generated build
artifacts. Do not exclude the repository, user profile, whole drive, a shared
folder containing other data, file types, processes or wildcard paths.

To add only that verified folder, open **Windows Security → Virus & threat
protection → Manage settings → Exclusions → Add or remove exclusions**, choose
**Add an exclusion → File or Folder**, then select the verified folder's exact absolute path. To undo it,
return to the same exclusion list, select that folder entry and choose
**Remove**. See Microsoft's [exclusion configuration guidance](https://learn.microsoft.com/en-us/defender-endpoint/microsoft-defender-antivirus-exclusions-configure).
The folder and its descendants will no longer receive Defender Antivirus
scans while excluded. Keep real-time protection enabled; this development
option is not an installation requirement or a workaround for a blocked
downloaded app. The installer does not configure exclusions.

### Static-runtime experiment

On the development GNOME X11 session, a Linux GNU build using
`RUSTFLAGS="-C target-feature=+crt-static"` produced a 14,785,736-byte static PIE
(`file` and `ldd` both confirmed static linkage), but launching the native board
aborted with `free(): invalid next size (fast)`. It is not a deliverable. The
working 13,495,224-byte build is one executable with system libc/libm/libgcc
linkage and dynamically loaded desktop/graphics libraries. Thus the literal
fully static requirement is not met by the working build; neither a compile
success nor the smaller package establishes that requirement. These numbers
record that experiment, not a promise about later revisions.

A separate `x86_64-unknown-linux-musl` build also linked successfully as a
14,322,688-byte static PIE. Its actual X11 launch exited with winit's
`XNotSupported` / `Dynamic loading not supported` error before creating a
window. These are different runtime failures, not evidence that another CRT
flag will make the required desktop stack static.

The constraint is in the required renderer/windowing stack:

- [eframe 0.31.1](https://docs.rs/eframe/0.31.1/eframe/enum.Renderer.html)
  offers Glow and Wgpu, with no software-renderer integration.
- [winit's X11 backend](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/linux/x11/xdisplay.rs)
  loads Xlib and related libraries through `x11-dl`; replacing that path is an
  [open upstream issue](https://github.com/rust-windowing/winit/issues/3198).
- glutin loads [GLX](https://github.com/rust-windowing/glutin/blob/v0.32.3/glutin/src/api/glx/mod.rs)
  and [EGL](https://github.com/rust-windowing/glutin/blob/v0.32.3/glutin/src/api/egl/mod.rs)
  libraries dynamically, so fixing X11 loading alone would be insufficient.
- [softbuffer](https://docs.rs/crate/softbuffer/0.4.8/source/README.md) supplies
  pixels to windows, not an egui renderer or a drop-in eframe backend.

Keeping the existing eframe integration requires system windowing/graphics
libraries. Keeping a literal fully static executable instead requires a
different renderer/windowing integration. The user has now accepted standard OS libraries and retaining eframe; G1
records that decision. These experiments remain historical evidence and do
not establish fully static support.

### Windows compatibility-runtime checks

The Windows GNU executable also links as a GUI-subsystem PE binary. An
isolated Wine 11.0 run exercised real Win32 panel creation, `WS_EX_TOPMOST`
pinning, panel-only `WM_CLOSE`, complete root shutdown, and the default
tray-parent/UI-child startup. No user profile or CLI credentials were used.

Wine 6.0 could not load `bcryptprimitives.dll`; the verified official Wine 11
packages resolved that loader failure. Wine 11's
`Shell_NotifyIconGetRect` returned `E_NOTIMPL` (`0x80004001`) for the registered
tray window. The `tray-icon` backend requires a successful rectangle lookup
before dispatching tray clicks, so Wine could not establish tray-menu or popup
click parity. This is concrete compatibility-runtime evidence, not a native
Windows 11 pass. All isolated Wine and virtual-display test processes were
stopped after the checks.

### Final Linux development measurement

The optimized build measured on GNOME X11 on 2026-09-05 is 14,157,976 bytes.
With the actual SNI host and live Claude, Codex and Grok polling, cold tray RSS
was 12,959,744 bytes. After three native UI open/dismiss cycles and a settling
period, tray RSS was 13,258,752 bytes. No UI child remained at either sample.
Both disk size and sustained tray-only memory met their targets on this
session; visible-window memory is not included in the tray-only target.

`RELEASE-METRICS.json` records these measurements and the executable's SHA-256.
`dist/token-ashtray-linux.zip` is the locally prepared binary-and-notices
package. Its executable SHA-256 is
`b07956794fa10648b50978ac6b2d59ab9724c4df95df51ed0d216a6e157f528f`.
Hosted release packages carry their own build-specific measurements and hashes.
These Linux results do not
establish Windows runtime parity or live validation of providers whose logins
were unavailable. Standard OS libraries are accepted; fully static linkage is
no longer an acceptance requirement.

The earlier GNU Windows compatibility package predates the final dependency
license changes and is not a release artifact. The retained workflow targets a
Windows MSVC package for delivery once Actions are re-enabled; follow
[native Windows acceptance](WINDOWS-ACCEPTANCE.md) on a native Windows machine.
