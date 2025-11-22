#!/bin/bash

# Hanzo Crates Idiomatic Renaming Script
# Renames all 24 crates from hanzo_* to idiomatic hanzo-* names
# Date: 2025-11-16

set -e  # Exit on error

echo "======================================================================"
echo "Hanzo Crates Idiomatic Renaming Script"
echo "======================================================================"
echo ""
echo "This script will:"
echo "1. Rename all [package] names in Cargo.toml files"
echo "2. Update all dependency references"
echo "3. Update workspace configuration"
echo "4. Rename directory structures"
echo ""
echo "WARNING: This makes significant changes to the repository!"
echo ""
read -p "Press ENTER to continue or Ctrl+C to abort..."

# Define renaming map (old_name → new_name)
declare -A RENAME_MAP=(
    ["hanzo-message-primitives"]="hanzo-messages"
    ["hanzo-crypto-identities"]="hanzo-identity"
    ["hanzo-libp2p-relayer"]="hanzo-libp2p"
    ["hanzo-job-queue-manager"]="hanzo-jobs"
    ["hanzo-fs"]="hanzo-fs"
    ["hanzo-embedding"]="hanzo-embed"
    ["hanzo-http-api"]="hanzo-api"
    ["hanzo-tools-primitives"]="hanzo-tools"
    ["hanzo-tools-runner"]="hanzo-runner"
    ["hanzo-sqlite"]="hanzo-db-sqlite"
    ["hanzo-db"]="hanzo-database"
    ["hanzo-hmm"]="hanzo-hmm"
    ["hanzo-non-rust-code"]="hanzo-runtime"
    ["hanzo-mcp"]="hanzo-mcp"
    ["hanzo-pqc"]="hanzo-pqc"
    ["hanzo-kbs"]="hanzo-kbs"
    ["hanzo-did"]="hanzo-did"
    ["hanzo-model-discovery"]="hanzo-models"
    ["hanzo-config"]="hanzo-config"
    ["hanzo-mining"]="hanzo-mining"
    ["hanzo-wasm-runtime"]="hanzo-wasm"
    ["hanzo-llm"]="hanzo-llm"
    ["hanzo-sheet"]="hanzo-sheet"
    ["hanzo-runtime-tests"]="hanzo-tests"
)

# Phase 1: Rename [package] names in Cargo.toml files
echo ""
echo "Phase 1: Renaming [package] names in Cargo.toml files..."
echo "----------------------------------------------------------------------"

for old_name in "${!RENAME_MAP[@]}"; do
    new_name="${RENAME_MAP[$old_name]}"
    crate_dir="hanzo-libs/$old_name"
    cargo_toml="$crate_dir/Cargo.toml"

    if [ -f "$cargo_toml" ]; then
        echo "Renaming: $old_name → $new_name"
        # Update [package] name field
        sed -i '' "s/^name = \"$old_name\"/name = \"$new_name\"/" "$cargo_toml"
    else
        echo "WARNING: $cargo_toml not found"
    fi
done

# Phase 2: Update all dependency references
echo ""
echo "Phase 2: Updating all dependency references..."
echo "----------------------------------------------------------------------"

# Find all Cargo.toml files
find hanzo-libs -name "Cargo.toml" -type f | while read -r cargo_toml; do
    echo "Processing: $cargo_toml"

    # Update each dependency reference
    for old_name in "${!RENAME_MAP[@]}"; do
        new_name="${RENAME_MAP[$old_name]}"

        # Replace dependency declarations
        # Pattern 1: hanzo-old-name = { workspace = true }
        sed -i '' "s/$old_name = { workspace = true }/$new_name = { workspace = true }/g" "$cargo_toml"

        # Pattern 2: hanzo-old-name = { path = "..." }
        sed -i '' "s/$old_name = { path/$new_name = { path/g" "$cargo_toml"

        # Pattern 3: hanzo-old-name = { version = "..." }
        sed -i '' "s/$old_name = { version/$new_name = { version/g" "$cargo_toml"

        # Pattern 4: hanzo-old-name = "..."
        sed -i '' "s/$old_name = \"/$new_name = \"/g" "$cargo_toml"
    done
done

# Phase 3: Update workspace configuration (root Cargo.toml)
echo ""
echo "Phase 3: Updating workspace configuration..."
echo "----------------------------------------------------------------------"

ROOT_CARGO="Cargo.toml"

if [ -f "$ROOT_CARGO" ]; then
    echo "Updating workspace members and dependencies in $ROOT_CARGO"

    # Update workspace members
    for old_name in "${!RENAME_MAP[@]}"; do
        new_name="${RENAME_MAP[$old_name]}"
        sed -i '' "s/\"hanzo-libs\/$old_name\"/\"hanzo-libs\/$new_name\"/g" "$ROOT_CARGO"
    done

    # Update workspace.dependencies
    for old_name in "${!RENAME_MAP[@]}"; do
        new_name="${RENAME_MAP[$old_name]}"

        # Update dependency name
        sed -i '' "s/$old_name = { path = \".\/hanzo-libs\/$old_name/$new_name = { path = \".\/hanzo-libs\/$new_name/g" "$ROOT_CARGO"
    done
fi

# Phase 4: Rename directories
echo ""
echo "Phase 4: Renaming directory structures..."
echo "----------------------------------------------------------------------"

for old_name in "${!RENAME_MAP[@]}"; do
    new_name="${RENAME_MAP[$old_name]}"
    old_dir="hanzo-libs/$old_name"
    new_dir="hanzo-libs/$new_name"

    if [ -d "$old_dir" ]; then
        echo "Renaming directory: $old_dir → $new_dir"
        mv "$old_dir" "$new_dir"
    else
        echo "WARNING: Directory $old_dir not found"
    fi
done

# Phase 5: Update path references in Cargo.toml files (after directory rename)
echo ""
echo "Phase 5: Updating path references after directory rename..."
echo "----------------------------------------------------------------------"

find hanzo-libs -name "Cargo.toml" -type f | while read -r cargo_toml; do
    for old_name in "${!RENAME_MAP[@]}"; do
        new_name="${RENAME_MAP[$old_name]}"

        # Update path references
        sed -i '' "s/path = \"..\/hanzo-libs\/$old_name\"/path = \"..\/hanzo-libs\/$new_name\"/g" "$cargo_toml"
        sed -i '' "s/path = \".\/hanzo-libs\/$old_name\"/path = \".\/hanzo-libs\/$new_name\"/g" "$cargo_toml"
    done
done

echo ""
echo "======================================================================"
echo "Renaming Complete!"
echo "======================================================================"
echo ""
echo "Summary:"
echo "- Renamed 24 [package] names to idiomatic kebab-case"
echo "- Updated all dependency references"
echo "- Updated workspace configuration"
echo "- Renamed all directory structures"
echo ""
echo "Next Steps:"
echo "1. Run: cargo check --workspace"
echo "2. Fix any compilation errors"
echo "3. Run: cargo test --workspace"
echo "4. Commit changes"
echo "5. Publish crates in dependency order"
echo ""
