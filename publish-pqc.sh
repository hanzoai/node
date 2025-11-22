#!/bin/bash
set -e

echo "📦 Publishing hanzo-pqc"
cd hanzo-libs/hanzo-pqc

cat > Cargo.toml.fixed << 'EOF'
[package]
name = "hanzo_pqc"
version = "1.1.10"
edition = "2021"
authors = ["Hanzo AI Inc"]
license = "MIT"
repository = "https://github.com/hanzoai/hanzo-node"
homepage = "https://hanzo.ai"
description = "Post-quantum cryptography for Hanzo AI platform"

[features]
default = ["ml-kem", "ml-dsa", "hybrid"]
ml-kem = []
ml-dsa = []
slh-dsa = []
hybrid = ["x25519-dalek"]
fips-mode = []
gpu-cc = []
tee-io = []

[dependencies]
oqs = { version = "0.11", default-features = false, features = ["std"] }
x25519-dalek = { version = "2.0.1", features = ["static_secrets"], optional = true }
ed25519-dalek = { version = "2.1.1", features = ["rand_core"] }
hkdf = "0.12"
sha2 = "0.10"
sha3 = "0.10"
blake3 = "1.5"
serde = { version = "1.0.219", features = ["derive"] }
serde_json = "1.0.117"
bincode = "1.3.3"
base64 = "0.22.0"
thiserror = "2.0"
anyhow = "1.0.86"
async-trait = "0.1.81"
tokio = { version = "1.36", features = ["rt", "rt-multi-thread", "macros", "fs", "io-util", "net", "sync", "time"] }
rand = "0.8.5"
getrandom = { version = "0.2", features = ["std"] }
hex = "0.4.3"
zeroize = { version = "1.8", features = ["derive"] }

[dev-dependencies]
tokio-test = "0.4"
criterion = "0.5"
proptest = "1.0"

[[bench]]
name = "pqc_benchmarks"
harness = false
EOF

cp Cargo.toml Cargo.toml.backup
cp Cargo.toml.fixed Cargo.toml

echo "📤 Publishing..."
cargo publish --allow-dirty 2>&1 || {
    mv Cargo.toml.backup Cargo.toml
    rm Cargo.toml.fixed
    exit 1
}

mv Cargo.toml.backup Cargo.toml
rm Cargo.toml.fixed
echo "✅ Published hanzo-pqc"
