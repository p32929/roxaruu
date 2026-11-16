#!/bin/sh
# Remove Roxaruu from this computer: the app, its commands, and everything it
# wrote (settings, keys, themes, open tabs, recent folders, to-dos, logs),
# however it was installed (install.sh, a download, or install-local.sh).
#
#     curl -fsSL https://raw.githubusercontent.com/p32929/roxaruu/master/uninstall.sh | sh
#     ./uninstall.sh [--yes] [--keep-settings]
#
# It lists what it found and asks first. Running Roxaruu windows are closed
# (unsaved changes in them are lost). Never touched: Oxru's folders, a `rox`
# command that isn't Roxaruu's, and the `.roxaruu/settings.json` a project
# may have of its own (it lives in that project).
#
# Overrides, for testing: PLATFORM (pretend to be another OS), APP_DIRS and
# BIN_DIRS (where to look), ROXARUU_NO_SYSTEM=1 (leave the system's own
# records alone: macOS's preferences, permissions, keychains, apt, Windows'
# registry).
set -eu

usage() {
    cat <<'HELP'
Remove Roxaruu: the app, its commands and everything it wrote.

Usage:
  ./uninstall.sh                   List what's there, ask, then remove it.
  ./uninstall.sh --yes             Don't ask.
  ./uninstall.sh --keep-settings   Keep your settings, keys and themes
                                   (~/.config/roxaruu), for a reinstall.
  ./uninstall.sh -h | --help       Show this help.

Running Roxaruu windows are closed first: unsaved changes in them are lost.
Oxru, other programs' commands and projects' own .roxaruu folders stay.
HELP
}

YES=
KEEP=
while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help) usage; exit 0 ;;
        -y | --yes) YES=1 ;;
        --keep-settings) KEEP=1 ;;
        *) echo "Unknown option: $1 (see ./uninstall.sh --help)" >&2; exit 2 ;;
    esac
    shift
done

say() { printf '%s\n' "$*"; }
system() { [ -z "${ROXARUU_NO_SYSTEM:-}" ]; }

case "${PLATFORM:-$(uname -s)}" in
    Darwin) OS=mac ;;
    MINGW* | MSYS* | CYGWIN* | Windows*) OS=windows ;;
    *) OS=linux ;;
esac
EXE=roxaruu
[ "$OS" = windows ] && EXE=roxaruu.exe

# ---- what's here --------------------------------------------------------------
FOUND=""
found() { [ -e "$1" ] || [ -L "$1" ] || return 0; FOUND="$FOUND
$1"; }

# A command in a bin folder, if it's Roxaruu's: the program itself, or a
# `rox` that runs it (a link to it, or a wrapper script naming it).
command_if_ours() {
    f="$1"
    [ -e "$f" ] || [ -L "$f" ] || return 0
    case "$(basename "$f")" in
        roxaruu | roxaruu.exe | roxaruu-hook | roxaruu-hook.exe) found "$f" ;;
        rox | rox.cmd)
            if [ -L "$f" ]; then
                case "$(readlink "$f")" in *roxaruu* | *Roxaruu*) found "$f" ;; esac
            elif grep -qi roxaruu "$f" 2>/dev/null; then
                found "$f"
            fi ;;
    esac
}

CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
SETTINGS="$HOME/.config/roxaruu"
BIN_DIRS="${BIN_DIRS:-${CARGO_HOME:-$HOME/.cargo}/bin $HOME/.local/bin /usr/local/bin}"
for d in $BIN_DIRS; do
    for name in roxaruu roxaruu-hook rox roxaruu.exe roxaruu-hook.exe rox.cmd; do
        command_if_ours "$d/$name"
    done
done

case "$OS" in
mac)
    LIB="$HOME/Library"
    APP_DIRS="${APP_DIRS:-/Applications $HOME/Applications}"
    for d in $APP_DIRS; do found "$d/Roxaruu.app"; done
    # The state folder (~/Library/Application Support/roxaruu) is also the
    # log's (…/Roxaruu: the same folder, macOS ignores case) and holds
    # install-local.sh's signing key.
    found "$LIB/Application Support/roxaruu"
    found "$LIB/Preferences/com.p32929.roxaruu.plist"
    found "$LIB/Saved Application State/com.p32929.roxaruu.savedState"
    found "$LIB/Caches/com.p32929.roxaruu"
    found "$LIB/HTTPStorages/com.p32929.roxaruu"
    ;;
linux)
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    DEB=
    if command -v dpkg >/dev/null 2>&1 && dpkg -s roxaruu >/dev/null 2>&1; then
        DEB=1
        FOUND="$FOUND
the roxaruu package (apt)"
    fi
    found "$DATA/applications/roxaruu.desktop"
    found "$DATA/icons/hicolor/512x512/apps/roxaruu.png"
    found "$DATA/icons/hicolor/scalable/apps/roxaruu.svg"
    found "$DATA/Roxaruu"
    found "$CONFIG/roxaruu"
    ;;
windows)
    LOCAL="${LOCALAPPDATA:-$HOME/AppData/Local}"
    ROAMING="${APPDATA:-$HOME/AppData/Roaming}"
    if command -v cygpath >/dev/null 2>&1; then
        LOCAL="$(cygpath -u "$LOCAL")"
        ROAMING="$(cygpath -u "$ROAMING")"
    fi
    PROGRAM="$LOCAL/Programs/Roxaruu"
    found "$PROGRAM"
    found "$ROAMING/Microsoft/Windows/Start Menu/Programs/Roxaruu.lnk"
    found "$LOCAL/Roxaruu"
    found "$ROAMING/roxaruu"
    ;;
