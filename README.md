# Roxaruu

A keyboard-first code editor with built-in terminals. Nothing else to
install: it works out of the box on each system. Every action has a key,
and the keys that work where you are are always on screen. Runs on macOS,
Windows and Linux; open its windows from a phone's browser with remote access.

## Download

Get the file for your system from the
[latest release](https://github.com/p32929/roxaruu/releases/latest).

### macOS (Apple Silicon and Intel)

1. Open `Roxaruu-<version>-macos.dmg` and drag **Roxaruu** into **Applications**.
2. The first time, right-click Roxaruu in Applications and choose **Open**, then
   **Open** again (it isn't notarized by Apple yet, so a plain double-click is
   refused). Or, in a terminal: `xattr -cr /Applications/Roxaruu.app`.

### Windows (10 and 11, 64-bit)

1. Run `Roxaruu-<version>-windows-x64-setup.exe`. It installs for you only (no
   admin password), adds Roxaruu to the Start Menu, the `rox` command, and
   **Open with Roxaruu** to File Explorer's right-click menu.
2. If Windows says "Windows protected your PC", choose **More info → Run
   anyway** (the installer isn't signed yet).

Or unzip `Roxaruu-<version>-windows-x64.zip` anywhere and run `roxaruu.exe`.

Terminals run Git Bash, so install [Git for Windows](https://git-scm.com/downloads/win)
if you haven't (Roxaruu offers the link when you open a terminal without it).
If you use WSL, pick it in **Settings → Terminal shell**; without Git Bash,
terminals use WSL by themselves.

### Linux (64-bit)

- Debian, Ubuntu, Mint, Pop!_OS: `sudo apt install ./roxaruu_<version>_amd64.deb`
- Anything else: unpack `Roxaruu-<version>-linux-x64.tar.gz`; the program is
  `bin/roxaruu` (copy `share/` into `~/.local/share/` for the app-menu entry).

## Use

Open a folder from the app, or from a terminal: `rox` (the folder you're in)
or `rox <path>`. Press **F1** for every shortcut, **⇧⌘P** / **Ctrl+Shift+P**
for every command.

Use it from a browser (another computer, a phone):
`rox --remote on --password <password> [folder]` prints the address to open;
`rox --remote off` stops it. On a server with no screen it runs on a virtual
one (needs Xvfb: `sudo apt install xvfb`). The password is kept in memory only.

## Build from source

Needs [Rust](https://rustup.rs). `./install-local.sh` builds and installs it
on macOS and Linux (and on Windows from Git Bash); `cargo run --release` runs
it from the checkout. Linux also needs: `libxkbcommon-dev libwayland-dev
libgtk-3-dev libdbus-1-dev`.

## Releases

Pushing a tag like `v0.3.0` builds and publishes the downloads above
(`.github/workflows/build.yml`).
