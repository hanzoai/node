#!/bin/bash

# Manual publishing script for hanzo-message-primitives v2 with correct versions

set -e

echo "🚀 Manually publishing hanzo-message-primitives (v2 fixed)"
echo "=============================================="

cd hanzo-libs/hanzo-message-primitives

# Create a temporary fixed Cargo.toml
cat > Cargo.toml.fixed << 'EOF'
[package]
name = "hanzo_message_primitives"
version = "1.1.10"
edition = "2021"
authors = ["Hanzo AI Inc"]
license = "MIT"
repository = "https://github.com/hanzoai/hanzo-node"
homepage = "https://hanzo.ai"
description = "Message primitives for Hanzo AI platform"

[dependencies]
serde_json = "1.0.117"
chacha20poly1305 = "0.7.1"
x25519-dalek = "2.0.1"
ed25519-dalek = "2.1.1"
rand = "0.8.5"
chrono = { version = "0.4", features = ["serde"] }
regex = "1"
thiserror = "2.0"
hex = "0.4.3"
aes-gcm = "0.10.3"
blake3 = "1.5"
rust_decimal = "1.17.0"
base64 = "0.22.0"
utoipa = { version = "4.2", features = ["yaml"] }
serde = { version = "1.0.219", features = ["derive"] }
tokio = { version = "1.36", features = ["rt", "rt-multi-thread", "macros", "fs", "io-util", "net", "sync", "time"] }
async-trait = "0.1.81"
tracing = "0.1.40"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
os_path = "0.8.0"

[lib]
crate-type = ["rlib"]

[dev-dependencies]
serial_test = "0.5"
tempfile = "3.8"

[workspace]

[[test]]
name = "hanzo_message_tests"
path = "tests/hanzo_message_tests.rs"

[[test]]
name = "hanzo_name_tests"
path = "tests/hanzo_name_tests.rs"
EOF

# Backup original
cp Cargo.toml Cargo.toml.backup

# Use fixed version
cp Cargo.toml.fixed Cargo.toml

echo "📤 Publishing to crates.io..."
cargo publish --allow-dirty 2>&1 || {
    echo "❌ Failed to publish. Checking error..."
    # Restore and exit with error
    mv Cargo.toml.backup Cargo.toml
    rm Cargo.toml.fixed
    exit 1
}

# Restore original
mv Cargo.toml.backup Cargo.toml
rm Cargo.toml.fixed

echo "✅ Successfully published hanzo-message-primitives!"
echo ""
echo "Waiting 10 seconds for crates.io to index..."
sleep 10