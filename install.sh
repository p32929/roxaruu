#!/bin/sh
# Install Roxaruu from its latest release (or ROXARUU_VERSION=0.2.50):
#
#     curl -fsSL https://raw.githubusercontent.com/p32929/roxaruu/master/install.sh | sh
#
#   macOS    Roxaruu.app in /Applications (~/Applications if that isn't
#            writable), the disk image for this Mac's chip, and `rox`.
#   Linux    the .deb through apt where there is one (asks for your password),
#            else the .tar.xz into ~/.local (no password), with a menu entry.
#   Windows  (Git Bash) the installer, for this user, without questions.
#
# Overrides, for testing: ROXARUU_DOWNLOADS (where the files are, instead of
# the release's page), ROXARUU_APP_DIR and ROXARUU_BIN_DIR (macOS),
# ROXARUU_PREFIX (Linux .tar.xz).
set -eu

REPO="p32929/roxaruu"
say() { printf '%s\n' "$*"; }
fail() { printf 'roxaruu install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs $1"; }
need curl

# Which version: asked for, or the latest release's.
version="${ROXARUU_VERSION:-}"
if [ -z "$version" ]; then
    latest=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null) ||
        fail "no release found (none published yet, or GitHub can't be reached)"
    version=$(printf '%s\n' "$latest" | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$version" ] || fail "no release found"
fi
version="${version#v}"
downloads="${ROXARUU_DOWNLOADS:-https://github.com/$REPO/releases/download/v$version}"

# This computer: which file, and how it goes in.
os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Darwin)
        case "$arch" in
            arm64) file="roxaruu_v${version}_macos_apple-silicon.dmg" ;;
            x86_64) file="roxaruu_v${version}_macos_intel.dmg" ;;
            *) fail "no build for this Mac ($arch)" ;;
        esac ;;
    Linux)
        [ "$arch" = x86_64 ] || fail "no Linux build for $arch yet (x86-64 only)"
        if command -v apt-get >/dev/null 2>&1 && { [ "$(id -u)" = 0 ] || command -v sudo >/dev/null 2>&1; } && [ -z "${ROXARUU_PREFIX:-}" ]; then
            file="roxaruu_v${version}_linux_x64.deb"
        else
            file="roxaruu_v${version}_linux_x64.tar.xz"
        fi ;;
    MINGW* | MSYS* | CYGWIN*)
        file="roxaruu_v${version}_windows_x64_setup.exe" ;;
    *) fail "no build for $os" ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
say "Downloading Roxaruu $version ($file)…"
case "$downloads" in
    http*) curl -fL --progress-bar -o "$tmp/$file" "$downloads/$file" || fail "couldn't download $downloads/$file" ;;
    *) cp "$downloads/$file" "$tmp/$file" || fail "no $downloads/$file" ;;
esac

case "$file" in
    *.dmg)
        apps="${ROXARUU_APP_DIR:-/Applications}"
        { mkdir -p "$apps" 2>/dev/null && [ -w "$apps" ]; } || apps="$HOME/Applications"
        mkdir -p "$apps"
        mount="$tmp/mount"
        hdiutil attach -nobrowse -quiet -mountpoint "$mount" "$tmp/$file" || fail "couldn't open the disk image"
        rm -rf "$apps/Roxaruu.app"
        cp -R "$mount/Roxaruu.app" "$apps/" || { hdiutil detach -quiet "$mount"; fail "couldn't copy Roxaruu.app to $apps"; }
        hdiutil detach -quiet "$mount" || true
        # Not notarized by Apple yet: let it open without the "unidentified
        # developer" refusal (this is what you'd allow by hand).
        xattr -dr com.apple.quarantine "$apps/Roxaruu.app" 2>/dev/null || true
        # `rox` and `roxaruu` in a folder on the PATH.
        bin="${ROXARUU_BIN_DIR:-/usr/local/bin}"
        [ -w "$bin" ] || { bin="$HOME/.local/bin"; mkdir -p "$bin"; }
        for name in rox roxaruu; do
            ln -sf "$apps/Roxaruu.app/Contents/MacOS/roxaruu" "$bin/$name"
        done
        say "Installed $apps/Roxaruu.app (Spotlight, Launchpad) and rox in $bin." ;;
    *.deb)
        sudo_=""
        [ "$(id -u)" = 0 ] || sudo_=sudo
        say "Installing the package (apt asks for your password)…"
        $sudo_ apt-get install -y "$tmp/$file" >/dev/null || fail "apt couldn't install $file"
        bin=/usr/bin
        say "Installed Roxaruu (the app menu) and rox." ;;
    *.tar.xz)
        need tar
        prefix="${ROXARUU_PREFIX:-$HOME/.local}"
        mkdir -p "$prefix"
        tar -xJf "$tmp/$file" -C "$prefix" || fail "couldn't unpack $file"
        bin="$prefix/bin"
        # The menu entry runs this copy, not /usr/bin's.
        desktop="$prefix/share/applications/roxaruu.desktop"
        [ -f "$desktop" ] && sed -i "s|/usr/bin/roxaruu|$bin/roxaruu|" "$desktop"
        cat > "$bin/rox" <<EOF
#!/bin/sh
exec "$bin/roxaruu" "\$@"
EOF
        chmod 755 "$bin/rox"
        say "Installed into $prefix (the app menu) and rox in $bin." ;;
    *.exe)
        say "Running the installer (for you only, no questions)…"
        "$tmp/$file" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER || fail "the installer failed"
        bin="$LOCALAPPDATA/Programs/Roxaruu"
        say "Installed Roxaruu (the Start Menu) and rox in $bin." ;;
esac

case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "Add $bin to your PATH to use rox from a terminal." ;;
esac
say "Open a folder: rox     (the one you're in; or rox <path>, or Roxaruu from your apps)"
