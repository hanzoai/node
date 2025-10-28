#!/bin/bash

# Fork all models from lmstudio-community and mlx-community to hanzo-community and hanzo-mlx
# This ensures we have a complete mirror of trusted, quantized models

set -e

echo "🚀 Hanzo Community Model Fork Pipeline"
echo "======================================"

# Check HF CLI
if ! command -v huggingface-cli &> /dev/null; then
    echo "❌ huggingface-cli not found. Install with: pip install huggingface-hub"
    exit 1
fi

# Check if logged in
if ! huggingface-cli whoami &>/dev/null; then
    echo "❌ Not logged in to HuggingFace. Please run: huggingface-cli login"
    exit 1
fi

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# Function to list all models from an organization
list_org_models() {
    local org=$1
    echo "📋 Fetching models from $org..." >&2

    # Use HF API to list models
    python3 << EOF
from huggingface_hub import HfApi
api = HfApi()
models = api.list_models(author="$org", limit=1000)
for model in models:
    print(model.modelId)
EOF
}

# Function to fork with error handling
safe_fork() {
    local model=$1
    local target_org=$2
    local model_name=$(basename "$model")

    echo "  → Forking $model to $target_org/$model_name"

    # Check if already exists
    if huggingface-cli repo create "$model_name" --organization "$target_org" --type model -y 2>&1 | grep -q "already exists"; then
        echo "    ⚠️  Repository already exists, skipping..."
        return 0
    fi

    # Use the fork script
    "$SCRIPT_DIR/fork_model.sh" "$model" "$target_org" "$model_name" 2>&1 | sed 's/^/    /' || {
        echo "    ❌ Failed to fork, continuing..."
        return 1
    }

    sleep 1  # Be nice to HF servers
}

# Mirror lmstudio-community to hanzo-community
echo ""
echo "📦 Mirroring lmstudio-community → hanzo-community"
echo "-------------------------------------------------"

LMSTUDIO_MODELS=$(list_org_models "lmstudio-community")
TOTAL_LM=$(echo "$LMSTUDIO_MODELS" | wc -l)
CURRENT=1

for model in $LMSTUDIO_MODELS; do
    echo "[$CURRENT/$TOTAL_LM] Processing $model"
    safe_fork "$model" "hanzo-community"
    ((CURRENT++))
done

# Mirror mlx-community to hanzo-mlx
echo ""
echo "🍎 Mirroring mlx-community → hanzo-mlx"
echo "--------------------------------------"

MLX_MODELS=$(list_org_models "mlx-community")
TOTAL_MLX=$(echo "$MLX_MODELS" | wc -l)
CURRENT=1

for model in $MLX_MODELS; do
    echo "[$CURRENT/$TOTAL_MLX] Processing $model"
    safe_fork "$model" "hanzo-mlx"
    ((CURRENT++))
done

echo ""
echo "✅ Community model mirroring complete!"
echo ""
echo "📊 Statistics:"
echo "  - lmstudio-community: $TOTAL_LM models → hanzo-community"
echo "  - mlx-community: $TOTAL_MLX models → hanzo-mlx"
echo ""
echo "View your models at:"
echo "  🌐 https://huggingface.co/hanzo-community"
echo "  🌐 https://huggingface.co/hanzo-mlx"