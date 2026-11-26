# Install Token Ashtray on Linux

Token Ashtray packages are published on
[GitHub Releases](https://github.com/rammicz/token-ashtray/releases). No
release has been published yet, so no package is available until the first
publication. From a published release, select the asset for your platform.
Check its bundled `LICENSE` and notices; current source is MIT-licensed.

The graphical download is one `token-ashtray-setup-linux-x86_64` file. It
supports Ubuntu 22.04 x86_64 and newer compatible Linux desktops (glibc 2.35+,
OpenGL, X11 or Wayland and standard desktop libraries). No sudo, Rust,
Python, FUSE, extraction or terminal commands are needed for normal setup.

1. Save the Linux graphical setup file from the release into Downloads.
2. In Files (Nautilus), right-click it, choose **Properties → Permissions**,
   and enable **Allow executing file as program**. Close Properties, then
   double-click the file. This permission step is normal for a downloaded
   Linux executable.
3. Quit Token Ashtray if it is already running, then click **Install / Upgrade**.
   Wait for the installed message and click **Close**.
4. Open **Token Ashtray** from the applications menu. Its application icon
   appears there alongside **Token Ashtray Setup**. The latter is for removal.
   If the desktop has not refreshed the menu, log out and back in.

Setup installs into `~/.local/opt/token-ashtray/`. To upgrade, download the
new setup file and repeat these steps. It upgrades both an existing default
ZIP installation and an earlier graphical installation. Repeat setup is safe.
Quit the app before setup and launch it again afterwards to use the new build.
Custom-location ZIP installations continue to use the ZIP helper below.

Start at login stays off on a new installation. An existing enabled or
disabled autostart entry stays unchanged on upgrade. Your app preferences at
`${XDG_CONFIG_HOME:-$HOME/.config}/token-ashtray/config.toml` and external
provider logins stay unchanged. Setup does not sign into providers.

To uninstall graphically, quit Token Ashtray, open **Token Ashtray Setup**
from the applications menu, and click **Uninstall**. Setup removes the app,
both menu entries and its autostart entry. It keeps app preferences and
provider logins. You can close the success window even though setup itself
has been removed. You do not need to keep the downloaded setup file.
If setup reports a stale/unrelated entry or a symbolic-link target, resolve
that conflict before trying again; it will not take ownership of those files.
If another setup operation is running, wait for it to finish and try again.
If a file operation fails, setup restores the previous files before reporting
that you can retry. Fix the reported permission or storage problem, then use
the same downloaded file again. If restoration itself fails, the error names
any retained backup files; keep those files until the storage problem is resolved.

Installed license notices, this guide, build identity (`SETUP-MANIFEST.json`)
and measurements are kept with the executable in the install folder.

## Portable ZIP and custom locations

The alternative Linux ZIP still includes the command-line helper. Download
and extract it into a folder you own, keeping `install-linux.py`,
`token-ashtray`, the icon and notices together. Python 3 is required only for
this helper.

From a terminal in the extracted folder, run:

```sh
python3 ./install-linux.py install
```

Setup copies the executable to `~/.local/opt/token-ashtray/` and adds **Token
Ashtray** to your applications menu. Launch it there, or run
`~/.local/opt/token-ashtray/token-ashtray`. If the menu does not refresh right
away, log out and back in. Setup does not turn on Start at login. Once the app
is running, use **Start at login** in its tray menu if you want it; you can
turn it off there later.

For an update, quit Token Ashtray from its tray menu, extract the new ZIP, and
run the same command from the new extracted folder. The installed path stays
fixed, so a previously enabled Start at login entry stays pointed at the
installed executable. Running setup again is safe. Your settings are stored
outside the installed folder at
`${XDG_CONFIG_HOME:-$HOME/.config}/token-ashtray/config.toml`. The update
preserves the Used/Remaining choice, provider order, board window settings, and other
preferences. Existing provider CLI logins stay where their CLIs keep them.

To use another per-user install location, add
`--install-dir '/path with spaces/Token Ashtray'` to the install command. To
move an existing installation, quit the app and run setup from the extracted
ZIP with both the new `--install-dir` and
`--previous-dir '/old/install/location'`. Setup moves its launcher and, if
Start at login was enabled, retargets that entry while keeping the opt-in.
If an entry is stale or belongs to something else, setup stops with an error
instead of changing it. Keep the extracted ZIP or download it again for
future updates and removal.

To remove the app, quit it and run from the extracted ZIP:

```sh
python3 ./install-linux.py uninstall
```

For a custom location, pass the same `--install-dir` used to install it.
Removal deletes the app-owned menu launcher, its Start at login entry, and
the installed executable, while keeping your settings. To remove Token
Ashtray's settings too, add `--remove-settings`. This deletes only the
`config.toml` path shown above. It never deletes Claude, Codex, Cursor, Grok,
Copilot, Kimi, Antigravity, or other provider CLI credentials. Repeat removal
is safe.

## Desktop requirements and tray troubleshooting

The executable uses standard operating-system and graphics libraries. On
Debian/Ubuntu X11, `libxkbcommon-x11-0` is required for keyboard support
(`sudo apt install libxkbcommon-x11-0`). Minimal/headless systems may lack it
even when Xvfb is installed. The app uses native StatusNotifierItem (SNI) through
`ksni`; it does not require the old adapter's `libayatana-appindicator` library.
Other desktops still need a working SNI tray host.

GNOME needs the [AppIndicator and KStatusNotifierItem Support extension](https://extensions.gnome.org/extension/615/appindicator-support/)
enabled to show SNI icons. Install the package for your distribution:

| Distribution | Package to install |
| --- | --- |
| [Ubuntu 22.04/24.04](https://packages.ubuntu.com/search?keywords=gnome-shell-extension-appindicator) | `sudo apt install gnome-shell-extension-appindicator` |
| [Ubuntu 26.04](https://packages.ubuntu.com/resolute/gnome-shell-ubuntu-extensions) | `sudo apt install gnome-shell-ubuntu-extensions` (provides the AppIndicator extension) |
| [Debian](https://packages.debian.org/en/stable/gnome/gnome-shell-extension-appindicator) | `sudo apt install gnome-shell-extension-appindicator` |
| [Fedora](https://packages.fedoraproject.org/pkgs/gnome-shell-extension-appindicator/gnome-shell-extension-appindicator/) | `sudo dnf install gnome-shell-extension-appindicator` |
| [Arch Linux](https://archlinux.org/packages/extra/any/gnome-shell-extension-appindicator/) | `sudo pacman -S gnome-shell-extension-appindicator` |

Enable AppIndicator in the GNOME **Extensions** app. For the standard extension,
`gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com` is supported;
Ubuntu's variant uses `ubuntu-appindicators@ubuntu.com`. The
[upstream installation guide](https://github.com/ubuntu/gnome-shell-extension-appindicator#installation)
notes that X11 may need a GNOME Shell restart; Wayland needs log out and back in.

Stock Ubuntu AppIndicators 42 maps single-left click to its menu and double-left
click to activation; choose **Open board** from the menu if needed. The separate
[GNOME 42 host route](https://github.com/rammicz/token-ashtray/blob/main/integrations/gnome42/README.md)
provides single-left activation and a distinct right-click menu on the pinned
GNOME 42.9 X11 baseline. It has its own source/license archive and manual,
per-user installation; this app installer does not change Shell extensions.

If the desktop accepts SNI registration but hides the icon, run the installed
executable with `--no-tray` for a visible board. Registration alone cannot prove
that the shell rendered it. Failed tray registration, including during
`--autostart`, opens the board with an explanation. On Wayland the compositor
controls board positioning, always-on-top behavior and whether a second launch
or **Open board** raises or restores a board that is already open.

## Provider sign-in and updates

Setup does not sign into providers. Open the relevant CLI or editor and follow
the provider-specific reconnect message: Cursor/Grok Bot use Cursor desktop,
Copilot can use editor/CLI login or existing environment credentials, and
Antigravity uses its signed-in CLI. Do not paste credentials into a bug report.
A failing provider does not stop the others. Check its message, network access
and login; rate-limited requests wait before retrying. Unknown limits remain
unknown rather than becoming zero.

Automatic updates are off by default. **Check for updates automatically**
under **Updates** in the board's **Window settings** lets the app check GitHub
Releases once a day and download a signed, verified update. It installs
without a window after you quit Token Ashtray, and at the next launch
**Updates** names the new version. A failed install is tried again after
later Quits, three attempts in all.
Only the per-user installation in `~/.local/opt/token-ashtray/` is eligible,
whether graphical setup or the ZIP helper without `--install-dir` made it; a
custom-location installation or a copy run from elsewhere never updates
itself. No release is signed yet, so current builds show that updates are
unavailable and send no update request. Until then, update with the install
steps above.
