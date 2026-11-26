#!/usr/bin/env python3
"""Per-user setup for the portable Linux Token Ashtray archive."""

import argparse
import os
from pathlib import Path
import sys
import tempfile


APP = "token-ashtray"
MARKER = ".token-ashtray-install"
MARKER_TEXT = "Token Ashtray per-user installation\n"


def path_from(value):
    path = Path(value).expanduser().absolute()
    if any(ord(character) < 32 for character in str(path)):
        raise ValueError(f"path contains a control character: {path!r}")
    return path


def xdg_dir(variable, fallback):
    return path_from(os.environ.get(variable) or Path.home() / fallback)


def existing_kind(path):
    if path.is_symlink():
        raise ValueError(f"refusing symbolic link: {path}")
    if path.exists() and not path.is_file():
        raise ValueError(f"expected a regular file: {path}")


def installed_state(directory, allow_removed=False):
    if directory.is_symlink():
        raise ValueError(f"refusing symbolic link: {directory}")
    marker = directory / MARKER
    existing_kind(marker)
    if marker.exists():
        if marker.read_text() != MARKER_TEXT:
            raise ValueError(f"installation marker is unrelated: {marker}")
        return True
    if directory.exists() and any(directory.iterdir()):
        if allow_removed:
            for name in (APP, "token-ashtray-256.png"):
                path = directory / name
                existing_kind(path)
                if path.exists():
                    raise ValueError(f"unowned app-named file in installation directory: {path}")
            return False
        raise ValueError(f"installation directory contains unrelated files: {directory}")
    return False


def desktop_command(binary, autostart=True):
    escaped = (str(binary).replace("\\", "\\\\").replace('"', '\\"')
               .replace("`", "\\`").replace("$", "\\$").replace("%", "%%"))
    command = f'"{escaped}"' + (" --autostart" if autostart else "")
    return command.replace("\\", "\\\\")


def desktop_fields(body):
    fields = {}
    in_entry = False
    for line in body.splitlines():
        line = line.strip()
        if line.startswith("["):
            in_entry = line == "[Desktop Entry]"
        elif in_entry and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            fields[key.strip()] = value.strip()
    return fields


def owned_entry(path, marker, accepted_commands):
    existing_kind(path)
    if not path.exists():
        return None
    body = path.read_text()
    fields = desktop_fields(body)
    if fields.get("Type") != "Application" or fields.get(marker) != "true" or fields.get("Exec") not in accepted_commands:
        raise ValueError(f"unrelated or stale desktop entry at {path}; resolve it before setup")
    return body


def atomic_write(path, data, mode=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".token-ashtray-", delete=False) as tmp:
        temporary = Path(tmp.name)
        try:
            if isinstance(data, str):
                data = data.encode()
            tmp.write(data)
            tmp.flush()
            os.fsync(tmp.fileno())
            if mode is not None:
                temporary.chmod(mode)
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def copy_atomic(source, destination, executable=False):
    existing_kind(destination)
    mode = source.stat().st_mode & 0o777
    atomic_write(destination, source.read_bytes(), mode | (0o100 if executable else 0))


def replace_exec(body, old, new):
    old_line = f"Exec={old}"
    lines = body.splitlines(keepends=True)
    matches = [i for i, line in enumerate(lines) if line.strip() == old_line]
    if len(matches) != 1:
        raise ValueError("startup entry has an ambiguous Exec line")
    i = matches[0]
    newline = "\n" if lines[i].endswith("\n") else ""
    lines[i] = f"Exec={new}{newline}"
    return "".join(lines)


def remove_owned_files(directory):
    if not installed_state(directory, allow_removed=True):
        return
    for name in (APP, "token-ashtray-256.png", MARKER):
        path = directory / name
        existing_kind(path)
        path.unlink(missing_ok=True)
    try:
        directory.rmdir()
    except OSError as error:
        if any(directory.iterdir()):
            print(f"Preserved other files in {directory}", file=sys.stderr)
        else:
            raise error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("install", "uninstall"))
    parser.add_argument("--install-dir", type=path_from,
                        default=path_from(Path.home() / ".local/opt" / APP))
    parser.add_argument("--previous-dir", type=path_from)
    parser.add_argument("--remove-settings", action="store_true")
    args = parser.parse_args()
    if args.action == "install" and args.remove_settings:
        parser.error("--remove-settings is only valid with uninstall")
    if args.action == "uninstall" and args.previous_dir:
        parser.error("--previous-dir is only valid with install")

    destination = args.install_dir
    previous = args.previous_dir or destination
    if previous != destination and not installed_state(previous):
        raise ValueError(f"previous installation is missing or unowned: {previous}")
    installed_state(destination, allow_removed=args.action == "uninstall")
    data_home = xdg_dir("XDG_DATA_HOME", ".local/share")
    config_home = xdg_dir("XDG_CONFIG_HOME", ".config")
    launcher = data_home / "applications" / "token-ashtray.desktop"
    autostart = config_home / "autostart" / "token-ashtray.desktop"
    current_command = desktop_command(destination / APP)
    previous_command = desktop_command(previous / APP)
    launcher_body = owned_entry(launcher, "X-Token-Ashtray-Installed",
                                {desktop_command(destination / APP, False),
                                 desktop_command(previous / APP, False)})
    autostart_body = owned_entry(autostart, "X-Token-Ashtray", {current_command, previous_command})

    if args.action == "install":
        source = Path(__file__).resolve().parent
        binary = source / APP
        icon = source / "token-ashtray-256.png"
        for path in (binary, icon):
            if not path.is_file() or not path.stat().st_size:
                raise ValueError(f"missing archive resource: {path}")
        destination.mkdir(parents=True, exist_ok=True)
        atomic_write(destination / MARKER, MARKER_TEXT)
        copy_atomic(binary, destination / APP, executable=True)
        copy_atomic(icon, destination / icon.name)
        launcher_text = ("[Desktop Entry]\nType=Application\nName=Token Ashtray\n"
                         f"Exec={desktop_command(destination / APP, False)}\n"
                         f"Icon={destination / icon.name}\nTerminal=false\n"
                         "Categories=Utility;\nX-Token-Ashtray-Installed=true\n")
        atomic_write(launcher, launcher_text)
        if (autostart_body is not None and previous_command != current_command
                and desktop_fields(autostart_body).get("Exec") == previous_command):
            atomic_write(autostart, replace_exec(autostart_body, previous_command, current_command))
        if previous != destination:
            remove_owned_files(previous)
        print(f"Installed {destination / APP}; launch from the applications menu")
    else:
        if autostart_body is not None:
            autostart.unlink()
            fields = desktop_fields(autostart_body)
            marker = fields.get("X-Token-Ashtray-Created-Autostart-Dir")
            identity = f"{autostart.parent.stat().st_dev}:{autostart.parent.stat().st_ino}"
            if marker == identity:
                try:
                    autostart.parent.rmdir()
                except OSError:
                    pass
        if launcher_body is not None:
            launcher.unlink()
        remove_owned_files(destination)
        if args.remove_settings:
            settings = config_home / APP / "config.toml"
            existing_kind(settings)
            settings.unlink(missing_ok=True)
            try:
                settings.parent.rmdir()
            except OSError:
                pass
            print(f"Removed app preferences at {settings}; provider CLI credentials were not changed")
        else:
            print(f"Kept app preferences at {config_home / APP / 'config.toml'}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, UnicodeError) as error:
        print(f"Token Ashtray setup: {error}", file=sys.stderr)
        sys.exit(1)
