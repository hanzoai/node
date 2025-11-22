#!/bin/bash
set -e

echo "📦 Publishing hanzo-non-rust-code"
cd hanzo-libs/hanzo-non-rust-code

# Create fixed Cargo.toml
cat > Cargo.toml.fixed << 'EOF'
[package]
name = "hanzo_non_rust_code"
version = "1.1.10"
edition = "2021"
authors = ["Hanzo AI Inc"]
license = "MIT"
repository = "https://github.com/hanzoai/hanzo-node"
homepage = "https://hanzo.ai"
description = "Non-Rust code execution for Hanzo AI platform"

[dependencies]
serde = { version = "1.0.219", features = ["derive"] }
serde_json = "1.0.117"
tokio = { version = "1.36", features = ["rt", "rt-multi-thread", "macros", "fs", "io-util", "net", "sync", "time"] }
hanzo_tools_runner = { version = "1.0.3", features = ["built-in-tools"] }
tempfile = "3.8"
hanzo_message_primitives = "1.1.10"
log = "0.4.20"
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

echo "✅ Published hanzo-non-rust-code"
