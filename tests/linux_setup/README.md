# Native Linux setup acceptance

This harness runs Nautilus, GNOME Shell and the actual native setup/app on
Ubuntu 22.04 X11 in a fresh bubblewrap profile. It uses a private PID/network,
DBus and Xvfb namespace; no user HOME, provider credentials or desktop sockets
are mounted. Host requirements are GNOME 42, Nautilus, bubblewrap, Xvfb,
xdotool, xwininfo, Python GI/Gdk and pyatspi. These are test tools, not installer
runtime dependencies.

```sh
python3 tests/linux_setup/run.py /tmp/setup-first-session dist/token-ashtray-setup-linux-x86_64 --legacy-app /path/to/prior/token-ashtray --previous-setup /path/to/prior/gui-setup --absent-applications --install-only
python3 tests/linux_setup/run.py /tmp/setup-after-login dist/token-ashtray-setup-linux-x86_64 --legacy-app /path/to/prior/token-ashtray --previous-setup /path/to/prior/gui-setup --resume-profile /tmp/setup-first-session
```

Use an earlier real Linux app as the default portable ZIP seed, and a separately
packaged prior graphical setup carrying an earlier app for GUI upgrade.
Neither seed is claimed as a published release. The first run starts with no XDG applications directory, performs graphical
installation and ends that desktop session. The second copies only that
isolated profile into a new namespace and starts a fresh GNOME/DBus session,
representing the documented desktop re-login when its app list needs refresh.
It keeps the installed absolute paths (`/work/...`) and download permission.
No menu entries are seeded or synthesized by the harness. The driver enables executable
permission through Nautilus Properties, executes the download through Files,
clicks the native setup controls, searches and launches from GNOME's actual
applications search with icon, and removes through the installed Setup menu
entry. Search selection uses the exact visible accessibility label belonging to
this session's Shell PID, independent of result order. Missing, ambiguous or
unusable labels fail the run; there is no direct-launch fallback. Duplicate
labels with identical screen bounds alias one result; distinct bounds are
ambiguous and fail. Receipts retain the selected native bounds/click and the
installed app or Setup PID, executable hash and arguments. Isolated selector regressions run without GNOME via
`python3 tests/test_linux_setup.py`.

It checks both upgrade routes, enabled/disabled startup, repeat setup,
byte-identical preferences and offline provider canaries. Screenshots, window
and executable identities, input/harness hashes and cleanup records stay in
the requested evidence directory. The wrapper reaps only its owned process;
its private namespace teardown removes all desktop children and listeners.
