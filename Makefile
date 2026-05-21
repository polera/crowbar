.PHONY: checks lint audit tools help release clean \
       linux-amd64 linux-arm64 macos-amd64 macos-arm64 \
       freebsd-amd64 freebsd-arm64 openbsd-amd64 openbsd-arm64

BINARY := crowbar
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
DIST := dist
CROSS := cross

help: ## Show available commands
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

checks: lint audit ## Run all checks (lint + audit)

lint: ## Run clippy lints
	cargo clippy -- -D warnings

audit: tools ## Run cargo audit
	cargo audit

tools: ## Install required tooling
	rustup component add clippy
	@command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit
	@command -v cross >/dev/null 2>&1 || cargo install cross

# --- Release builds ---

release: linux-amd64 linux-arm64 macos-amd64 macos-arm64 \
         freebsd-amd64 freebsd-arm64 openbsd-amd64 openbsd-arm64 ## Build all release binaries
	@echo "\nRelease binaries:"
	@ls -lh $(DIST)/

$(DIST):
	mkdir -p $(DIST)

linux-amd64: $(DIST) ## Build for Linux x86_64 (static musl)
	$(CROSS) build --release --target x86_64-unknown-linux-musl
	cp target/x86_64-unknown-linux-musl/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-linux-amd64

linux-arm64: $(DIST) ## Build for Linux aarch64 (static musl)
	$(CROSS) build --release --target aarch64-unknown-linux-musl
	cp target/aarch64-unknown-linux-musl/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-linux-arm64

macos-amd64: $(DIST) ## Build for macOS x86_64
	$(CROSS) build --release --target x86_64-apple-darwin
	cp target/x86_64-apple-darwin/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-darwin-amd64

macos-arm64: $(DIST) ## Build for macOS aarch64
	$(CROSS) build --release --target aarch64-apple-darwin
	cp target/aarch64-apple-darwin/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-darwin-arm64

freebsd-amd64: $(DIST) ## Build for FreeBSD x86_64
	$(CROSS) build --release --target x86_64-unknown-freebsd
	cp target/x86_64-unknown-freebsd/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-freebsd-amd64

freebsd-arm64: $(DIST) ## Build for FreeBSD aarch64
	$(CROSS) build --release --target aarch64-unknown-freebsd
	cp target/aarch64-unknown-freebsd/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-freebsd-arm64

openbsd-amd64: $(DIST) ## Build for OpenBSD x86_64
	$(CROSS) build --release --target x86_64-unknown-openbsd
	cp target/x86_64-unknown-openbsd/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-openbsd-amd64

openbsd-arm64: $(DIST) ## Build for OpenBSD aarch64
	$(CROSS) build --release --target aarch64-unknown-openbsd
	cp target/aarch64-unknown-openbsd/release/$(BINARY) $(DIST)/$(BINARY)-$(VERSION)-openbsd-arm64

clean: ## Remove build artifacts
	cargo clean
	rm -rf $(DIST)
