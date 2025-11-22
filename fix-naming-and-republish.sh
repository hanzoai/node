#!/bin/bash

# Fix all hanzo crate names to use kebab-case (hyphens) instead of snake_case (underscores)
# As requested by the user

set -e

CRATES=(
  "hanzo-baml"
  "hanzo-config"
  "hanzo-crypto-identities"
  "hanzo-db"
  "hanzo-did"
  "hanzo-embedding"
  "hanzo-fs"
  "hanzo-hmm"
  "hanzo-http-api"
  "hanzo-job-queue-manager"
  "hanzo-libp2p-relayer"
  "hanzo-llm"
  "hanzo-mcp"
  "hanzo-message-primitives"
  "hanzo-mining"
  "hanzo-model-discovery"
  "hanzo-non-rust-code"
  "hanzo-pqc"
  "hanzo-sheet"
  "hanzo-sqlite"
  "hanzo-tools-primitives"
  "hanzo-tools-runner"
  "hanzo-wasm-runtime"
)

echo "=================================================="
echo "FIXING HANZO CRATE NAMING TO USE KEBAB-CASE"
echo "=================================================="
echo ""

# Step 1: Fix all Cargo.toml name fields
echo "Step 1: Updating Cargo.toml name fields to use hyphens..."
for crate in "${CRATES[@]}"; do
  TOML_PATH="hanzo-libs/${crate}/Cargo.toml"
  if [ -f "$TOML_PATH" ]; then
    UNDERSCORE_NAME="${crate//-/_}"
    echo "  Fixing $TOML_PATH: hanzo_* → hanzo-*"
    sed -i.bak "s/^name = \"$UNDERSCORE_NAME\"/name = \"$crate\"/" "$TOML_PATH"
  fi
done

# Step 2: Update all dependency references in Cargo.toml files
echo ""
echo "Step 2: Updating dependency references..."
find hanzo-libs -name "Cargo.toml" -type f -not -path "*/target/*" | while read -r toml; do
  echo "  Processing $toml"
  for crate in "${CRATES[@]}"; do
    UNDERSCORE_NAME="${crate//-/_}"
    # Update workspace dependencies
    sed -i.bak "s/\\b$UNDERSCORE_NAME\\s*=/\"$crate\" =/" "$toml"
    # Update regular dependencies
    sed -i.bak "s/\\[$UNDERSCORE_NAME\\]/[$crate]/" "$toml"
  done
done

# Step 3: Update root Cargo.toml workspace members
echo ""
echo "Step 3: Updating workspace Cargo.toml..."
ROOT_TOML="Cargo.toml"
if [ -f "$ROOT_TOML" ]; then
  for crate in "${CRATES[@]}"; do
    UNDERSCORE_NAME="${crate//-/_}"
    sed -i.bak "s/\\b$UNDERSCORE_NAME\\s*=/\"$crate\" =/" "$ROOT_TOML"
  done
fi

# Step 4: Bump versions to 1.1.11
echo ""
echo "Step 4: Bumping versions to 1.1.11..."
for crate in "${CRATES[@]}"; do
  TOML_PATH="hanzo-libs/${crate}/Cargo.toml"
  if [ -f "$TOML_PATH" ]; then
    sed -i.bak 's/^version = "1\.1\.10"/version = "1.1.11"/' "$TOML_PATH"
  fi
done

# Update workspace version in root Cargo.toml
if [ -f "$ROOT_TOML" ]; then
  sed -i.bak 's/^version = "1\.1\.10"/version = "1.1.11"/' "$ROOT_TOML"
fi

# Step 5: Clean up backup files
echo ""
echo "Step 5: Cleaning up backup files..."
find . -name "*.bak" -type f -delete

echo ""
echo "=================================================="
echo "NAMING FIXES COMPLETE"
echo "=================================================="
echo ""
echo "All crate names now use kebab-case (hyphens) as requested."
echo "All dependency references updated."
echo "Version bumped to 1.1.11."
echo ""
echo "Ready to publish. Run cargo build first to verify."
