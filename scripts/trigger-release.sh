#!/bin/bash

# Script to trigger a release workflow for hanzod

set -e

# Check if version is provided
if [ -z "$1" ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 v1.1.11"
    exit 1
fi

VERSION=$1

# Validate version format
if ! [[ "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in format v*.*.* (e.g., v1.1.11)"
    exit 1
fi

echo "🚀 Triggering release for version $VERSION"

# Create and push tag
git tag -a "$VERSION" -m "Release $VERSION"
git push origin "$VERSION"

echo "✅ Release triggered! Check GitHub Actions for build progress:"
echo "   https://github.com/hanzoai/node/actions"