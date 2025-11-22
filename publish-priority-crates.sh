#!/bin/bash

echo "========================================================================="
echo "Publishing Hanzo Crates in Dependency Order"
echo "========================================================================="

# Priority 1: Crates with no hanzo dependencies (can be published first)
# These are the foundational crates that others depend on

PRIORITY_1=(
  "hanzo-message-primitives"   # Core message types
  "hanzo-crypto-identities"    # Cryptography primitives
  "hanzo-tools-primitives"     # Tool system primitives
)

echo "Priority 1: Foundational crates (no hanzo dependencies)"
echo "---------------------------------------------------------------------"

for crate in "${PRIORITY_1[@]}"; do
  echo "Publishing $crate..."
  cd "hanzo-libs/$crate" || continue
  
  # Check if it compiles first
  if cargo check 2>&1 | grep -q "error:"; then
    echo "❌ $crate has compilation errors, skipping"
    cd ../..
    continue
  fi
  
  # Try to publish
  if cargo publish --allow-dirty 2>&1 | tee "../../publish-$crate.log" | grep -q "Uploading"; then
    echo "✅ $crate published successfully"
  else
    echo "❌ $crate publication failed (see publish-$crate.log)"
  fi
  
  cd ../..
  sleep 2  # Rate limiting
done

echo "========================================================================="
echo "Priority 1 complete. Check logs for details."
echo "========================================================================="
