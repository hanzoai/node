#!/bin/bash
#
# Check crate versions and publish to crates.io if newer version is available.
# This script is designed to run in CI on every push to main.
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_action() {
    echo -e "${BLUE}[ACTION]${NC} $1"
}

# Get local version from Cargo.toml
get_local_version() {
    local cargo_toml="$1"
    grep -m1 '^version = ' "$cargo_toml" | sed 's/version = "\(.*\)"/\1/'
}

# Get published version from crates.io
get_crates_io_version() {
    local crate_name="$1"

    # Try jq first (faster), fall back to grep if not available
    local version=""
    if command -v jq &> /dev/null; then
        version=$(curl -s "https://crates.io/api/v1/crates/$crate_name" 2>/dev/null | \
            jq -r '.crate.max_version // empty' 2>/dev/null || echo "")
    else
        # Fallback: extract using grep and sed
        local response=$(curl -s "https://crates.io/api/v1/crates/$crate_name" 2>/dev/null)
        version=$(echo "$response" | grep -o '"max_version":"[^"]*"' | head -1 | sed 's/"max_version":"\([^"]*\)"/\1/')
    fi

    echo "$version"
}

# Compare versions (returns 0 if v1 > v2, 1 otherwise)
version_gt() {
    local v1="$1"
    local v2="$2"

    if [ -z "$v2" ]; then
        # No published version, always publish
        return 0
    fi

    # Use sort -V for version comparison
    if [ "$(printf '%s\n' "$v1" "$v2" | sort -V | tail -n1)" = "$v1" ] && [ "$v1" != "$v2" ]; then
        return 0
    else
        return 1
    fi
}

# Check if crate needs --no-verify flag
needs_no_verify() {
    local crate_name="$1"
    case "$crate_name" in
        hanzo-runner)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

# Publish a single crate
publish_crate() {
    local crate_name="$1"
    local crate_dir="$2"

    print_info "Checking $crate_name..."

    # Get local version
    local cargo_toml="$crate_dir/Cargo.toml"
    if [ ! -f "$cargo_toml" ]; then
        print_warn "Cargo.toml not found for $crate_name"
        return 1
    fi

    local local_version=$(get_local_version "$cargo_toml")
    print_info "  Local version: $local_version"

    # Get crates.io version
    local published_version=$(get_crates_io_version "$crate_name")
    if [ -n "$published_version" ]; then
        print_info "  crates.io version: $published_version"
    else
        print_info "  crates.io version: Not published yet"
    fi

    # Check if we need to publish
    if version_gt "$local_version" "$published_version"; then
        print_action "📦 New version detected for $crate_name: $local_version"

        # Check for API token
        if [ -z "$CARGO_REGISTRY_TOKEN" ]; then
            print_error "CARGO_REGISTRY_TOKEN environment variable not set"
            return 1
        fi

        # Publish the crate (save current directory)
        local original_dir=$(pwd)
        cd "$crate_dir"

        if needs_no_verify "$crate_name"; then
            print_action "Publishing $crate_name with --no-verify..."
            publish_output=$(cargo publish --token "$CARGO_REGISTRY_TOKEN" --no-verify 2>&1)
            publish_result=$?
        else
            print_action "Publishing $crate_name..."
            publish_output=$(cargo publish --token "$CARGO_REGISTRY_TOKEN" 2>&1)
            publish_result=$?
        fi

        # Return to original directory
        cd "$original_dir"

        # Check if publish succeeded or already exists
        if [ $publish_result -eq 0 ]; then
            print_info "✅ Successfully published $crate_name"
        elif echo "$publish_output" | grep -q "already exists"; then
            print_info "✅ $crate_name is already published at this version"
        else
            print_error "Failed to publish $crate_name"
            echo "$publish_output"
            return 1
        fi

        # Wait for crates.io indexing
        print_info "Waiting 30s for crates.io indexing..."
        sleep 30

        return 0
    else
        print_info "  ✓ $crate_name is up to date"
        return 0
    fi
}

# Main function
main() {
    # Get repository root
    REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    cd "$REPO_ROOT"

    # Define crates in dependency order (same as publish-ordered.yml)
    # Foundation crates (no internal deps)
    FOUNDATION_CRATES=(
        hanzo-messages
        hanzo-embed
        hanzo-runner
        hanzo-tools-runner
        hanzo-models
        hanzo-model-discovery
    )

    # Core crates (depend on foundation)
    CORE_CRATES=(
        hanzo-identity
        hanzo-pqc
        hanzo-did
        hanzo-db-sqlite
        hanzo-runtime
        hanzo-tools
    )

    # Service crates (depend on core)
    SERVICE_CRATES=(
        hanzo-mcp
        hanzo-fs
        hanzo-kbs
        hanzo-llm
        hanzo-jobs
        hanzo-database
        hanzo-wasm
        hanzo-wasm-runtime
    )

    # Integration crates (depend on services)
    INTEGRATION_CRATES=(
        hanzo-libp2p
        hanzo-libp2p-relayer
        hanzo-http-api
        hanzo-api
        hanzo-job-queue-manager
    )

    local success=0
    local failed=0

    # Process each group in order
    for crate in "${FOUNDATION_CRATES[@]}"; do
        if publish_crate "$crate" "hanzo-libs/$crate"; then
            ((success++))
        else
            ((failed++))
        fi
    done

    for crate in "${CORE_CRATES[@]}"; do
        if publish_crate "$crate" "hanzo-libs/$crate"; then
            ((success++))
        else
            ((failed++))
        fi
    done

    for crate in "${SERVICE_CRATES[@]}"; do
        if publish_crate "$crate" "hanzo-libs/$crate"; then
            ((success++))
        else
            ((failed++))
        fi
    done

    for crate in "${INTEGRATION_CRATES[@]}"; do
        if publish_crate "$crate" "hanzo-libs/$crate"; then
            ((success++))
        else
            ((failed++))
        fi
    done

    # Print summary
    echo ""
    print_info "========================================="
    print_info "Publication Summary"
    print_info "========================================="
    print_info "Checked: $((success + failed)) crates"
    print_info "Success: $success"
    print_info "Failed: $failed"

    if [ $failed -gt 0 ]; then
        exit 1
    fi
}

# Run main
main "$@"
