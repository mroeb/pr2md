#!/bin/sh
# Install or update pr2md for the current user (no root needed) on Linux and macOS.
#
#   ./install.sh                 # uses target/release/pr2md (builds it if missing)
#   ./install.sh --source PATH   # install a specific binary
#   ./install.sh --build         # always run cargo build --release first
#   ./install.sh --no-start      # don't start the tray app
#   ./install.sh --uninstall
#
# Installs to ~/.local/bin (override with PR2MD_INSTALL_DIR), enables start at login and
# starts the tray app. Re-running replaces the binary only if it changed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
dir=${PR2MD_INSTALL_DIR:-"$HOME/.local/bin"}
dest="$dir/pr2md"
source=""
build=0
start=1
uninstall=0

while [ $# -gt 0 ]; do
    case "$1" in
        --source) source=$2; shift ;;
        --build) build=1 ;;
        --no-start) start=0 ;;
        --uninstall) uninstall=1 ;;
        -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
    shift
done

stop_running() {
    if pgrep -f "^$dest" >/dev/null 2>&1; then
        echo "Stopping running pr2md..."
        pkill -f "^$dest" || true
        sleep 1
    fi
}

if [ "$uninstall" = 1 ]; then
    stop_running
    [ -x "$dest" ] && "$dest" autostart off >/dev/null 2>&1 || true
    rm -f "$dest"
    echo "pr2md removed. Settings stay in your config folder (see 'pr2md status' before uninstalling)."
    exit 0
fi

# --- find (or build) the binary
if [ -z "$source" ]; then
    for c in "$here/target/release/pr2md" "$here/pr2md"; do
        if [ -x "$c" ]; then source=$c; break; fi
    done
    if [ -f "$here/Cargo.toml" ] && { [ "$build" = 1 ] || [ -z "$source" ]; }; then
        command -v cargo >/dev/null 2>&1 || { echo "cargo not found - install Rust from https://rustup.rs or use --source" >&2; exit 1; }
        echo "Building pr2md (cargo build --release)..."
        (cd "$here" && cargo build --release)
        source="$here/target/release/pr2md"
    fi
fi
[ -n "$source" ] && [ -f "$source" ] || { echo "No pr2md binary found. Build it or pass --source PATH." >&2; exit 1; }

# --- copy only if new or different
if [ -f "$dest" ] && cmp -s "$source" "$dest"; then
    echo "Already up to date: $dest"
else
    stop_running
    mkdir -p "$dir"
    cp "$source" "$dest.new" && chmod 755 "$dest.new" && mv -f "$dest.new" "$dest"
    echo "Installed $dest"
fi

# --- start at login (points at the installed file)
"$dest" autostart on >/dev/null
echo "Autostart enabled (starts at login)."

case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "Note: $dir is not on your PATH (the tray app works anyway)." ;;
esac

# --- (re)start the tray app
if [ "$start" = 1 ] && ! pgrep -f "^$dest" >/dev/null 2>&1; then
    nohup "$dest" tray >/dev/null 2>&1 &
    echo "pr2md is running in the tray."
fi
