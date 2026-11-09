#!/bin/sh
# Build this checkout in release mode and install Roxaruu for this user,
# like any other app on the system:
#
#   macOS    Roxaruu.app in /Applications (Launchpad, Spotlight, the Dock),
#            plus the `roxaruu` and `rox` commands in ~/.cargo/bin.
#   Linux    the commands in ~/.cargo/bin, plus an app-menu entry and icon
#            (~/.local/share/applications, ~/.local/share/icons).
#   Windows  (Git Bash / MSYS) roxaruu.exe and rox.cmd in
#            %LOCALAPPDATA%\Programs\Roxaruu, plus a Start Menu shortcut.
#
#     ./install-local.sh            (--vup [patch|minor|major] bumps the
#                                    version first; --help for the rest)
#
# Re-run it to update. `rox` opens the folder you're in (`rox path` another
# one). It never touches Oxru: different command, app and bundle id.
#
# Overrides: BIN_DIR (the commands), APP_DIR (macOS app folder),
# XDG_DATA_HOME (Linux menu entry), PLATFORM (pretend to be another OS, for
# testing the script). The macOS app is unsigned.
#
# `./run.sh --temp` runs the checkout without installing anything.
set -eu

usage() {
    cat <<'HELP'
Build this checkout and install Roxaruu for this user, like any other app.

Usage:
  ./install-local.sh                  Build (release) and install. Re-run to update.
  ./install-local.sh --vup [part]     Bump the version first, then build and install.
                                      part: patch (default), minor or major
                                      e.g. 0.2.0 -> 0.2.1 / 0.3.0 / 1.0.0
  ./install-local.sh -h | --help      Show this help.

The version shows in the app's status bar and in `rox --version`, so you can
see the update landed.

What it installs:
  macOS    Roxaruu.app in /Applications (or ~/Applications), and the
           `roxaruu` and `rox` commands in ~/.cargo/bin.
  Linux    the commands in ~/.cargo/bin, plus an app-menu entry and icon.
  Windows  (Git Bash) roxaruu.exe and rox.cmd in
           %LOCALAPPDATA%\Programs\Roxaruu, plus a Start Menu shortcut.

Replacing the installed app closes its running windows on macOS; to try a
change without that, use ./run.sh --temp.

Settings (environment variables):
  BIN_DIR        where the commands go
  APP_DIR        macOS: where Roxaruu.app goes
  XDG_DATA_HOME  Linux: where the app-menu entry goes
  PLATFORM       pretend to be another OS (to test this script)
HELP
}

VUP=
while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help) usage; exit 0 ;;
        --vup=*) VUP="${1#--vup=}" ;;
        --vup)
            VUP=patch
            case "${2:-}" in patch | minor | major) VUP="$2"; shift ;; esac
            ;;
        *) echo "Unknown option: $1 (see ./install-local.sh --help)" >&2; exit 2 ;;
    esac
    shift
done
case "$VUP" in
    "" | patch | minor | major) ;;
    *) echo "--vup takes patch, minor or major, not \"$VUP\"" >&2; exit 2 ;;
esac

cd "$(dirname "$0")"

green() { printf '\033[0;32m%s\033[0m\n' "$1"; }
blue()  { printf '\033[0;34m%s\033[0m\n' "$1"; }

case "${PLATFORM:-$(uname -s)}" in
    Darwin) OS=mac ;;
    MINGW* | MSYS* | CYGWIN* | Windows*) OS=windows ;;
    *) OS=linux ;;
esac

if ! command -v cargo >/dev/null 2>&1; then
    echo "Need 'cargo': install Rust from https://rustup.rs"
    exit 1
fi

