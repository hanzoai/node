#!/bin/bash

# Hanzo Crates Idiomatic Renaming Script v2
# Compatible with macOS bash 3.2
# Date: 2025-11-16

set -e  # Exit on error

echo "======================================================================"
echo "Hanzo Crates Idiomatic Renaming Script v2"
echo "======================================================================"
echo ""

# Define renaming pairs (old_name:new_name)
RENAMES="
hanzo-message-primitives:hanzo-messages
hanzo-crypto-identities:hanzo-identity
hanzo-libp2p-relayer:hanzo-libp2p
hanzo-job-queue-manager:hanzo-jobs
hanzo-fs:hanzo-fs
hanzo-embedding:hanzo-embed
hanzo-http-api:hanzo-api
hanzo-tools-primitives:hanzo-tools
hanzo-tools-runner:hanzo-runner
hanzo-sqlite:hanzo-db-sqlite
hanzo-db:hanzo-database
hanzo-hmm:hanzo-hmm
hanzo-non-rust-code:hanzo-runtime
hanzo-mcp:hanzo-mcp
hanzo-pqc:hanzo-pqc
hanzo-kbs:hanzo-kbs
hanzo-did:hanzo-did
hanzo-model-discovery:hanzo-models
hanzo-config:hanzo-config
hanzo-mining:hanzo-mining
hanzo-wasm-runtime:hanzo-wasm
hanzo-llm:hanzo-llm
hanzo-sheet:hanzo-sheet
hanzo-runtime-tests:hanzo-tests
"

# Phase 1: Rename [package] names
echo "Phase 1: Renaming [package] names in Cargo.toml files..."
echo "----------------------------------------------------------------------"

echo "$RENAMES" | while IFS=: read -r old_name new_name; do
    [ -z "$old_name" ] && continue

    crate_dir="hanzo-libs/$old_name"
    cargo_toml="$crate_dir/Cargo.toml"

    if [ -f "$cargo_toml" ]; then
        echo "Renaming: $old_name → $new_name"
        sed -i '' "s/^name = \"$old_name\"/name = \"$new_name\"/" "$cargo_toml"
    fi
done

# Phase 2: Update all dependency references
echo ""
echo "Phase 2: Updating dependency references..."
echo "----------------------------------------------------------------------"

find hanzo-libs -name "Cargo.toml" -type f | while read -r cargo_toml; do
    echo "Processing: $cargo_toml"

    echo "$RENAMES" | while IFS=: read -r old_name new_name; do
        [ -z "$old_name" ] && continue

        # Update dependency declarations
        sed -i '' "s/$old_name = { workspace = true }/$new_name = { workspace = true }/g" "$cargo_toml"
        sed -i '' "s/$old_name = { path/$new_name = { path/g" "$cargo_toml"
    done
done

# Phase 3: Update workspace configuration
echo ""
echo "Phase 3: Updating workspace configuration..."
echo "----------------------------------------------------------------------"

ROOT_CARGO="Cargo.toml"

if [ -f "$ROOT_CARGO" ]; then
    echo "Updating $ROOT_CARGO"

    echo "$RENAMES" | while IFS=: read -r old_name new_name; do
        [ -z "$old_name" ] && continue

        # Update workspace members
        sed -i '' "s/\"hanzo-libs\/$old_name\"/\"hanzo-libs\/$new_name\"/g" "$ROOT_CARGO"

        # Update workspace.dependencies paths
        sed -i '' "s/$old_name = { path = \".\/hanzo-libs\/$old_name/$new_name = { path = \".\/hanzo-libs\/$new_name/g" "$ROOT_CARGO"
    done
fi

# Phase 4: Rename directories
echo ""
echo "Phase 4: Renaming directories..."
echo "----------------------------------------------------------------------"

echo "$RENAMES" | while IFS=: read -r old_name new_name; do
    [ -z "$old_name" ] && continue

    old_dir="hanzo-libs/$old_name"
    new_dir="hanzo-libs/$new_name"

    if [ -d "$old_dir" ]; then
        echo "Renaming: $old_dir → $new_dir"
        mv "$old_dir" "$new_dir"
    fi
done

# Phase 5: Update path references
echo ""
echo "Phase 5: Updating path references..."
echo "----------------------------------------------------------------------"

find hanzo-libs -name "Cargo.toml" -type f | while read -r cargo_toml; do
    echo "$RENAMES" | while IFS=: read -r old_name new_name; do
        [ -z "$old_name" ] && continue

        sed -i '' "s/path = \"..\/hanzo-libs\/$old_name\"/path = \"..\/hanzo-libs\/$new_name\"/g" "$cargo_toml"
        sed -i '' "s/path = \".\/hanzo-libs\/$old_name\"/path = \".\/hanzo-libs\/$new_name\"/g" "$cargo_toml"
    done
done

echo ""
echo "======================================================================"
echo "Renaming Complete!"
echo "======================================================================"
echo ""
echo "Next: cargo check --workspace"
echo ""
