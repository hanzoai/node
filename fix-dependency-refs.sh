#!/bin/bash

# Fix all dependency references to use hyphen names
set -e

echo "Fixing all dependency references to use hyphens..."

# Find all Cargo.toml files (excluding target directories)
find hanzo-libs -name "Cargo.toml" -type f -not -path "*/target/*" | while read -r toml; do
  echo "Processing: $toml"

  # Replace all hanzo_* dependency keys with hanzo-*
  # Pattern: hanzo_whatever = { path = ...
  sed -i.bak2 -E 's/^hanzo_([a-z_-]+)\s*=/hanzo-\1 =/' "$toml"

  # Also fix in features sections where dependencies are referenced
  # Pattern: hanzo_whatever/feature
  sed -i.bak2 -E 's/"hanzo_([a-z_-]+)\//\"hanzo-\1\//g' "$toml"
done

# Fix root Cargo.toml workspace dependencies
if [ -f "Cargo.toml" ]; then
  echo "Processing: Cargo.toml (root)"
  sed -i.bak2 -E 's/^hanzo_([a-z_-]+)\s*=/hanzo-\1 =/' "Cargo.toml"
fi

# Clean up backup files
find . -name "*.bak2" -type f -delete

echo "Done! All dependency references now use hyphens."