# ---- 0. the version ----------------------------------------------------------
# The first `version = "x.y.z"` in Cargo.toml is the package's.
current_version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1; }
set_version() {
    awk -v v="$1" '!done && /^version = "/ { $0 = "version = \"" v "\""; done = 1 } 1' \
        Cargo.toml > Cargo.toml.new && mv Cargo.toml.new Cargo.toml
}
OLD_VERSION="$(current_version)"
if [ -n "$VUP" ]; then
    case "$OLD_VERSION" in
        *[!0-9.]* | *.*.*.* | "") echo "Can't bump \"$OLD_VERSION\": expected x.y.z" >&2; exit 1 ;;
    esac
    MAJOR="${OLD_VERSION%%.*}"
    REST="${OLD_VERSION#*.}"
    MINOR="${REST%%.*}"
    PATCH="${REST#*.}"
    case "$VUP" in
        major) NEW_VERSION="$((MAJOR + 1)).0.0" ;;
        minor) NEW_VERSION="$MAJOR.$((MINOR + 1)).0" ;;
        patch) NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))" ;;
    esac
    set_version "$NEW_VERSION"
    green "Version $OLD_VERSION -> $NEW_VERSION ($VUP)"
fi

blue "Building release binary…"
if ! cargo build --release --quiet; then
    # Don't leave a bumped version that was never built or installed.
    if [ -n "$VUP" ]; then
        set_version "$OLD_VERSION"
        echo "Build failed: version put back to $OLD_VERSION" >&2
    fi
    exit 1
fi
EXE=roxaruu
[ "$OS" = windows ] && EXE=roxaruu.exe
BIN="target/release/$EXE"
VERSION="$(current_version)"

# ---- 1. the commands ---------------------------------------------------------
if [ "$OS" = windows ]; then
    LOCAL="${LOCALAPPDATA:-$HOME/AppData/Local}"
    command -v cygpath >/dev/null 2>&1 && LOCAL="$(cygpath -u "$LOCAL")"
    BIN_DIR="${BIN_DIR:-$LOCAL/Programs/Roxaruu}"
else
    BIN_DIR="${BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
fi
mkdir -p "$BIN_DIR"
# Copy beside, then rename: replacing a running binary in place can crash it.
cp "$BIN" "$BIN_DIR/.$EXE.new" && mv -f "$BIN_DIR/.$EXE.new" "$BIN_DIR/$EXE"
green "Installed $BIN_DIR/$EXE"
# `rox`, the short name: a wrapper, not a link, so the process is still
# called roxaruu (which is how windows find each other).
if [ "$OS" = windows ]; then
    printf '@"%%~dp0roxaruu.exe" %%*\r\n' > "$BIN_DIR/rox.cmd"
    green "Installed $BIN_DIR/rox.cmd"
else
    printf '#!/bin/sh\nexec "%s/roxaruu" "$@"\n' "$BIN_DIR" > "$BIN_DIR/rox"
    chmod +x "$BIN_DIR/rox"
    green "Installed $BIN_DIR/rox"
fi
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: add $BIN_DIR to your PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

# ---- 2. the app, where the system lists apps ---------------------------------
case "$OS" in
linux)
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    mkdir -p "$DATA/applications" "$DATA/icons/hicolor/512x512/apps"
    cp assets/icon.png "$DATA/icons/hicolor/512x512/apps/roxaruu.png"
    cat > "$DATA/applications/roxaruu.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Roxaruu
GenericName=Code Editor
Comment=A keyboard-first editor with terminals
Exec="$BIN_DIR/roxaruu" %F
Icon=roxaruu
Terminal=false
Categories=Development;TextEditor;IDE;
StartupWMClass=roxaruu
Version=1.0
X-Roxaruu-Version=$VERSION
DESKTOP
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database "$DATA/applications" >/dev/null 2>&1 || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -q "$DATA/icons/hicolor" >/dev/null 2>&1 || true
    green "Installed $DATA/applications/roxaruu.desktop (your app menu)"
    echo "Open a folder:  rox     (the one you're in; or rox <path>, or Roxaruu from the app menu)"
    ;;

windows)
    START="${APPDATA:-$HOME/AppData/Roaming}/Microsoft/Windows/Start Menu/Programs"
    TARGET="$BIN_DIR/$EXE"
    if command -v cygpath >/dev/null 2>&1; then
        START="$(cygpath -w "$(cygpath -u "$START")")"
        TARGET="$(cygpath -w "$TARGET")"
    fi
    if powershell.exe -NoProfile -Command "\$s = (New-Object -ComObject WScript.Shell).CreateShortcut('$START\\Roxaruu.lnk'); \$s.TargetPath = '$TARGET'; \$s.Description = 'Roxaruu'; \$s.Save()" >/dev/null 2>&1; then
        green "Added Roxaruu to the Start Menu"
    else
        echo "Note: couldn't add a Start Menu shortcut; run $TARGET directly."
    fi
    echo "Open a folder:  rox     (the one you're in; or rox <path>, or Roxaruu from the Start Menu)"
    ;;

