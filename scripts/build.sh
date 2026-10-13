#!/bin/sh
set -eu

for candidate in "$(command -v cargo 2>/dev/null || true)" \
    "${CARGO_HOME:-$HOME/.cargo}/bin/cargo" \
    /opt/homebrew/opt/rustup/bin/cargo \
    /opt/homebrew/bin/cargo \
    /usr/local/bin/cargo
do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
        PATH="$(dirname "$candidate"):$PATH" exec "$candidate" build --release
    fi
done

echo "herdr-jest: cargo not found. Install Rust (https://rustup.rs) and rebuild." >&2
exit 1
