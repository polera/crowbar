.PHONY: checks lint audit tools help

help: ## Show available commands
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

checks: lint audit ## Run all checks (lint + audit)

lint: ## Run clippy lints
	cargo clippy -- -D warnings

audit: tools ## Run cargo audit
	cargo audit

tools: ## Install required tooling
	rustup component add clippy
	@command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit
