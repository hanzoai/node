#!/bin/bash

# Fork a single model to Hanzo HuggingFace organization
# Usage: ./fork_model.sh <source_model> <target_org> [new_name]

set -e

SOURCE_MODEL=$1
TARGET_ORG=$2
NEW_NAME=${3:-$(basename $SOURCE_MODEL)}

if [ -z "$SOURCE_MODEL" ] || [ -z "$TARGET_ORG" ]; then
    echo "Usage: $0 <source_model> <target_org> [new_name]"
    echo "Example: $0 meta-llama/Llama-3.3-70B-Instruct hanzo-lm"
    exit 1
fi

echo "🚀 Forking $SOURCE_MODEL to $TARGET_ORG/$NEW_NAME"

# Create temp directory
TEMP_DIR="/tmp/hf_fork_$$"
mkdir -p $TEMP_DIR

# Download the model
echo "📥 Downloading model..."
huggingface-cli download "$SOURCE_MODEL" \
    --local-dir "$TEMP_DIR" \
    --local-dir-use-symlinks False \
    --resume-download

# Create the repository
echo "📝 Creating repository $TARGET_ORG/$NEW_NAME..."
huggingface-cli repo create "$NEW_NAME" \
    --organization "$TARGET_ORG" \
    --type model \
    -y || echo "Repository might already exist, continuing..."

# Add Hanzo metadata to README if it exists
if [ -f "$TEMP_DIR/README.md" ]; then
    echo "📝 Adding Hanzo metadata to README..."
    cat << EOF >> "$TEMP_DIR/README.md"

---

## Hanzo AI Fork Information

This model has been forked to the Hanzo AI organization for availability and integration with Hanzo infrastructure.

- **Original Model**: $SOURCE_MODEL
- **Fork Date**: $(date -u +"%Y-%m-%d")
- **Organization**: [$TARGET_ORG](https://huggingface.co/$TARGET_ORG)
- **Integration**: Compatible with [Hanzo Node](https://github.com/hanzoai/node)

### Usage with Hanzo

\`\`\`python
from hanzo import HanzoClient

client = HanzoClient()
response = client.generate(
    model="$TARGET_ORG/$NEW_NAME",
    prompt="Your prompt here"
)
\`\`\`

For more information, visit [hanzo.ai](https://hanzo.ai)
EOF
fi

# Upload to the new repository
echo "📤 Uploading to $TARGET_ORG/$NEW_NAME..."
huggingface-cli upload "$TARGET_ORG/$NEW_NAME" \
    "$TEMP_DIR" . \
    --repo-type model

# Clean up
echo "🧹 Cleaning up..."
rm -rf "$TEMP_DIR"

echo "✅ Successfully forked $SOURCE_MODEL to $TARGET_ORG/$NEW_NAME"
echo "🔗 View at: https://huggingface.co/$TARGET_ORG/$NEW_NAME"