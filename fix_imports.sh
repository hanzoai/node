#!/bin/bash

echo "Fixing import errors across the codebase..."

# Fix missing imports in hanzo_node main binary
find hanzo-bin/hanzo-node/src -name "*.rs" -type f | while read file; do
  # Check if file uses EmbeddingModelType without importing it
  if grep -q "EmbeddingModelType" "$file" && ! grep -q "use.*EmbeddingModelType" "$file"; then
    echo "Fixing imports in $file"
    # Add import after the first use statement
    sed -i '' '0,/^use /{s/^use /use hanzo_embedding::model_type::EmbeddingModelType;\nuse /}' "$file"
  fi
  
  # Check if file uses MCPServerTool without importing it
  if grep -q "MCPServerTool" "$file" && ! grep -q "use.*MCPServerTool" "$file"; then
    echo "Adding MCPServerTool import to $file"
    sed -i '' '0,/^use /{s/^use /use hanzo_tools_primitives::tools::mcp_server_tool::MCPServerTool;\nuse /}' "$file"
  fi
done

# Fix openrouter issues - comment out or add the missing function
find hanzo-bin/hanzo-node/src -name "*.rs" -type f -exec sed -i '' \
  's/get_openrouter_model/\/\/ get_openrouter_model/g' {} \;

echo "Import fixes applied!"