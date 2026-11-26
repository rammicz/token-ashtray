# Application and third-party license policy

Token Ashtray's application and native setup code are MIT-licensed under
`LICENSE`, with Jiri Hernik and the original repository URL in the copyright
notice. Preserve the copyright and permission notices in copies or substantial
portions. Third-party components retain their own licenses and notices. The
Cargo package remains unpublished to crates.io (`publish = false`).

The owner approved MIT for the public release on 2026-11-21. Packages built
before that decision retain the license files bundled when they were built;
verify every rebuilt candidate's bundled license and notices separately.

The graphical Windows setup additionally uses unmodified Inno Setup 6.5.4
under its permissive [license](../packaging/windows/INNO-LICENSE.txt). Preserve
the engine's built-in copyright and About information and install its complete
license alongside the application notices. The application and setup code
retain their MIT license; the engine retains its own license. Upstream's
commercial-license purchase request is distinct from the permission in that
license. See the pinned
[build procedure](../packaging/windows/README.md).

`about.toml` lists the accepted licenses for the Linux GNU and Windows MSVC
release graphs. `cargo-about 0.9.2` resolves the selected license alternatives
and generates `THIRD-PARTY.md`. Regenerate that report with the locked
dependencies and compare it with the committed file before packaging, locally
while Actions are disabled. An unknown or unaccepted license is a release
failure, not an acknowledgement that can be bypassed with a repository variable.

## Code dependencies

The selected code licenses allow distribution with the MIT-licensed
application, subject to their notice, attribution and other conditions. Keep
the complete generated notices with each executable and in About. When a crate
offers alternatives, only a permitted alternative is selected; for example, `r-efi` offers MIT alongside
LGPL and this policy selects MIT.

`directories 6` introduced `option-ext 0.2.0`, which is MPL-2.0-only. MPL permits
larger proprietary works, but it requires availability of the covered source.
To avoid those separate duties, the application uses the MIT/Apache-2.0
`directories-next` implementation instead. Do not restore an MPL exception or
add GPL/LGPL/AGPL-only code to the accepted list. Reassess dependency changes
against this policy before release.

The Windows/macOS tray dependency is pinned to `tray-icon 0.14.1`, whose
Linux-only dependency uses the MIT/Apache-2.0 `dirs-next`. Later tray versions add
an unused Linux `dirs` dependency that introduces MPL-only `option-ext` into
the lockfile and cargo-about's report. The pin keeps the complete locked
dependency audit free of that source-disclosure obligation; Linux uses `ksni`.

Primary references:

- [MIT license](https://opensource.org/license/mit)
- [Apache License 2.0, redistribution conditions](https://www.apache.org/licenses/LICENSE-2.0)
- [Mozilla's MPL FAQ, executable distribution and proprietary larger works](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)
- [directories-next license declaration](https://docs.rs/crate/directories-next/2.0.0)
- [dirs-sys-next license declaration](https://docs.rs/crate/dirs-sys-next/0.1.2)

## Pinned native integration

The local eframe 0.31.1 patch retains the upstream MIT and Apache-2.0 license
texts and metadata. Its changed Glow integration initializes accessibility
for every Glow viewport. The application uses one main quota board. The shared native scheduler also
services due hidden-window repaints on Windows. See [patch provenance](EFRAME-PATCH.md)
for the exact upstream checksum, complete modified-file list and removal checks. Generated
third-party notices include the patched eframe package and the compatible
AccessKit/AT-SPI dependencies under the existing accepted-license policy.

## Embedded fonts and data

`epaint_default_fonts 0.31.1` includes fonts under OFL-1.1 and
Ubuntu-font-1.0, in addition to its code licenses. Those terms permit bundling
and embedding the fonts with software; the fonts retain their own licenses
and do not make the application source public. We use the upstream font
assets without renaming or modifying them, include their copyright/license
texts, and do not sell them separately or claim they are proprietary.

- [SIL Open Font License 1.1, conditions 1–5](https://openfontlicense.org/open-font-license-official-text/)
- [Ubuntu Font Licence 1.0, permission and conditions](https://canonical.com/legal/font-licence)

The selected Unicode and CDLA-Permissive data licenses likewise remain in the
generated report. The application's MIT license does not replace the licenses
for these assets or dependencies.

The Compact window identity also embeds the unmodified Open Sans SemiBold
static face (weight 600) under OFL-1.1. Its pinned official source, size and
checksums are in `assets/fonts/SOURCES.json`; the upstream copyright and full
license are in `assets/fonts/OFL.txt`. The notice template includes both in
`THIRD-PARTY.md`, which is embedded in About and accompanies each packaged
binary. The dedicated identity family retains egui's proportional fallback
fonts without changing other application typography.

## Release evidence

Each release candidate must pass the locked license policy, deterministic notice
comparison, normal code checks, and explicit package allowlist checks. The
platform ZIPs contain `LICENSE`, `THIRD-PARTY.md`, this review, the platform
executable, and build measurements. The Linux package also contains
`INSTALL-LINUX.md`, `install-linux.py`, and `token-ashtray-256.png`; the
Windows package also contains `INSTALL-WINDOWS.md` and `install-windows.ps1`.
No repository source or provider credentials belong in either package.
The standalone Windows graphical setup embeds the Windows ZIP payload plus
the Inno engine license and ownership markers. It installs notices and removal
resources in `.setup`; verify the rebuilt candidate's exact embedded licenses
and hashes before delivery.
The standalone Linux graphical installer reuses the approved eframe/Glow
stack without new dependencies. Its exact payload is the Linux app, icon,
`LICENSE`, `THIRD-PARTY.md`, this review, `INSTALL-LINUX.md` and measurements;
the native setup code uses the same MIT application license.
Setup carries and installs the full notices alongside the app. It also
installs itself and its generated build manifest for graphical removal.
Record the exact app/setup hashes and source SHA with those metrics.


## Provider identity assets

Provider marks are used to identify their respective services. They remain the
marks of their owners; inclusion does not imply endorsement.

Assets are sourced from [theSVG](https://github.com/glincker/thesvg) at revision
`9e7c56e6602bba6f71b32b045fd6133f9e9b40d4`. The upstream per-icon registry identifies Claude and Cursor
as CC0-1.0; Codex, Grok, GitHub Copilot, Kimi and Antigravity as MIT.
See `SOURCES.json` for pinned source URLs and checksums. The source SVG geometry
is preserved; currentColor follows the board foreground and rendering creates
96×96 RGBA textures. No logo is downloaded at runtime.

CC0 reference: https://creativecommons.org/publicdomain/zero/1.0/legalcode

## theSVG MIT license

MIT License

Copyright (c) 2025 thesvg.org

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Separate GNOME host extension

The optional [GNOME 42 route](../integrations/gnome42/README.md) is an independent
GPL-2.0-or-later Shell extension source distribution. It communicates with the
application through standard SNI/DBusMenu and is never linked into the application,
added to Cargo dependencies, or included in the application binary ZIP. Its
archive retains complete editable extension sources, schema inputs, GPL license
and upstream notices. The route modification is GPL-2.0-or-later too; it does not
change the application's MIT license. Distribute that extension archive
separately with its sources and notices intact. This route makes no change to
the accepted application dependency licenses above.
