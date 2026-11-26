# Refresh feedback native probe

Build `cargo build --locked --example refresh_feedback_probe`. Run the resulting
example executable only on an isolated native QA desktop. It opens its own
board and, in hosted mode, a tray icon; it never drives the desktop or browser.
The launcher explicitly injects a disposable ConfigStore into the probe host and UI child,
and supplies temporary cache/home directories. It does not rely on APPDATA overrides
for Windows known-folder configuration paths.
Only the two synthetic providers in the example are scheduled. They never read
credentials or call any provider endpoint. Keep its author link and autostart
setting untouched.

```
refresh_feedback_probe --standalone --output-dir <standalone-directory>
refresh_feedback_probe --output-dir <hosted-directory>
refresh_feedback_probe --standalone --reduced-motion --output-dir <static-directory>
```

The same production board, IPC and host source is compiled into the example.
Its crate-private factory supplies two synthetic providers with a four-second
minimum, ten-second scheduled interval and five-second timeout. Production
constructors retain their sixty-second minimum and twenty-second timeout.
The synthetic requests finish after 0.6 and 2.5 seconds with identical quotas.
Claude fails on attempts two and four with six seconds of backoff; cached
values and the failure detail stay present.

Each run lasts about 39 seconds. It captures its own renderer through egui's
screenshot command as PPM files, plus the actual native frame's AccessKit card
state text/live attributes as JSON. No desktop capture or Windows settings
changes are used. `--reduced-motion` sets this probe's egui animation duration
to zero. The ordinary application also reads the current desktop preference.

Startup is followed by requests at five seconds, a repeated activation at 5.1,
and a request at nine seconds that waits for Claude's backoff. The captures
cover each card's own result, waiting, running, partial failure and completion,
and quiet scheduled failure/retry. Provider call counts are logged separately.
The scripted requests call the same method as the toolbar; mouse/Enter/Space
activation is covered by `app::refresh_feedback_test`. Native input routing and
close-to-tray/reopen still require independent native verification.

The factory is a dependency-injection seam, not a production CLI flag. The
example has its own entry point and the normal binary cannot enable fixtures.
A successful build is not native acceptance. Inspect the captured images and
state receipts and record platform, mode, executable hash and observed results.
