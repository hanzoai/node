#!/bin/bash

echo "Cleaning up Rust warnings in Hanzo Node project..."

# Fix unused imports by adding underscore prefix or removing them
echo "Fixing unused imports..."

# Remove unused imports from various files
find hanzo-bin hanzo-libs -name "*.rs" -type f -exec sed -i '' \
  -e 's/use.*DetailedFunctionCall.*;//g' \
  -e 's/use.*providers::openrouter.*;//g' \
  -e 's/use.*MCPServerTool.*;//g' \
  -e 's/use.*AgentNetworkOfferingResponse.*;//g' \
  -e 's/use.*encryption_secret_key_to_string.*;//g' \
  -e 's/use.*std::process::Command.*;//g' \
  -e 's/use.*OllamaTextEmbeddingsInference.*;//g' {} \;

# Add allow(dead_code) to files with many unused functions
echo "Adding allow(dead_code) attributes..."

# Add to specific modules with many unused functions
for file in \
  "hanzo-bin/hanzo-node/src/managers/galxe_quests.rs" \
  "hanzo-bin/hanzo-node/src/network/agent_payments_manager/my_agent_offerings_manager.rs" \
  "hanzo-bin/hanzo-node/src/utils/printer.rs" \
  "hanzo-bin/hanzo-node/src/network/v1_api/api_v1_commands.rs" \
  "hanzo-bin/hanzo-node/src/network/v1_api/api_v1_internal_commands.rs" \
  "hanzo-bin/hanzo-node/src/utils/github_mcp.rs"
do
  if [ -f "$file" ]; then
    # Add #![allow(dead_code)] at the top of the file if not already present
    if ! grep -q "#!\[allow(dead_code)\]" "$file"; then
      sed -i '' '1i\
#![allow(dead_code)]
' "$file"
    fi
  fi
done

echo "Cleanup complete! Run 'cargo build' to verify."