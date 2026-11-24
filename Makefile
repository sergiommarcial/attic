SHELL := /bin/bash
export PATH := $(HOME)/.cargo/bin:$(PATH)
# Cargo builds registry crates from their own folders, and some ship a
# rust-toolchain.toml that would make rustup's proxy pick an old compiler.
export RUSTUP_TOOLCHAIN ?= stable

TESTS ?=

.PHONY: help build test test-fast test-scripts check-formula clippy fmt fmt-check deny machete lint-ci ci install install-tools

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-14s %s\n", $$1, $$2}'

build: ## Build everything
	cargo build --workspace --locked

test: ## Run every test
	cargo test --workspace --locked

test-fast: ## Unit tests plus the integration files in TESTS ("store scan")
	cargo test -p attic-core --locked --lib $(foreach t,$(TESTS),--test $(t))

test-scripts: ## Test the release scripts
	sh scripts/test-release-scripts.sh

check-formula: ## Style and audit the Homebrew formula in a throwaway tap
	sh scripts/check-formula.sh

clippy: ## Lint, warnings are errors
	cargo clippy --workspace --all-targets --locked -- -D warnings

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Check formatting
	cargo fmt --all --check

deny: ## Licenses and advisories
	cargo deny check

machete: ## Unused dependencies
	cargo machete

lint-ci: ## Lint the workflows and scripts
	actionlint
	shellcheck scripts/*.sh

ci: fmt-check clippy test test-scripts check-formula lint-ci deny machete ## What CI would run

install: ## Install the attic binary into ~/.cargo/bin
	cargo install --path crates/attic --locked

install-tools: ## Install cargo-deny, cargo-machete, actionlint and shellcheck
	cargo install --locked cargo-deny cargo-machete
	brew install actionlint shellcheck
