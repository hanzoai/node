#!/bin/bash
set -e

echo "📦 Publishing hanzo-crypto-identities"
cd hanzo-libs/hanzo-crypto-identities

# Create fixed Cargo.toml
cat > Cargo.toml.fixed << 'EOF'
[package]
name = "hanzo_crypto_identities"
version = "1.1.10"
edition = "2021"
authors = ["Hanzo AI Inc"]
license = "MIT"
repository = "https://github.com/hanzoai/hanzo-node"
homepage = "https://hanzo.ai"
description = "Cryptographic identities for Hanzo AI platform"

[dependencies]
tokio = { version = "1.36", features = ["rt", "rt-multi-thread", "macros", "fs", "io-util", "net", "sync", "time"] }
serde_json = "1.0.117"
hanzo_message_primitives = "1.1.10"
x25519-dalek = { version = "2.0.1", features = ["static_secrets"] }
ed25519-dalek = { version = "2.1.1", features = ["rand_core"] }
chrono = { version = "0.4", features = ["serde"] }
dashmap = "6.0"
lazy_static = "1.4.0"
trust-dns-resolver = "0.23.2"
hanzo_non_rust_code = "1.1.10"
tempfile = "3.8"
EOF

# Backup and use fixed version
cp Cargo.toml Cargo.toml.backup
cp Cargo.toml.fixed Cargo.toml

echo "📤 Publishing to crates.io..."
cargo publish --allow-dirty 2>&1 || {
    echo "❌ Failed"
    mv Cargo.toml.backup Cargo.toml
    rm Cargo.toml.fixed
    exit 1
}

# Restore
mv Cargo.toml.backup Cargo.toml
rm Cargo.toml.fixed

echo "✅ Published hanzo-crypto-identities"
