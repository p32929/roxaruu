#!/bin/sh
# Run Roxaruu.
#
#     ./run.sh [folder|file]          install it (./install-local.sh) and open it
#     ./run.sh --temp [folder|file]   run this checkout straight from target/,
#                                     leaving the installed app and its windows alone
set -eu

TEMP=
if [ "${1:-}" = "--temp" ]; then TEMP=1; shift; fi
# Absolute, before we leave the caller's directory.
if [ $# -gt 0 ]; then set -- "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; fi
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
