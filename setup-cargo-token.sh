#!/bin/bash
set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo "Setting up CARGO_REGISTRY_TOKEN for Rust SDK publishing"
echo "======================================================="
echo ""

# Check if gh CLI is authenticated
if ! gh auth status &>/dev/null; then
    echo -e "${RED}✗ GitHub CLI not authenticated${NC}"
    echo "Please run: gh auth login"
    exit 1
fi

echo -e "${GREEN}✓ GitHub CLI authenticated${NC}"

# Get crates.io token
echo ""
echo -e "${YELLOW}Please provide your crates.io API token:${NC}"
echo "1. Go to https://crates.io/settings/tokens"
echo "2. Create a new token with 'publish-update' scope"
echo "3. Copy the token (starts with 'crates-io_...')"
echo ""
read -s -p "Enter your CARGO_REGISTRY_TOKEN: " CARGO_TOKEN
echo ""

if [[ -z "$CARGO_TOKEN" ]]; then
    echo -e "${RED}✗ No token provided${NC}"
    exit 1
fi

# Add to GitHub secrets
echo ""
echo "Adding CARGO_REGISTRY_TOKEN to GitHub secrets..."
if gh secret set CARGO_REGISTRY_TOKEN --repo hanzoai/rust-sdk --body "$CARGO_TOKEN"; then
    echo -e "${GREEN}✓ Successfully added CARGO_REGISTRY_TOKEN${NC}"
else
    echo -e "${RED}✗ Failed to add token${NC}"
    exit 1
fi

# Verify
echo ""
echo "Verifying setup..."
if gh secret list --repo hanzoai/rust-sdk | grep -q CARGO_REGISTRY_TOKEN; then
    echo -e "${GREEN}✓ CARGO_REGISTRY_TOKEN verified in GitHub secrets${NC}"
    echo ""
    echo "Setup complete! You can now:"
    echo "1. Test with: cd ~/work/hanzo/rust-sdk && ./scripts/release-package.sh hanzo-kbs 0.1.0 --dry-run"
    echo "2. Release with: cd ~/work/hanzo/rust-sdk && ./scripts/release-package.sh <package> <version>"
    echo ""
    echo "Monitor releases at: https://github.com/hanzoai/rust-sdk/actions"
else
    echo -e "${RED}✗ Token not found after adding${NC}"
    exit 1
fi