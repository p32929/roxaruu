#!/bin/sh
# Build this checkout in release mode and install it:
#   1. the `roxaruu` and `rox` commands into ~/.cargo/bin (`rox` opens the
#      folder you're in, `rox path` another one), and
#   2. on macOS, Roxaruu.app (Launchpad, Spotlight, the Dock).
#
#     ./install-local.sh
#
# Re-run it to update. It never touches Oxru: different command, app and
# bundle id. BIN_DIR / APP_DIR override where things go; SIGN_ID picks the
# signing identity ("-" for ad-hoc).
set -eu
cd "$(dirname "$0")"

green() { printf '\033[0;32m%s\033[0m\n' "$1"; }
blue()  { printf '\033[0;34m%s\033[0m\n' "$1"; }

if ! command -v cargo >/dev/null 2>&1; then
    echo "Need 'cargo': install Rust from https://rustup.rs"
    exit 1
fi

blue "Building release binary…"
cargo build --release --quiet
BIN="target/release/roxaruu"

# ---- 1. the command ---------------------------------------------------------
BIN_DIR="${BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
mkdir -p "$BIN_DIR"
# Copy beside, then rename: replacing a running binary in place can crash it.
cp "$BIN" "$BIN_DIR/.roxaruu.new" && mv -f "$BIN_DIR/.roxaruu.new" "$BIN_DIR/roxaruu"
green "Installed $BIN_DIR/roxaruu"
# `rox`, the short name: a wrapper, not a link, so the process is still
# called roxaruu (which is how windows find each other).
printf '#!/bin/sh\nexec "%s/roxaruu" "$@"\n' "$BIN_DIR" > "$BIN_DIR/rox"
chmod +x "$BIN_DIR/rox"
green "Installed $BIN_DIR/rox"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: add $BIN_DIR to your PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

[ "$(uname -s)" = "Darwin" ] || exit 0

# ---- 2. Roxaruu.app ----------------------------------------------------------
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
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
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

# macOS remembers "Allow" for Documents etc. against the signing identity. An
# ad-hoc signature changes every build, so it would ask again after each
# install; a real identity (even the free Apple Development one) sticks.
SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning 2>/dev/null | awk -F'"' '/Developer ID Application|Apple Development/ {print $2; exit}')}"
if [ -n "$SIGN_ID" ] && [ "$SIGN_ID" != "-" ] && codesign --force --sign "$SIGN_ID" "$DEST.new" >/dev/null 2>&1; then
    green "Signed with $SIGN_ID"
else
    codesign --force --sign - "$DEST.new" >/dev/null 2>&1 || true
    echo "Note: signed ad-hoc, so macOS will ask for folder access again after each install."
fi

rm -rf "$DEST"
mv "$DEST.new" "$DEST"
if [ -n "${REGISTER:-}" ]; then
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST" >/dev/null 2>&1 || true
fi
green "Installed $DEST"
echo "Open a folder:  rox     (the one you're in; or rox <path>, or Roxaruu from Spotlight)"