esac
# Settings, keys and themes: kept with --keep-settings. (On Linux it's the
# state folder too; then only the state inside goes.)
if [ -n "$KEEP" ]; then
    FOUND="$(printf '%s\n' "$FOUND" | grep -vx "$SETTINGS" || true)"
else
    found "$SETTINGS"
fi
FOUND="$(printf '%s\n' "$FOUND" | sed '/^$/d' | sort -u)"

RUNNING=
if [ "$OS" = windows ]; then
    tasklist //FI "IMAGENAME eq $EXE" //NH 2>/dev/null | grep -qi "^$EXE" && RUNNING=1
else
    pgrep -x roxaruu >/dev/null 2>&1 && RUNNING=1
fi

if [ -z "$FOUND" ] && [ -z "$RUNNING" ]; then
    say "Roxaruu isn't installed here: nothing to remove."
    exit 0
fi

say "This removes:"
printf '%s\n' "$FOUND" | sed 's/^/  /'
[ -n "$RUNNING" ] && say "and closes Roxaruu, which is running (unsaved changes in it are lost)."
[ -n "$KEEP" ] && say "Kept: $SETTINGS (your settings, keys and themes)."
if [ -z "$YES" ]; then
    # From the terminal even when this script came through a pipe (curl | sh).
    if ! { exec 3</dev/tty; } 2>/dev/null; then
        say "Can't ask here (no terminal): run it again with --yes to remove all of this." >&2
        exit 1
    fi
    printf 'Remove all of this? [y/N] '
    read -r answer <&3 || answer=
    case "$answer" in y | Y | yes | YES) ;; *) say "Nothing removed."; exit 0 ;; esac
fi

# ---- close it -----------------------------------------------------------------
if [ -n "$RUNNING" ]; then
    if [ "$OS" = windows ]; then
        taskkill //F //IM "$EXE" >/dev/null 2>&1 || true
        while tasklist //FI "IMAGENAME eq $EXE" //NH 2>/dev/null | grep -qi "^$EXE"; do sleep 0.2; done
    else
        pkill -x roxaruu 2>/dev/null || true
        while pgrep -x roxaruu >/dev/null 2>&1; do sleep 0.2; done
    fi
    say "Closed Roxaruu"
fi

# ---- what the system keeps about it -------------------------------------------
if system; then
    case "$OS" in
    mac)
        # install-local.sh's signing key: its keychain, and the trust given it.
        sign="$HOME/Library/Application Support/Roxaruu/signing"
        if [ -f "$sign/cert.pem" ]; then
            security remove-trusted-cert "$sign/cert.pem" >/dev/null 2>&1 || true
        fi
        [ -f "$sign/roxaruu.keychain-db" ] && security delete-keychain "$sign/roxaruu.keychain-db" >/dev/null 2>&1 || true
        # What you allowed it (microphone, files…), and macOS's list of apps.
        tccutil reset All com.p32929.roxaruu >/dev/null 2>&1 || true
        defaults delete com.p32929.roxaruu >/dev/null 2>&1 || true
        for d in ${APP_DIRS}; do
            [ -d "$d/Roxaruu.app" ] &&
                /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$d/Roxaruu.app" >/dev/null 2>&1 || true
        done
        ;;
    linux)
        if [ -n "$DEB" ]; then
            sudo_=""
            [ "$(id -u)" = 0 ] || sudo_=sudo
            say "Removing the package (apt asks for your password)…"
            $sudo_ apt-get remove -y roxaruu >/dev/null || say "Note: apt couldn't remove the roxaruu package"
        fi
        ;;
    windows)
        # The installer's own uninstaller first (its files, shortcuts and
        # File Explorer's "Open with Roxaruu").
        [ -x "$PROGRAM/unins000.exe" ] && "$PROGRAM/unins000.exe" //VERYSILENT //SUPPRESSMSGBOXES //NORESTART || true
        for key in 'Directory\shell\Roxaruu' 'Directory\Background\shell\Roxaruu' '*\shell\Roxaruu' 'AppUserModelId\p32929.Roxaruu'; do
            reg delete "HKCU\\Software\\Classes\\$key" //f >/dev/null 2>&1 || true
        done
        # `rox` on PATH: the installer added the folder, its uninstaller doesn't take it out.
        win_program="$(cygpath -w "$PROGRAM" 2>/dev/null || printf '%s' "$PROGRAM")"
        powershell.exe -NoProfile -Command "\$p = [Environment]::GetEnvironmentVariable('Path', 'User'); if (\$p) { \$keep = (\$p -split ';' | Where-Object { \$_ -and \$_.TrimEnd('\\') -ne '$win_program'.TrimEnd('\\') }) -join ';'; [Environment]::SetEnvironmentVariable('Path', \$keep, 'User') }" >/dev/null 2>&1 || true
        ;;
    esac
fi

# ---- the files ----------------------------------------------------------------
printf '%s\n' "$FOUND" | while IFS= read -r path; do
    case "$path" in "" | "the roxaruu package (apt)") continue ;; esac
    rm -rf "$path" 2>/dev/null || say "Couldn't remove $path (try: sudo rm -rf \"$path\")"
done
# On Linux the settings share the state folder: with --keep-settings, only
# what isn't settings, keys or themes goes.
if [ -n "$KEEP" ] && [ "$OS" = linux ] && [ -d "$SETTINGS" ]; then
    for f in "$SETTINGS"/* "$SETTINGS"/.[!.]*; do
        case "$(basename "$f")" in settings.json | keybindings.json | themes | queries) ;; *) rm -rf "$f" ;; esac
    done
fi
say "Roxaruu is removed."
