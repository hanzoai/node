#!/bin/bash

# SQLite to LanceDB Migration Script
# This script migrates your existing SQLite database to the new LanceDB format

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}╔════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║   SQLite to LanceDB Migration Tool     ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════╝${NC}"
echo ""

# Default paths
SQLITE_PATH="${SQLITE_PATH:-./storage/db.sqlite}"
LANCEDB_PATH="${LANCEDB_PATH:-./storage/lancedb}"
BACKUP_DIR="${BACKUP_DIR:-./storage/backups}"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --sqlite-path)
            SQLITE_PATH="$2"
            shift 2
            ;;
        --lancedb-path)
            LANCEDB_PATH="$2"
            shift 2
            ;;
        --no-backup)
            NO_BACKUP=true
            shift
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --sqlite-path PATH    Path to SQLite database (default: ./storage/db.sqlite)"
            echo "  --lancedb-path PATH   Path to LanceDB directory (default: ./storage/lancedb)"
            echo "  --no-backup          Skip backup creation"
            echo "  --help               Show this help message"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

# Check if SQLite database exists
if [ ! -f "$SQLITE_PATH" ]; then
    echo -e "${RED}Error: SQLite database not found at $SQLITE_PATH${NC}"
    exit 1
fi

# Create backup unless disabled
if [ "$NO_BACKUP" != "true" ]; then
    echo -e "${YELLOW}Creating backup...${NC}"
    mkdir -p "$BACKUP_DIR"
    BACKUP_FILE="$BACKUP_DIR/db_backup_$(date +%Y%m%d_%H%M%S).sqlite"
    cp "$SQLITE_PATH" "$BACKUP_FILE"
    echo -e "${GREEN}✓ Backup created at: $BACKUP_FILE${NC}"
fi

# Check if LanceDB directory already exists
if [ -d "$LANCEDB_PATH" ]; then
    echo -e "${YELLOW}Warning: LanceDB directory already exists at $LANCEDB_PATH${NC}"
    read -p "Do you want to continue and potentially overwrite data? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Migration cancelled."
        exit 0
    fi
fi

# Build the migration tool
echo -e "${YELLOW}Building migration tool...${NC}"
cd "$(dirname "$0")/.."
cargo build --release --bin hanzo-migrate --features "hanzo_lancedb/migration"

if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to build migration tool${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Migration tool built successfully${NC}"

# Run the migration
echo -e "${YELLOW}Starting migration...${NC}"
echo "  Source: $SQLITE_PATH"
echo "  Target: $LANCEDB_PATH"
echo ""

./target/release/hanzo-migrate \
    --sqlite-path "$SQLITE_PATH" \
    --lancedb-path "$LANCEDB_PATH" \
    --verify

if [ $? -ne 0 ]; then
    echo -e "${RED}Migration failed!${NC}"
    if [ "$NO_BACKUP" != "true" ]; then
        echo -e "${YELLOW}Your original database is backed up at: $BACKUP_FILE${NC}"
    fi
    exit 1
fi

echo ""
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN}✓ Migration completed successfully!${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""
echo "Next steps:"
echo "1. Update your Hanzo Node configuration to use LanceDB"
echo "2. Restart Hanzo Node with the new database"
echo "3. Verify that all data is accessible"

if [ "$NO_BACKUP" != "true" ]; then
    echo ""
    echo -e "${YELLOW}Note: Your original SQLite database is backed up at:${NC}"
    echo "  $BACKUP_FILE"
    echo "  You can delete it once you've verified the migration."
fi

echo ""
echo "To start Hanzo Node with LanceDB:"
echo -e "  ${GREEN}cargo run --release --features lancedb${NC}"
echo ""
echo "To revert to SQLite (if needed):"
echo -e "  ${YELLOW}cargo run --release --features sqlite${NC}"