#!/bin/bash
# Fix empty dependency braces in all hanzo Cargo.toml files

cd hanzo-libs

for dir in hanzo-*/; do
    if [ -f "$dir/Cargo.toml" ]; then
        echo "Fixing $dir/Cargo.toml..."

        # Fix empty braces
        sed -i.bak2 's/rand = {  }/rand = "0.8.5"/' "$dir/Cargo.toml"
        sed -i.bak2 's/keyphrases = {  }/keyphrases = "0.3.3"/' "$dir/Cargo.toml"
        sed -i.bak2 's/csv = {  }/csv = "1.1.6"/' "$dir/Cargo.toml"

        # Fix malformed [dependencies.serde] sections
        sed -i.bak2 '/^\[dependencies.serde\]$/,/^$/d' "$dir/Cargo.toml"

        rm -f "$dir/Cargo.toml.bak2"
    fi
done

echo "✅ Fixed empty dependencies"
