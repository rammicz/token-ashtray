# Pinned eframe native patches

Token Ashtray uses the published eframe 0.31.1 crate through the local
`vendor/eframe` Cargo patch. Its upstream crate archive SHA-256 is
`d0dfe0859f3fb1bc6424c57d41e10e9093fe938f426b691e42272c2f336d915c`,
verified against the crates.io index before extraction. All 32 published files,
including package metadata, are retained. The archive omits its declared
license files; `LICENSE-MIT` and `LICENSE-APACHE` are added unchanged from
upstream revision `1669e52a7ccfc3489c1b0999b9ed48894a0b3887`, recorded in
the archive’s `.cargo_vcs_info.json`. Their upstream Git blob IDs are
`673ea5f0505d6fa65c6a2b6a753c93b8e19d11b9` (MIT) and
`11069edd79019f7dafbe3138841cf289209270dd` (Apache).
Three published files are modified: `src/native/glow_integration.rs`,
`src/native/mod.rs` and `src/native/run.rs`. A new
`src/native/hidden_repaint.rs` contains the repaint dispatch and its regression
tests. A second new file, `src/native/first_show.rs`, gates initial Windows
Glow exposure. `vendor/eframe-glow-accessibility.patch` retains its original filename
and records the complete diff from the published archive, including the two
unchanged upstream license files. Applying it reconstructs all 36 vendored
files; package metadata and the other published files remain unchanged.

## Deferred viewport accessibility

Upstream initializes AccessKit only for the root Glow window. Deferred
viewports need their own native accessibility adapters. The application now
uses a single quota board; the general vendored viewport support remains. The patch carries the existing
winit event-loop proxy into the shared viewport creation path and initializes
an adapter exactly when a viewport's egui-winit State is created. It removes
the root-only duplicate. Existing action routing, per-window tree output,
window ownership and viewport removal remain in use. The patch does not
change the dependency versions or introduce another application event loop.

The application explicitly enables eframe's `accesskit` feature. On Linux,
the adapter follows `org.a11y.Status.IsEnabled` on the desktop session bus;
ordinary screen-reader activation controls accessibility. Tests enable egui's
tree generation directly and assert reachable nodes and actions. Native QA
must additionally correlate the board tree with its
actual window, using a private session whose accessibility status is enabled.
An egui tree test alone cannot prove that the native adapter is connected.

The accessibility change is specific to the app's Glow renderer. Linux and
Windows continue to use their upstream AccessKit platform adapters; Linux
checks do not establish native Windows accessibility behavior.

## Hidden Windows repaint dispatch

Win32 does not deliver the normal paint event to a hidden window. An egui
close command queues a viewport event that needs another UI update, so hiding
the last window could strand the child process before it consumed that close.
The tray parent then waited for the child to exit and retained subsequent
Open board and About actions.

The native event-loop wrapper now directly updates a hidden Windows window
when its existing repaint deadline is due. It does not show the window,
add a timer or change repaint deadlines. Any hidden window qualifies,
including a minimized one with a 0×0 client area whose queued close still
needs the update. Visible windows, including minimized ones, and other
platforms retain native paint dispatch. The
regression source is also compiled by the application's ordinary `cargo test`
command; native tray, board and About behavior still requires Windows QA.
The dispatcher is shared native event-loop code. WGPU integration itself is
unchanged and is not selected or claimed as tested by this application.

## First Windows Glow exposure

Upstream shows the root after painting but before applying the application's
queued viewport commands. Deferred HWNDs are created visible before their own
first callback. Both paths can expose provisional, oversized geometry even
when the application queues a correct fit.

Windows Glow now creates each HWND hidden while retaining the requested
visibility separately. Its first callback receives that HWND's actual native
DPI and outer/client rectangles. After its first paint and buffer swap, initial
style, minimum, size and position commands run before the pending visibility
and focus requests. Explicit hidden startup stays hidden. Output from another
viewport cannot release a child before that child's own paint. Later paints,
hide/show and close-to-tray retain ordinary command handling. Recreation resets
the gate for the new HWND while preserving the last requested visibility.

New hidden viewports request one immediate repaint after their native ID maps
are installed, using the existing hidden-window dispatcher above. Root startup
also retains its immediate resume repaint. Linux and macOS keep upstream
exposure timing; WGPU integration is unchanged. The gate does not estimate
decorations, change winit's global window-creation order or replace per-window
DPI handling. Ordinary application tests compile the production gate and cover
command order, hidden startup, child repaint, recreation and live fitted output.
Native checks must still establish the first visible outer frame and compositor
behavior on the exact Windows artifact at the required scales.

## Reconstructing and removing the patch

Extract the crate archive with the checksum above. From inside the extracted
`eframe-0.31.1` directory, apply the repository's context-free patch using an
absolute path:

```sh
git -c core.autocrlf=false apply --unidiff-zero --check /path/to/token-ashtray/vendor/eframe-glow-accessibility.patch
git -c core.autocrlf=false apply --unidiff-zero /path/to/token-ashtray/vendor/eframe-glow-accessibility.patch
```

The per-command setting preserves the archive's line endings without changing
Git configuration. The patch itself is pinned to LF in `.gitattributes` so it
also applies after a Windows checkout. Verification compares all 36 resulting
files byte for byte with their committed `vendor/eframe` Git blobs, including
the unchanged package metadata and the two license blob IDs recorded above.

Remove this local patch only when a separately reviewed upstream eframe update
initializes accessibility for every Glow viewport, services hidden Windows
repaints and applies first-frame geometry before exposing Windows viewports.
Re-run the emitted-tree checks, native board selection and
quota-wording checks, and hidden-root Open board/About and close-to-tray
checks. Regenerate the lockfile and third-party notices under the existing
license policy when updating dependencies; retain this crate's license notice
while the patched dependency ships.
