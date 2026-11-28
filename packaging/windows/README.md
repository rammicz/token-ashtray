# Windows graphical setup

`setup.iss` uses the unmodified Inno Setup **6.5.4** engine. Obtain that
compiler from its [official release](https://github.com/jrsoftware/issrc/releases/tag/is-6_5_4),
verify the Authenticode signature, and keep it outside the distributed package.
The pinned compiler installer used during preparation was signed by Pyrsys
B.V. and Windows reported a valid signature. No compiler is downloaded by the
build script. See [upstream verification](https://jrsoftware.org/inno-setup/downloads/verify).

Build and package only a clean, reviewed source SHA. Record that SHA and the
final app, ZIP and setup hashes with the acceptance evidence. After source or
notice changes, rebuild the candidate and verify its embedded licenses;
earlier artifact receipts apply only to their original bytes.

Build the final application for MSVC with its CRT linked statically and
build-host paths remapped:

```powershell
python scripts/release_build.py --target x86_64-pc-windows-msvc --bin token-ashtray
```

The script runs `cargo build --release --locked --target x86_64-pc-windows-msvc`
with `-C target-feature=+crt-static` and `--remap-path-prefix` rules for the
user profile, Cargo and rustup homes, checkout and Cargo output directory (in
both drive-letter cases), then scans the executable for remaining build-host
paths. Do not set `RUSTFLAGS` yourself: the script refuses it, because Cargo
would replace rather than merge the flags. `build_windows_setup.py` repeats the
scan on the payload executable before Inno compresses it, since the finished
setup's compressed payload cannot be scanned.

Use a dedicated `CARGO_TARGET_DIR` when keeping dynamic and static CRT builds
side by side. Check the final PE's imports with Visual Studio's
`dumpbin /DEPENDENTS`: it must not depend on VCRUNTIME, MSVCP, libgcc or
libwinpthread DLLs. A PE header alone cannot prove the MSVC compiler or CRT
linkage. Record the build command and binary SHA-256 with the acceptance
evidence. Generate/compare locked notices and record actual measurements as
described in [RELEASE.md](../../docs/RELEASE.md). Unknown measurements stay
null and must not be presented as a runtime pass.

Using Python 3.11 or newer, create the normal Windows ZIP with
`scripts/package_release.py`, then:

```powershell
python scripts/build_windows_setup.py dist/token-ashtray-windows-msvc.zip --iscc 'C:\path\Inno Setup 6\ISCC.exe' --output dist
python tests/test_windows_setup.py
```

The builder rejects extra, duplicate, empty or missing archive members, a
binary/hash mismatch, a non-x64/GUI PE, and an old helper lacking the current
read-only preflight contract. It adds the two exact ownership markers and
`INNO-LICENSE.txt`, then produces `Token-Ashtray-Setup.exe` and a SHA-256
sidecar. No source repository or compiler is embedded. Publish the EXE and
checksum on the GitHub release (see [docs/RELEASE.md](../../docs/RELEASE.md));
source pushes do not publish it.

The per-user AppId is `{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}` and must remain
stable across releases. It adds one HKCU Installed apps entry. The default
directory, installation marker, shortcut description and Run value name match
the legacy helper. Graphical upgrades keep the installation path stable;
moving an existing graphical installation is not supported.

Before writing application files, Inno invokes the embedded helper with
`-ValidateOnly`. It checks running processes and ownership without copying,
creating shortcuts or changing startup entries. Inno then installs files and
the shortcut in its rollback-capable transaction; canceling before that
transaction leaves the existing installation intact. A later file-copy error
uses Inno's rollback, including restoration of replaced application files.
The setup never rewrites Run or StartupApproved, preserving their bytes.

Uninstall first validates the installation, then asks for confirmation. At
`usUninstall`, after confirmation and before Inno deletes its tracked files
or registration, the helper rechecks ownership and removes the application,
shortcut and startup entries. A helper refusal raises a fatal event exception,
retaining the Installed apps registration and setup resources for retry.
File/registry failures during helper removal can leave a partial removal;
resolve the reported cause and retry using the retained uninstaller. Settings
and provider profiles are never part of the Inno file log or uninstall list.
Unrelated files are retained; no recursive wildcard removal is used.

The application and setup code are MIT-licensed under the repository's
[LICENSE](../../LICENSE). Packages are published on GitHub Releases with a
`SHA256SUMS` file; `Token-Ashtray-Setup.exe` is included when built and
published for a release.
The installer engine retains its own license and upstream notices and origin.
Its complete unmodified license is in [INNO-LICENSE.txt](INNO-LICENSE.txt) and
is installed alongside the application notices.
Upstream requests commercial users purchase a license; that request is
separate from the redistribution permission in the license. No purchase or
signing service is performed by this build.

The EXE is unsigned unless a maintainer explicitly signs and rehashes it.
Verify the final delivered hash and downloaded-file dialogs on native Windows
11 under a standard user using [WINDOWS-ACCEPTANCE.md](../../docs/WINDOWS-ACCEPTANCE.md).
Do not change protection settings to obtain a passing result.
