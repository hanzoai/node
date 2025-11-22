#!/bin/bash

# Yank old underscore versions (v1.1.10) to deprecate them
# This makes them unavailable for NEW projects but existing projects continue working

echo "Yanking old underscore versions (v1.1.10) from crates.io..."
echo "================================================================="

# List of all 24 hanzo crates with underscore names
CRATES=(
    "hanzo_message_primitives"
    "hanzo_crypto_identities"
    "hanzo_libp2p_relayer"
    "hanzo_job_queue_manager"
    "hanzo_fs"
    "hanzo_embedding"
    "hanzo_http_api"
    "hanzo_tools_primitives"
    "hanzo_tools_runner"
    "hanzo_sqlite"
    "hanzo_db"
    "hanzo_hmm"
    "hanzo_non_rust_code"
    "hanzo_mcp"
    "hanzo_pqc"
    "hanzo_kbs"
    "hanzo_did"
    "hanzo_model_discovery"
    "hanzo_config"
    "hanzo_mining"
    "hanzo_wasm_runtime"
    "hanzo_llm"
    "hanzo_sheet"
    "hanzo_runtime_tests"
)

VERSION="1.1.10"

for crate in "${CRATES[@]}"; do
    echo "Yanking $crate v$VERSION..."
    cargo yank --vers $VERSION $crate

    if [ $? -eq 0 ]; then
        echo "✅ Successfully yanked $crate v$VERSION"
    else
        echo "⚠️  Failed to yank $crate v$VERSION (may not exist or already yanked)"
    fi
    echo ""
done

echo "================================================================="
echo "Yanking complete!"
echo ""
echo "Next steps:"
echo "1. Fix missing metadata fields"
echo "2. Fix cargo feature issues"
echo "3. Publish new kebab-case versions (v1.1.11)"
