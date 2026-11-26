# Pinned winit Windows patches

Token Ashtray uses the complete published winit 0.30.13 crate through the
`vendor/winit` Cargo patch. The archive SHA-256 is
`a6755fa58a9f8350bd1e472d4c3fcc25f824ec358933bba33306d0b63df5978d`,
verified against the original Cargo.lock entry and the crates.io index. All
217 published files are retained. The archive records upstream revision
`e9809ef54b18499bb4f2cac945719ecc2a61061b` in `.cargo_vcs_info.json`.
Its Apache-2.0 `LICENSE` is unchanged, SHA-256
`6dc0e068dcf3a5bc8e054205b85b7720e1d49265bbc64bf515d2cf79197df69a`.
Dependency versions, features and package metadata are unchanged.

Two context-free patches, applied in this order, make up the complete
archive-to-vendor diff:

1. `vendor/winit-pointer-reentry.patch` modifies
   `src/platform_impl/windows/mod.rs` and
   `src/platform_impl/windows/event_loop.rs`. Its new
   `src/platform_impl/windows/cursor_movement.rs` contains the shared position
   dispatch decision and its regression tests.
2. `vendor/winit-theme-refresh.patch` further modifies those two files and
   `src/platform_impl/windows/dark_mode.rs`. Its new
   `src/platform_impl/windows/theme_refresh.rs` contains the refresh decision
   and its regression tests.

Applying both reconstructs all 219 vendored files, including every unchanged
published file.

## Cursor position after reentry

Upstream's Windows backend remembers the last client position per window.
It emits `CursorLeft` on departure but retains that position. Returning
directly to the same client position emits `CursorEntered` while suppressing
`CursorMoved` as a duplicate. egui-winit clears its pointer on `CursorLeft`
and requires a position before it can translate mouse buttons. Consequently
the repeated position must reach consumers when the cursor enters again.

The Windows `WM_MOUSEMOVE` path now passes its actual
`PointerMoveKind::Enter` result to the position decision. It emits the first
position on entry even when it equals the remembered position. Duplicate
moves while inside remain suppressed. Both departure paths clear the
`IN_WINDOW` flag: `WM_MOUSELEAVE` and the `PointerMoveKind::Leave` branch.
The next successful entry therefore follows the same decision. No OS cursor
query, synthetic input, timer, focus change or egui cache workaround is added.
Other platforms retain their upstream code.

The regression is also compiled by the application's ordinary `cargo test`.
It checks that same-position reentry restores a position before a consumer
can handle a button, with controls for duplicate inside moves, distinct
positions and fresh windows. It tests the production dispatch decision;
native Windows QA must separately establish hover/click recovery on the
actual main board, including direct same-coordinate reentry.
This patch does not by itself explain an earlier panel-close observation.

## Theme after a Windows appearance change

When a window follows the system theme, upstream handles `WM_SETTINGCHANGE`
by asking uxtheme `ShouldAppsUseDarkMode` (ordinal 132) and emits
`ThemeChanged` only when the answer differs. uxtheme keeps answering with the
process's previous appearance until `RefreshImmersiveColorPolicyState`
(ordinal 104) runs, so every light/dark change was applied one change late.

When the message's area is `ImmersiveColorSet`, the handler now calls ordinal
104 before reading the theme. It resolves the ordinal lazily, exactly like
ordinal 132, and does nothing when it is unavailable. Startup theme reads,
explicitly chosen themes, other settings areas and other platforms keep their
upstream behaviour.

The application's ordinary `cargo test` also compiles these regressions. A
model of uxtheme's cached answer checks that each change from either starting
appearance is seen on the same message. Further checks cover the area name
match and the refresh-then-query order, and confirm that the actual
`WM_SETTINGCHANGE` arm and the ordinal-104 lookup use that order. The cached
answer itself comes from Windows. Only a native run that switches the
Windows app mode while Token Ashtray follows the system theme proves the fix.

## Reconstructing and removing the patches

Extract the published crate archive with the checksum above. From inside
`winit-0.30.13`, apply both context-free patches in order:

```sh
git -c core.autocrlf=false apply --unidiff-zero --check /path/to/token-ashtray/vendor/winit-pointer-reentry.patch
git -c core.autocrlf=false apply --unidiff-zero /path/to/token-ashtray/vendor/winit-pointer-reentry.patch
git -c core.autocrlf=false apply --unidiff-zero --check /path/to/token-ashtray/vendor/winit-theme-refresh.patch
git -c core.autocrlf=false apply --unidiff-zero /path/to/token-ashtray/vendor/winit-theme-refresh.patch
```

Verification compares every resulting file byte for byte with the committed
`vendor/winit` blobs. The patches are pinned to LF and the license is
preserved verbatim by `.gitattributes`. Replace a patch only with a reviewed
upstream version that preserves its contract: the cursor-entry position, or
the same-message theme after an appearance change. Re-run the ordinary
regressions and the matching native checks: board pointer reentry and
lifecycle, or system-theme switching. Follow the repository's license and
notice validation when changing dependencies; retain winit's license notice
while the dependency ships.
