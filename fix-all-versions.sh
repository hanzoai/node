#!/bin/bash

# Fix ALL dependency version issues in hanzo-http-api and other crates
cd /Users/z/work/shinkai/hanzo-node

echo "Fixing dependency versions..."

# hanzo-http-api fixes
cd hanzo-libs/hanzo-http-api
sed -i.bak 's/warp = { version = "[^"]*"/warp = { version = "0.3"/' Cargo.toml
sed -i.bak 's/tokio-util = { version = "[^"]*"/tokio-util = { version = "0.7"/' Cargo.toml
sed -i.bak 's/rmcp = { version = "[^"]*"/rmcp = { version = "0.8"/' Cargo.toml
cd ../..

# Check for other version issues
echo "Checking all crates for version issues..."

# Find all dependencies with versions >= 1.0.0 that might not exist
for dir in hanzo-libs/*; do
  if [ -f "$dir/Cargo.toml" ]; then
    echo "Checking $dir..."
    # Common problematic dependencies
    cd "$dir"
    # Fix any tokio-util >= 1.0
    sed -i.bak 's/tokio-util = { version = "[1-9][^"]*"/tokio-util = { version = "0.7"/' Cargo.toml
    sed -i.bak 's/tokio-util = "[1-9][^"]*"/tokio-util = "0.7"/' Cargo.toml

    # Fix any warp >= 1.0
    sed -i.bak 's/warp = { version = "[1-9][^"]*"/warp = { version = "0.3"/' Cargo.toml
    sed -i.bak 's/warp = "[1-9][^"]*"/warp = "0.3"/' Cargo.toml

    # Fix rmcp versions
    sed -i.bak 's/rmcp = { version = "[1-9][^"]*"/rmcp = { version = "0.8"/' Cargo.toml
    sed -i.bak 's/rmcp = "[1-9][^"]*"/rmcp = "0.8"/' Cargo.toml
    cd ..
  fi
done

cd ..

echo "All version fixes applied!"