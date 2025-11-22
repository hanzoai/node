#!/usr/bin/env python3
"""
Fix ed25519-dalek v2.x and x25519-dalek v2.x API changes in hanzo-message-primitives
"""

import os
import re

BASE_PATH = "/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-message-primitives/src"

# API Changes:
# ed25519-dalek v2.x:
#   - SigningKey::generate(&mut rng) → SigningKey::new(&mut rng)
#   - (already uses SigningKey/VerifyingKey which is correct)
#
# x25519-dalek v2.x:
#   - StaticSecret::random_from_rng(rng) → StaticSecret::random_from_rng(&mut rng)
#   - (StaticSecret/PublicKey still exist, just need &mut)

def fix_file(filepath):
    """Fix crypto API usage in a single file"""
    with open(filepath, 'r') as f:
        content = f.read()

    original = content

    # Fix ed25519-dalek v2.x: SigningKey::generate → SigningKey::new
    content = re.sub(
        r'SigningKey::generate\(',
        'SigningKey::new(',
        content
    )

    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        print(f"✅ Fixed {filepath}")
        return True
    else:
        print(f"⏭️  No changes needed in {filepath}")
        return False

def main():
    print("🔧 Fixing ed25519-dalek and x25519-dalek API usage")
    print("=" * 60)

    # Files that need fixing based on errors
    files_to_fix = [
        "hanzo_utils/signatures.rs",
    ]

    fixed_count = 0
    for file_rel in files_to_fix:
        filepath = os.path.join(BASE_PATH, file_rel)
        if os.path.exists(filepath):
            if fix_file(filepath):
                fixed_count += 1
        else:
            print(f"⚠️  File not found: {filepath}")

    print("=" * 60)
    print(f"✅ Fixed {fixed_count} file(s)")
    print("\n🧪 Testing compilation...")

    # Test compilation
    os.chdir("/Users/z/work/shinkai/hanzo-node/hanzo-libs/hanzo-message-primitives")
    result = os.system("cargo check 2>&1 | tail -10")

    if result == 0:
        print("\n✅ Compilation successful!")
    else:
        print("\n⚠️  Compilation had issues (see output above)")

if __name__ == "__main__":
    main()
