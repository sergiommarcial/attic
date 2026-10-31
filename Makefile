SHELL := /bin/bash
export PATH := $(HOME)/.cargo/bin:$(PATH)

TESTS ?=

.PHONY: help build test test-fast clippy fmt fmt-check deny machete ci install install-tools

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-14s %s\n", $$1, $$2}'

build: ## Build everything
	cargo build --workspace --locked

test: ## Run every test
	cargo test --workspace --locked

test-fast: ## Unit tests plus the integration files in TESTS ("store scan")
	cargo test -p attic-core --locked --lib $(foreach t,$(TESTS),--test $(t))

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

ci: fmt-check clippy test deny machete ## What CI would run

install: ## Install the attic binary into ~/.cargo/bin
	cargo install --path crates/attic --locked

install-tools: ## Install cargo-deny and cargo-machete
	cargo install --locked cargo-deny cargo-machete
