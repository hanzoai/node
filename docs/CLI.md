# Hanzo Node CLI Documentation

## Overview

The Hanzo Node (`hanzod`) provides a comprehensive command-line interface for managing the node, cryptographic keys, and various operational tasks.

## Installation

```bash
cargo install --path hanzo-bin/hanzo-node
```

Or download the pre-built binary from the releases page.

## Commands

### `hanzod run`

Run the Hanzo node with specified configuration.

```bash
hanzod run [OPTIONS]
```

#### Options

- `--node-ip <IP>` - Node IP address (default: 0.0.0.0, env: NODE_IP)
- `--node-port <PORT>` - Node port (default: 9452, env: NODE_PORT)
- `--api-ip <IP>` - API IP address (default: 0.0.0.0, env: NODE_API_IP)
- `--api-port <PORT>` - API port (default: 9450, env: NODE_API_PORT)

#### Examples

```bash
# Run with default settings
hanzod run

# Run with custom ports
hanzod run --node-port 9500 --api-port 9501

# Run with environment variables
export NODE_PORT=9500
hanzod run
```

### `hanzod keys`

Manage cryptographic keys for the node.

#### Subcommands

##### `hanzod keys generate`

Generate new cryptographic keys.

```bash
hanzod keys generate <TYPE> [OPTIONS]
```

**Key Types:**

- `wallet` - Generate wallet keys for transactions
  - `--network <NETWORK>` - Network to generate for (default: sepolia)
- `identity` - Generate node identity keys
- `x402` - Generate X402 payment protocol keys
- `test` - Generate all test keys for CI/testing
- `staking` - Generate staking/validator keys
  - `--amount <ETH>` - Amount to stake

**Options:**

- `-f, --format <FORMAT>` - Output format (env, json, yaml, toml) [default: env]
- `-s, --save` - Save keys to ~/.hanzod/keys/ directory

**Examples:**

```bash
# Generate wallet keys for mainnet
hanzod keys generate wallet --network mainnet --save

# Generate test keys for CI
hanzod keys generate test --format json --save

# Generate staking keys
hanzod keys generate staking --amount 32 --save
```

##### `hanzod keys list`

List existing keys stored in ~/.hanzod/keys/.

```bash
hanzod keys list [OPTIONS]
```

**Options:**

- `-s, --show-private` - Show full key values (default: show only metadata)

**Examples:**

```bash
# List all keysets (metadata only)
hanzod keys list

# Show full key values (⚠️ careful with this)
hanzod keys list --show-private
```

##### `hanzod keys import`

Import keys from a file.

```bash
hanzod keys import [FILE]
```

**Examples:**

```bash
# Import from file
hanzod keys import keys.json

# Import from stdin
cat keys.json | hanzod keys import
```

##### `hanzod keys export`

Export keys to a file.

```bash
hanzod keys export [FILE] [OPTIONS]
```

**Options:**

- `--include-private` - Include private keys in export

**Examples:**

```bash
# Export public keys only
hanzod keys export keys-public.json

# Export with private keys (⚠️ be careful)
hanzod keys export keys-full.json --include-private
```

### `hanzod init`

Initialize node configuration.

```bash
hanzod init [OPTIONS]
```

**Options:**

- `-f, --force` - Force overwrite existing configuration

**Examples:**

```bash
# Initialize new node
hanzod init

# Reinitialize (overwrites existing config)
hanzod init --force
```

### `hanzod version`

Display version information.

```bash
hanzod version
```

## Key Storage

All keys are stored in `~/.hanzod/keys/` with restrictive permissions (700 for directory, 600 for files).

### Directory Structure

```
~/.hanzod/
├── keys/
│   ├── wallet-20250917-123456.json
│   ├── identity-20250917-123457.json
│   ├── test-20250917-123458.json
│   └── staking-20250917-123459.json
├── config/
│   └── node.toml
└── data/
    └── db.sqlite
```

## Environment Variables

The following environment variables can be used:

- `NODE_IP` - Node IP address
- `NODE_PORT` - Node port
- `NODE_API_IP` - API IP address
- `NODE_API_PORT` - API port
- `RUST_LOG` - Logging level (debug, info, warn, error)

## Security Best Practices

1. **Never commit keys to version control**
   - The `.gitignore` is configured to exclude all key files
   - Use `~/.hanzod/keys/` for local key storage

2. **Protect production keys**
   - Use hardware wallets for production validator keys
   - Never expose private keys in logs or console output
   - Use `--show-private` flag with extreme caution

3. **Test keys are for testing only**
   - Keys generated with `hanzod keys generate test` should NEVER be used with real funds
   - These are meant for CI/CD and local testing only

4. **Secure key storage**
   - Keys are automatically saved with restrictive permissions (600)
   - The keys directory has 700 permissions (owner only)
   - Consider encrypting the filesystem where keys are stored

## CI/CD Integration

### GitHub Actions

```yaml
- name: Generate test keys
  run: |
    hanzod keys generate test --format env > test-keys.env
    source test-keys.env
    echo "::add-mask::$FROM_WALLET_PRIVATE_KEY"
    echo "FROM_WALLET_PRIVATE_KEY=$FROM_WALLET_PRIVATE_KEY" >> $GITHUB_ENV
```

### Docker

```bash
# Generate keys and use with Docker
hanzod keys generate test --save
docker run --env-file ~/.hanzod/keys/test-latest.env hanzo/node:latest
```

## Troubleshooting

### Permission Denied

If you get permission errors accessing `~/.hanzod/keys/`:

```bash
# Fix permissions
chmod 700 ~/.hanzod
chmod 700 ~/.hanzod/keys
chmod 600 ~/.hanzod/keys/*
```

### Key Generation Fails

Ensure you have the required system dependencies:

```bash
# Ubuntu/Debian
apt-get install libssl-dev

# macOS
brew install openssl
```

## Examples

### Complete Node Setup

```bash
# 1. Initialize node
hanzod init

# 2. Generate identity keys
hanzod keys generate identity --save

# 3. Generate wallet keys
hanzod keys generate wallet --network sepolia --save

# 4. Run the node
hanzod run
```

### Setting Up for Testing

```bash
# Generate test keys
hanzod keys generate test --save

# Source the environment
source ~/.hanzod/keys/test-latest.env

# Run tests
cargo test
```

### Validator Setup

```bash
# Generate staking keys
hanzod keys generate staking --amount 32 --save

# Export public keys for deposit
hanzod keys export validator-deposit.json

# Keep private keys secure!
```

## Related Documentation

- [Node Configuration](./CONFIG.md)
- [API Documentation](./API.md)
- [Security Guide](./SECURITY.md)
- [Deployment Guide](./DEPLOYMENT.md)