mac)
    if [ -z "${APP_DIR:-}" ]; then
        if [ -w /Applications ]; then APP_DIR=/Applications; else APP_DIR="$HOME/Applications"; fi
        REGISTER=1
    fi
    DEST="$APP_DIR/Roxaruu.app"
    mkdir -p "$APP_DIR"
    rm -rf "$DEST.new"
    mkdir -p "$DEST.new/Contents/MacOS" "$DEST.new/Contents/Resources"
    cp "$BIN" "$DEST.new/Contents/MacOS/roxaruu"
    cp assets/AppIcon.icns "$DEST.new/Contents/Resources/AppIcon.icns"
    cat > "$DEST.new/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>               <string>Roxaruu</string>
    <key>CFBundleDisplayName</key>        <string>Roxaruu</string>
    <key>CFBundleExecutable</key>         <string>roxaruu</string>
    <key>CFBundleIconFile</key>           <string>AppIcon</string>
    <key>CFBundleIdentifier</key>         <string>com.p32929.roxaruu</string>
    <key>CFBundlePackageType</key>        <string>APPL</string>
    <key>CFBundleVersion</key>            <string>$VERSION</string>
    <key>CFBundleShortVersionString</key> <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>     <string>10.15</string>
    <key>NSHighResolutionCapable</key>    <true/>
    <!-- Belt and braces with the activity the app holds: no App Nap. -->
    <key>NSAppSleepDisabled</key>         <true/>
    <key>LSApplicationCategoryType</key>  <string>public.app-category.developer-tools</string>
    <key>NSDocumentsFolderUsageDescription</key> <string>Roxaruu opens code projects stored in your Documents folder.</string>
    <key>NSDesktopFolderUsageDescription</key>   <string>Roxaruu opens code projects stored on your Desktop.</string>
    <key>NSDownloadsFolderUsageDescription</key> <string>Roxaruu opens code projects stored in your Downloads folder.</string>
</dict>
</plist>
PLIST

    # Unsigned: no key, no identity. (The binary keeps only the automatic
    # placeholder signature the linker gives every Apple Silicon program,
    # which macOS needs to run it at all.) Built on this Mac, it has no
    # quarantine flag, so it opens without a Gatekeeper prompt. Clear any
    # extended attributes the build picked up, too.
    xattr -cr "$DEST.new" 2>/dev/null || true

    # Update in place: same folder, new contents. Launchpad and the Apps
    # list keep track of the app by its folder; deleting and recreating it
    # each install made them lose it. Old signature files from earlier
    # (signed) installs go too, or macOS would call the app damaged.
    if [ -d "$DEST" ]; then
        rm -rf "$DEST/Contents"
        mv "$DEST.new/Contents" "$DEST/Contents"
        rmdir "$DEST.new"
    else
        mv "$DEST.new" "$DEST"
    fi
    touch "$DEST"
    if [ -n "${REGISTER:-}" ]; then
        # Tell macOS about it now rather than whenever it next looks:
        # LaunchServices (opening, the Dock), then Spotlight (search,
        # Launchpad, the Apps list all read Spotlight's index).
        /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f -R -trusted "$DEST" >/dev/null 2>&1 || true
        mdimport "$DEST" >/dev/null 2>&1 || true
        found=""
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            if mdfind -onlyin "$APP_DIR" 'kMDItemCFBundleIdentifier == "com.p32929.roxaruu"' 2>/dev/null | grep -q "$DEST"; then
                found=1
                break
            fi
            sleep 1
        done
        if [ -n "$found" ]; then
            green "Indexed: Roxaruu is in Spotlight, Launchpad and the Apps list"
        else
            echo "Note: Spotlight hasn't indexed it yet; it will shortly (or run: mdimport \"$DEST\")"
        fi
    fi
    green "Installed $DEST"
    echo "Open a folder:  rox     (the one you're in; or rox <path>, or Roxaruu from Spotlight)"
    ;;
esac
