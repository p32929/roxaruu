#!/bin/sh
# Run Roxaruu.
#
#     ./run.sh [folder|file]          install it (./install-local.sh) and open it
#                                     (no folder: the one you're in)
#     ./run.sh --temp [folder|file]   run this checkout straight from target/,
#                                     leaving the installed app and its windows alone
set -eu

case "${1:-}" in
    -h | --help)
        cat <<'HELP'
Run Roxaruu from this checkout.

Usage:
  ./run.sh [folder|file]          Install (./install-local.sh), then open it.
                                  No folder: the one you're in.
  ./run.sh --temp [folder|file]   Build and run this checkout straight from
                                  target/, installing nothing: the installed
                                  app and its open windows are left alone.
  ./run.sh -h | --help            Show this help.

Either way the window detaches: you get the prompt back, and closing the
terminal leaves the window open.

See also: ./install-local.sh --help, and roxaruu --help once installed.
HELP
        exit 0
        ;;
esac

TEMP=
if [ "${1:-}" = "--temp" ]; then TEMP=1; shift; fi
# Absolute, before we leave the caller's directory; none given means the
# folder you're in, as with `rox`.
if [ $# -gt 0 ]; then set -- "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; else set -- "$PWD"; fi
cd "$(dirname "$0")"

if [ -n "$TEMP" ]; then
    cargo build --release --quiet
    # The build's own binary, not the installed copy: running windows of
    # the installed app keep going untouched.
    exec target/release/roxaruu "$@"
fi

./install-local.sh
if [ "$(uname -s)" = "Darwin" ]; then
    APP="${APP_DIR:-$([ -w /Applications ] && echo /Applications || echo "$HOME/Applications")}/Roxaruu.app"
    if [ $# -gt 0 ]; then open -n "$APP" --args "$@"; else open -n "$APP"; fi
else
    exec "${BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}/roxaruu" "$@"
fi
