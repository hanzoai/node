#!/bin/bash

# Prepare Hanzo crates for publishing to crates.io

set -e

echo "Preparing Hanzo crates for publishing..."

# Add necessary metadata to workspace Cargo.toml
cat >> Cargo.toml << 'EOF'

[workspace.package.metadata]
description = "Hanzo AI Node - Distributed AI Infrastructure"
repository = "https://github.com/hanzoai/hanzo-node"
license = "MIT OR Apache-2.0"
homepage = "https://hanzo.ai"
documentation = "https://docs.hanzo.ai"
keywords = ["ai", "distributed", "blockchain", "pqc", "mcp"]
categories = ["network-programming", "cryptography", "web-programming"]
EOF

# List of crates in dependency order
CRATES=(
    "hanzo-libs/hanzo-message-primitives"
    "hanzo-libs/hanzo-crypto-identities"
    "hanzo-libs/hanzo-fs"
    "hanzo-libs/hanzo-embedding"
    "hanzo-libs/hanzo-sqlite"
    "hanzo-libs/hanzo-non-rust-code"
    "hanzo-libs/hanzo-mcp"
    "hanzo-libs/hanzo-pqc"
    "hanzo-libs/hanzo-kbs"
    "hanzo-libs/hanzo-tools-primitives"
    "hanzo-libs/hanzo-job-queue-manager"
    "hanzo-libs/hanzo-http-api"
    "hanzo-libs/hanzo-libp2p-relayer"
    "hanzo-test-framework"
    "hanzo-test-macro"
    "hanzo-bin/hanzo-node"
)

# Add metadata to each crate
for crate in "${CRATES[@]}"; do
    if [ -f "$crate/Cargo.toml" ]; then
        echo "Processing $crate..."
        
        # Check if metadata already exists
        if ! grep -q "\[package.metadata\]" "$crate/Cargo.toml"; then
            # Add package metadata section
            cat >> "$crate/Cargo.toml" << 'EOF'

[package.metadata]
description = { workspace = true }
repository = { workspace = true }
license = { workspace = true }
homepage = { workspace = true }
documentation = { workspace = true }
keywords = { workspace = true }
categories = { workspace = true }
EOF
        fi
    fi
done

echo "Crates prepared for publishing!"
echo ""
echo "To publish, run:"
echo "  ./publish-crates.sh"