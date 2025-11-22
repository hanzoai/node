#!/bin/bash

echo "========================================================================="
echo "Fixing Workspace Dependencies - Replace version numbers with { workspace = true }"
echo "========================================================================="

# Find all Cargo.toml files in hanzo-libs
for toml in hanzo-libs/*/Cargo.toml; do
  echo "Processing: $toml"
  
  # Replace hanzo-* = "1.1.XX" with hanzo-* = { workspace = true }
  sed -i '' -E 's/^(hanzo-[a-z-]+) = "1\.1\.[0-9]+"/\1 = { workspace = true }/' "$toml"
  
  # Also catch any remaining hanzo_* patterns
  sed -i '' -E 's/^(hanzo_[a-z_-]+) = "1\.1\.[0-9]+"/\1 = { workspace = true }/' "$toml"
done

echo "========================================================================="
echo "All workspace dependencies updated!"
echo "========================================================================="
