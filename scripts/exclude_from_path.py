#!/usr/bin/env python3

"""
exclude_from_path: Temporarily shadows directories in PATH to exclude a specific binary.
"""

import argparse
from datetime import datetime
import os
import random
import shutil
import string
import subprocess
import sys
import tempfile


def generate_session_dir() -> str:
    """Creates a common parent directory in the format:

    {temp_root}/exclude_from_path/{YYYY-MM-DD}.{randomchars}
    """
    date_str = datetime.now().strftime("%Y-%m-%d")
    rand_chars = "".join(random.choices(string.ascii_lowercase + string.digits, k=8))
    base_parent = os.path.join(tempfile.gettempdir(), "exclude_from_path")
    session_dir = os.path.join(base_parent, f"{date_str}.{rand_chars}")
    return session_dir


def sanitize_dirname(path: str) -> str:
    """Converts a path into a safe directory name component."""
    return path.strip("/").replace("/", "_") or "root"


def build_filtered_path(binary_name: str, session_dir: str) -> tuple[str, list[str]]:
    """Inspects PATH and creates shadow directories for locations containing binary_name.

    Returns the new PATH string and a list of created directories.
    """
    current_path = os.environ.get("PATH", "")
    path_entries = current_path.split(os.pathsep)

    new_path_entries: list[str] = []
    created_dirs: list[str] = []
    counter = 1

    for entry in path_entries:
        if not entry:
            continue

        target_file = os.path.join(entry, binary_name)

        # Check if directory exists and contains the binary to exclude
        if os.path.isdir(entry) and (
            os.path.exists(target_file) or os.path.islink(target_file)
        ):
            sys.stderr.write(
                f"[INFO] Found '{binary_name}' in PATH directory: {entry}\n"
            )

            # Ensure parent session directory exists
            os.makedirs(session_dir, exist_ok=True)

            # Create path-specific shadow directory (e.g., path1_usr_bin)
            safe_name = sanitize_dirname(entry)
            shadow_dir = os.path.join(session_dir, f"path{counter}_{safe_name}")
            os.makedirs(shadow_dir, exist_ok=True)
            created_dirs.append(shadow_dir)
            counter += 1

            # Populate shadow directory with symlinks to everything except binary_name
            try:
                for item in os.listdir(entry):
                    if item == binary_name:
                        continue
                    src = os.path.join(entry, item)
                    dst = os.path.join(shadow_dir, item)
                    try:
                        os.symlink(src, dst)
                    except OSError as e:
                        sys.stderr.write(
                            f"[WARN] Failed to link {src} -> {dst}: {e}\n"
                        )
            except PermissionError as e:
                sys.stderr.write(
                    f"[WARN] Cannot read directory {entry}: {e}. Keeping original.\n"
                )
                new_path_entries.append(entry)
                continue

            sys.stderr.write(f"[INFO] Replaced '{entry}' -> '{shadow_dir}'\n")
            new_path_entries.append(shadow_dir)
        else:
            new_path_entries.append(entry)

    return os.pathsep.join(new_path_entries), created_dirs


def cleanup_directories(created_dirs: list[str], session_dir: str) -> None:
    """Removes all shadow directories and the parent session directory."""
    for directory in created_dirs:
        if os.path.exists(directory):
            sys.stderr.write(f"[INFO] Removing shadow directory: {directory}\n")
            shutil.rmtree(directory, ignore_errors=True)

    if os.path.exists(session_dir):
        sys.stderr.write(f"[INFO] Removing session parent: {session_dir}\n")
        shutil.rmtree(session_dir, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Exclude a binary from PATH during command execution or export."
    )
    parser.add_argument("binary_name", help="Name of the executable binary to exclude")
    parser.add_argument(
        "--show-path",
        action="store_true",
        help="Print modified PATH to stdout and preserve generated shadow directories",
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Command and arguments to execute with the modified PATH",
    )

    args = parser.parse_args()

    session_dir = generate_session_dir()
    new_path, created_dirs = build_filtered_path(args.binary_name, session_dir)

    if args.show_path:
        print(new_path)
        return

    # If --show-path wasn't passed, a command is required
    if not args.command:
        parser.error(
            "A command must be specified when not running with --show-path."
        )

    # Strip leading '--' if passed to separate python args from the command
    command_to_run = args.command
    if command_to_run and command_to_run[0] == "--":
        command_to_run = command_to_run[1:]

    if not command_to_run:
        parser.error("No valid command provided to execute.")

    # Prepare environment with modified PATH
    env = os.environ.copy()
    env["PATH"] = new_path

    exit_code = 0
    try:
        proc = subprocess.run(command_to_run, env=env)
        exit_code = proc.returncode
    except FileNotFoundError:
        sys.stderr.write(
            f"[ERROR] Command not found in modified PATH: {command_to_run[0]}\n"
        )
        exit_code = 127
    except KeyboardInterrupt:
        exit_code = 130
    finally:
        cleanup_directories(created_dirs, session_dir)

    sys.exit(exit_code)


if __name__ == "__main__":
    main()
