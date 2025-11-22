#!/bin/bash

echo "========================================================================="
echo "Hanzo Crates Naming Convention Verification"
echo "========================================================================="
echo ""

# 1. Check all [package] names use hyphens
echo "1. Checking [package] names (should all use hyphens):"
echo "---------------------------------------------------------------------"
grep '^name = "hanzo' hanzo-libs/*/Cargo.toml hanzo-bin/*/Cargo.toml hanzo-test-*/Cargo.toml 2>/dev/null | while read line; do
  if echo "$line" | grep -q 'name = "hanzo_'; then
    echo "❌ FAIL: $line"
  else
    echo "✅ PASS: $line"
  fi
done
echo ""

# 2. Check all [lib] names use underscores
echo "2. Checking [lib] names (should all use underscores):"
echo "---------------------------------------------------------------------"
grep -A1 '^\[lib\]' hanzo-libs/*/Cargo.toml hanzo-test-*/Cargo.toml 2>/dev/null | grep 'name = "hanzo' | while read line; do
  if echo "$line" | grep -q 'name = "hanzo-'; then
    echo "❌ FAIL: $line (should use underscores)"
  else
    echo "✅ PASS: $line"
  fi
done
echo ""

# 3. Check dependency references use hyphens
echo "3. Checking dependency references (should use hyphens):"
echo "---------------------------------------------------------------------"
UNDERSCORE_DEPS=$(grep -r 'hanzo_[a-z_]* = {' --include='Cargo.toml' . | grep -v '^\s*#' | grep -v target | grep -v '.bak' | wc -l)
if [ "$UNDERSCORE_DEPS" -gt 0 ]; then
  echo "❌ FAIL: Found $UNDERSCORE_DEPS dependency references with underscores"
  grep -r 'hanzo_[a-z_]* = {' --include='Cargo.toml' . | grep -v '^\s*#' | grep -v target | grep -v '.bak'
else
  echo "✅ PASS: All dependency references use hyphens"
fi
echo ""

# 4. Check feature references use hyphens
echo "4. Checking feature references (should use hyphens):"
echo "---------------------------------------------------------------------"
UNDERSCORE_FEATURES=$(grep -r '"hanzo_[a-z_]*/' --include='Cargo.toml' . | grep -v '^\s*#' | grep -v target | grep -v '.bak' | wc -l)
if [ "$UNDERSCORE_FEATURES" -gt 0 ]; then
  echo "❌ FAIL: Found $UNDERSCORE_FEATURES feature references with underscores"
  grep -r '"hanzo_[a-z_]*/' --include='Cargo.toml' . | grep -v '^\s*#' | grep -v target | grep -v '.bak'
else
  echo "✅ PASS: All feature references use hyphens"
fi
echo ""

echo "========================================================================="
echo "Verification Complete"
echo "========================================================================="
