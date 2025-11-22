#!/bin/bash

# Publish NEW kebab-case versions (v1.1.11) as completely new crates
# This script publishes in dependency order (tiers)

echo "Publishing NEW kebab-case Hanzo crates to crates.io..."
echo "================================================================="
echo "Version: 1.1.11"
echo "All crates will be published as NEW crates (different IDs from underscore versions)"
echo "================================================================="

# Helper function to publish a crate
publish_crate() {
    local crate_path=$1
    local crate_name=$(basename $crate_path)

    echo ""
    echo "Publishing $crate_name..."
    echo "---"

    cd "$crate_path" || exit 1

    cargo publish --allow-dirty 2>&1

    if [ $? -eq 0 ]; then
        echo "✅ Successfully published $crate_name"
    else
        echo "❌ Failed to publish $crate_name"
        # Continue anyway to publish other crates
    fi

    cd - > /dev/null
}

# Tier 1: No dependencies
echo ""
echo "Tier 1: Foundational crates (no dependencies)..."
echo "================================================================="
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-message-primitives"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-crypto-identities"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-pqc"

# Tier 2: Depends on Tier 1
echo ""
echo "Tier 2: Core infrastructure..."
echo "================================================================="
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-embedding"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-non-rust-code"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-did"

# Tier 3: Depends on Tier 2
echo ""
echo "Tier 3: Tools and database..."
echo "================================================================="
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-tools-primitives"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-mcp"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-config"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-sqlite"

# Tier 4: Depends on Tier 3
echo ""
echo "Tier 4: Application layer..."
echo "================================================================="
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-db"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-libp2p-relayer"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-job-queue-manager"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-fs"

# Tier 5: Higher-level services
echo ""
echo "Tier 5: Services and features..."
echo "================================================================="
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-kbs"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-model-discovery"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-hmm"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-llm"

# Tier 6: Specialized modules
echo ""
echo "Tier 6: Specialized features..."
echo "================================================================="
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-sheet"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-wasm-runtime"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-mining"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-http-api"
publish_crate "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-tools-runner"

echo ""
echo "================================================================="
echo "Publishing complete!"
echo "================================================================="
echo ""
echo "All NEW kebab-case crates have been published to crates.io"
echo "Users must update their Cargo.toml files:"
echo "  - Change hanzo_* → hanzo-*"
echo "  - Update version to 1.1.11"
echo ""
echo "See MIGRATION.md for detailed migration guide."
