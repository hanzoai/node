# Hanzo Node Makefile

.PHONY: build build-release run clean test check dev

# Default target
all: build

# Build in debug mode
build:
	cargo build --bin hanzod

# Build in release mode  
build-release:
	cargo build --release --bin hanzod

# Run the node locally
run:
	sh scripts/run_node_localhost.sh

# Development mode - build and run
dev: build
	sh scripts/run_node_localhost.sh

# Clean build artifacts
clean:
	cargo clean

# Run tests
test:
	IS_TESTING=1 cargo test -- --test-threads=1

# Check for compilation errors without building
check:
	cargo check

# Run with full stack (node + agent provider)
run-all:
	sh scripts/run_all_localhost.sh

# Build with Swagger UI support
build-swagger:
	cargo build --features hanzo_node/swagger-ui

# Format code
fmt:
	cargo fmt

# Run clippy linter
clippy:
	cargo clippy -- -W clippy::all

# Install dependencies
deps:
	cargo fetch

help:
	@echo "Available targets:"
	@echo "  make build         - Build in debug mode"
	@echo "  make build-release - Build in release mode"
	@echo "  make run          - Run the node locally"
	@echo "  make dev          - Build and run in development mode"
	@echo "  make clean        - Clean build artifacts"
	@echo "  make test         - Run tests"
	@echo "  make check        - Check for compilation errors"
	@echo "  make run-all      - Run full stack"
	@echo "  make build-swagger - Build with Swagger UI"
	@echo "  make fmt          - Format code"
	@echo "  make clippy       - Run clippy linter"
	@echo "  make deps         - Fetch dependencies"