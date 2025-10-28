#!/bin/bash

# Hanzo Installation Script
# Downloads and installs hanzod and hanzoai binaries for your platform

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

# Map architecture names
case "$ARCH" in
    x86_64)
        ARCH="amd64"
        ;;
    aarch64|arm64)
        ARCH="arm64"
        ;;
    *)
        echo -e "${RED}Unsupported architecture: $ARCH${NC}"
        exit 1
        ;;
esac

# Map OS names
case "$OS" in
    darwin)
        OS="macos"
        ;;
    linux)
        OS="linux"
        ;;
    mingw*|msys*|cygwin*)
        OS="windows"
        ;;
    *)
        echo -e "${RED}Unsupported OS: $OS${NC}"
        exit 1
        ;;
esac

echo -e "${BLUE}🚀 Installing Hanzo AI Suite${NC}"
echo -e "   Platform: ${GREEN}$OS-$ARCH${NC}"
echo ""

# Installation directory
INSTALL_DIR="${HANZO_HOME:-$HOME/.hanzo}/bin"
mkdir -p "$INSTALL_DIR"

# GitHub API URLs
HANZOD_RELEASE_URL="https://api.github.com/repos/hanzoai/node/releases/latest"
HANZOAI_RELEASE_URL="https://api.github.com/repos/hanzoai/engine/releases/latest"

# Function to download and install a binary
install_binary() {
    local name=$1
    local release_url=$2
    local binary_name=$3
    local archive_pattern=$4

    echo -e "${YELLOW}📥 Installing $name...${NC}"

    # Get the download URL for the appropriate asset
    local download_url=$(curl -s "$release_url" | \
        grep -o "\"browser_download_url\": \"[^\"]*$archive_pattern[^\"]*\"" | \
        grep -o "https://[^\"]*" | \
        head -n1)

    if [ -z "$download_url" ]; then
        echo -e "${RED}Could not find $name binary for $OS-$ARCH${NC}"
        echo -e "${YELLOW}You may need to build from source or wait for the next release.${NC}"
        return 1
    fi

    # Download the archive
    local temp_dir=$(mktemp -d)
    cd "$temp_dir"

    echo "   Downloading from: $download_url"
    curl -sL "$download_url" -o archive

    # Extract based on file type
    if [[ "$download_url" == *.zip ]]; then
        unzip -q archive
    else
        tar -xzf archive
    fi

    # Move binary to installation directory
    if [ -f "$binary_name" ]; then
        mv "$binary_name" "$INSTALL_DIR/"
        chmod +x "$INSTALL_DIR/$binary_name"
        echo -e "   ${GREEN}✓ Installed to $INSTALL_DIR/$binary_name${NC}"
    elif [ -f "${binary_name}.exe" ]; then
        mv "${binary_name}.exe" "$INSTALL_DIR/"
        echo -e "   ${GREEN}✓ Installed to $INSTALL_DIR/${binary_name}.exe${NC}"
    else
        echo -e "${RED}Binary not found in archive${NC}"
        ls -la
        return 1
    fi

    # Cleanup
    cd - > /dev/null
    rm -rf "$temp_dir"
}

# Install hanzod (Hanzo Node)
if [ "$OS" == "windows" ]; then
    install_binary "hanzod" "$HANZOD_RELEASE_URL" "hanzod" "hanzod-${OS}-${ARCH}.zip"
else
    install_binary "hanzod" "$HANZOD_RELEASE_URL" "hanzod" "hanzod-${OS}-${ARCH}.tar.gz"
fi

# Install hanzoai (Hanzo Engine)
if [ "$OS" == "windows" ]; then
    install_binary "hanzoai" "$HANZOAI_RELEASE_URL" "hanzoai" "hanzoai-${OS}-${ARCH}.zip"
else
    install_binary "hanzoai" "$HANZOAI_RELEASE_URL" "hanzoai" "hanzoai-${OS}-${ARCH}.tar.gz"
fi

echo ""
echo -e "${GREEN}✅ Installation complete!${NC}"
echo ""
echo "Add the following to your shell configuration (.bashrc, .zshrc, etc.):"
echo -e "${YELLOW}export PATH=\"\$HOME/.hanzo/bin:\$PATH\"${NC}"
echo ""
echo "Then reload your shell or run:"
echo -e "${YELLOW}source ~/.bashrc${NC}"
echo ""
echo "To verify installation, run:"
echo -e "${BLUE}hanzod --version${NC}"
echo -e "${BLUE}hanzoai --version${NC}"