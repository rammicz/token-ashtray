# Responsive quota layout evidence

The fixture contains Antigravity's two named weekly windows, a single-meter
Claude card, and three Codex meters including a long unbroken identity and
stale/reset details. It is normalized offline data, not an account response.

Focused assertions and optional geometry receipts:

```sh
cargo test --locked --bin token-ashtray quota_display_tests
TOKEN_ASHTRAY_LAYOUT_RECEIPTS=/tmp/new-layout-geometry cargo test --locked --bin token-ashtray write_responsive_geometry_receipts -- --ignored
```

The receipts read actual card slots and meter interaction rectangles from the
real renderer at 360 and 1,000 logical points, for the main board.
Tests also inspect complete painted title/detail glyphs, containment, order,
non-overlap, a single meter's full card width, and focus/usage activation while
resizing between stacked and side-by-side layouts.

Capture an already-built application in a disposable Ubuntu 22.04 GNOME X11
desktop:

```sh
python3 tests/responsive_layout/run.py /tmp/layout-before /path/to/baseline/token-ashtray
python3 tests/responsive_layout/run.py /tmp/layout-after target/release/token-ashtray --expect-responsive
```

This launches the actual ELF with its existing private `--ui-child` pipe
protocol and a fresh profile containing legacy main-board and detached entries. It does not
start a provider poller. Bubblewrap isolates HOME, configuration, PID/network,
DBus and Xvfb from the user's desktop and credentials. Required test tools are
GNOME Shell, bubblewrap, Xvfb, xdotool, Python GI/Gdk and pyatspi.

The driver enables the private AT-SPI bus, confirms detached entries stay closed and captures the board
at both widths, measures card background pixels and quota accessibility
bounds, and exercises real mouse and Tab/Enter usage activation. A fixture
browser executable records URI activation without contacting a provider.
Images, full accessible names, first-row heights, component widths, input
hashes and process/cleanup receipts remain in the requested work directory.
No product-only debug option or replacement rendering implementation is used.

## Meter presentation, Light/Dark and a frozen reset

```sh
python3 tests/responsive_layout/run.py /tmp/meter-before /path/to/baseline/token-ashtray --fixture tests/fixtures/meter-presentation.json --themes dark light --mode used --freeze-clock
python3 tests/responsive_layout/run.py /tmp/meter-after target/release/token-ashtray --fixture tests/fixtures/meter-presentation.json --themes dark light --mode used --freeze-clock --expect-responsive
```

This optional fixture adds exact `6d 23h` resets, fully used Claude, zero,
fractional and 23% values, a 50% boundary under the value, missing reset,
stale freshness and shared attribution. Files are prefixed `dark-`/`light-`.
A local C compiler builds `frozen-clock.c` into a fixture-only preload library.
Only the UI app's CLOCK_REALTIME is fixed at Unix 1800000000; monotonic timers
stay real. `clock-proof.json` records the tested epoch and library hash.
Neither the application ELF nor the user's clock is modified. The default
responsive-layout fixture and mode remain available without these options.

For a disposable Docker container with no host display, bus, home or credential
mounts, pass `--isolated-container` and use an empty `/work` directory. Run the
container with `--network none`; mount source read-only and provide writable
`/tmp` and `/run`. This mode skips nested bubblewrap, creates `/harness` as a
link to this harness and starts only the private session. Stop/remove the
outer container after the runner exits so all native child processes are reaped.
The same option is available in `tests/gnome42/run.py`.
