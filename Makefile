.PHONY: build build-release test clean install dev fmt lint check-deps

# Default target
all: build

# Development build
build:
	cargo build --bin airstack

# Release build
build-release:
	cargo build --release --bin airstack

# Build for all platforms (requires cross)
build-all: check-deps
	cross build --release --target x86_64-unknown-linux-gnu --bin airstack
	cross build --release --target aarch64-unknown-linux-gnu --bin airstack
	cross build --release --target x86_64-apple-darwin --bin airstack
	cross build --release --target aarch64-apple-darwin --bin airstack
	cross build --release --target x86_64-pc-windows-msvc --bin airstack

# Run tests
test:
	cargo test --all-features --workspace

# Clean build artifacts
clean:
	cargo clean
	rm -rf bin/
	rm -rf dist/
	rm -rf node_modules/

# Install for development
install: build-release
	mkdir -p bin
	cp target/release/airstack bin/
	npm install
	npm run build

# Development mode with file watching
dev:
	cargo watch -x "build --bin airstack"

# Format code
fmt:
	cargo fmt --all
	npm run build && npx prettier --write src/

# Lint code
lint:
	cargo clippy --all-targets --all-features -- -D warnings
	npm run build

# Check dependencies
check-deps:
	@command -v cross >/dev/null 2>&1 || { echo >&2 "cross is required but not installed. Run: cargo install cross"; exit 1; }

# Create example project
example:
	mkdir -p example
	cd example && ../bin/airstack init example-project

# Run integration tests
test-integration: install
	cd example && ../bin/airstack status --config airstack.toml

# Setup development environment
setup:
	rustup target add x86_64-unknown-linux-gnu
	rustup target add aarch64-unknown-linux-gnu
	rustup target add x86_64-apple-darwin
	rustup target add aarch64-apple-darwin
	rustup target add x86_64-pc-windows-msvc
	cargo install cross
	cargo install cargo-watch
	npm install

# Package for distribution
package: build-release
	mkdir -p bin
	cp target/release/airstack bin/
	npm run build
	npm pack

# Quick smoke test
smoke-test: install
	./bin/airstack --version
	./bin/airstack --help
	cd /tmp && $(PWD)/bin/airstack init test-project --config test-airstack.toml
	cd /tmp && $(PWD)/bin/airstack status --config test-airstack.toml --dry-run

help:
	@echo "Available targets:"
	@echo "  build          - Build debug version"
	@echo "  build-release  - Build release version"
	@echo "  build-all      - Build for all platforms (requires cross)"
	@echo "  test           - Run all tests"
	@echo "  clean          - Clean build artifacts"
	@echo "  install        - Install for development"
	@echo "  dev            - Development mode with file watching"
	@echo "  fmt            - Format code"
	@echo "  lint           - Lint code"
	@echo "  example        - Create example project"
	@echo "  setup          - Setup development environment"
	@echo "  package        - Package for distribution"
	@echo "  smoke-test     - Run quick smoke test"
	@echo "  help           - Show this help"