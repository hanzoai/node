#!/bin/bash

# Generate test keys for Hanzo node
# Keys are saved to ~/.hanzod/keys/ for local use
# Can be exported to GitHub environment for CI

set -e

KEYS_DIR="$HOME/.hanzod/keys"
KEYS_FILE="$KEYS_DIR/test-keys.env"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Hanzo Node Test Key Generator${NC}"
echo "================================"
echo

# Create keys directory if it doesn't exist
mkdir -p "$KEYS_DIR"
chmod 700 "$KEYS_DIR"

# Function to generate a random hex key
generate_key() {
    python3 -c "import secrets; print('0x' + secrets.token_hex(32))"
}

# Function to generate a random address
generate_address() {
    python3 -c "import secrets; print('0x' + secrets.token_hex(20))"
}

# Generate test mnemonic (using first 12 words from BIP39 for testing)
TEST_MNEMONIC="abandon ability able about above absent absorb abstract absurd abuse access accident"

echo "Generating test keys..."
echo

# Generate keys
FROM_WALLET_PRIVATE_KEY=$(generate_key)
X402_PRIVATE_KEY=$(generate_key)
X402_PAY_TO=$(generate_address)
RESTORE_WALLET_MNEMONICS_NODE2="$TEST_MNEMONIC"

# Save to file
cat > "$KEYS_FILE" << EOF
# Hanzo Node Test Keys
# Generated: $(date)
# WARNING: These are test keys only - DO NOT USE IN PRODUCTION

export FROM_WALLET_PRIVATE_KEY="$FROM_WALLET_PRIVATE_KEY"
export X402_PRIVATE_KEY="$X402_PRIVATE_KEY"
export X402_PAY_TO="$X402_PAY_TO"
export RESTORE_WALLET_MNEMONICS_NODE2="$RESTORE_WALLET_MNEMONICS_NODE2"
EOF

chmod 600 "$KEYS_FILE"

echo -e "${GREEN}✓${NC} Keys generated and saved to: ${YELLOW}$KEYS_FILE${NC}"
echo

# Display the keys (for copying to GitHub secrets if needed)
echo "Keys generated (for GitHub Actions secrets):"
echo "============================================"
echo
echo "FROM_WALLET_PRIVATE_KEY=$FROM_WALLET_PRIVATE_KEY"
echo "X402_PRIVATE_KEY=$X402_PRIVATE_KEY"
echo "X402_PAY_TO=$X402_PAY_TO"
echo "RESTORE_WALLET_MNEMONICS_NODE2=$RESTORE_WALLET_MNEMONICS_NODE2"
echo

# Instructions
echo -e "${YELLOW}To use these keys:${NC}"
echo "1. For local testing: source $KEYS_FILE"
echo "2. For GitHub Actions: Add as repository secrets in Settings → Secrets"
echo "3. For Docker: docker run --env-file $KEYS_FILE ..."
echo

echo -e "${RED}⚠️  WARNING:${NC} Never commit these keys to the repository!"
echo -e "${RED}⚠️  WARNING:${NC} These are test keys only - never use with real funds!"