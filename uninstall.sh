#!/bin/sh
# Remove claude-token-usage and the ctu alias. Build artifacts are left alone.
set -eu

BINDIR="${BINDIR:-$HOME/.local/bin}"
removed=0

for name in claude-token-usage ctu; do
  target="$BINDIR/$name"
  if [ -L "$target" ] || [ -f "$target" ]; then
    rm -f "$target"
    echo "removed $target"
    removed=$((removed+1))
  fi
done

if [ "$removed" = 0 ]; then
  echo "nothing to remove in $BINDIR"
else
  echo "removed $removed file(s)"
fi
