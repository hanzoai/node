#!/bin/bash
for dir in hanzo-libs/*; do
  if [ -f "$dir/Cargo.toml" ]; then
    deps=$(grep -c 'hanzo-[a-z-]* = {' "$dir/Cargo.toml" 2>/dev/null || echo "0")
    printf "%s: %d\n" "$(basename $dir)" "$deps"
  fi
done | sort -t':' -k2,2n
