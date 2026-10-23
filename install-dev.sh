#!/bin/sh
# Symlink ~/.local/bin/{claude-token-usage,ctu} at target/release/claude-token-usage,
# so a `cargo build --release` is picked up immediately.
#
# Note: `cargo clean` deletes the target, which leaves both links dangling.
# Re-run this script after cleaning.
set -eu

BINDIR="${BINDIR:-$HOME/.local/bin}"
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/target/release/claude-token-usage"

command -v cargo >/dev/null 2>&1 || { echo "install-dev: cargo not found" >&2; exit 1; }
[ -x "$BIN" ] || { echo "building release first ..."; ( cd "$HERE" && cargo build --release ); }

mkdir -p "$BINDIR"
ln -sf "$BIN" "$BINDIR/claude-token-usage"
ln -sf "$BIN" "$BINDIR/ctu"

echo "linked (dev):"
echo "  $BINDIR/claude-token-usage -> $BIN"
echo "  $BINDIR/ctu                -> $BIN"

case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *) echo ""; echo "note: $BINDIR is not on your PATH. Add it:"; echo "  export PATH=\"$BINDIR:\$PATH\"" ;;
esac
