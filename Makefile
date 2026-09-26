SHELL := /bin/bash
PY := uv
SIDECAR := sidecar
UI := ui
RUST := crates/app

.PHONY: help setup format lint typecheck test test-py test-rust build-ui check dev build bundle clean

help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

setup: ## Install all dependencies
	cd $(SIDECAR) && $(PY) sync --all-groups
	cd $(UI) && npm install

format: ## Auto-format Python and Rust
	cd $(SIDECAR) && $(PY) run ruff format .
	cd $(SIDECAR) && $(PY) run ruff check --fix .
	cd $(RUST) && cargo fmt

lint: ## Lint Python + Rust (no writes)
	cd $(SIDECAR) && $(PY) run ruff format --check .
	cd $(SIDECAR) && $(PY) run ruff check .
	cd $(RUST) && cargo fmt --check
	cd $(RUST) && cargo clippy --all-targets -- -D warnings

typecheck: ## Type-check Python
	cd $(SIDECAR) && $(PY) run pyright

test: test-py test-rust ## Run all tests

test-py: ## Run sidecar tests
	cd $(SIDECAR) && $(PY) run pytest

test-rust: ## Run Rust tests
	cd $(RUST) && cargo test

test-ui: ## Run the optional frontend unit tests
	cd $(UI) && npm run test

build-ui: ## Build the frontend (required before cargo build)
	cd $(UI) && npm run build

# The frontend bundle is a COMPILE-time input: `tauri::generate_context!` reads
# `ui/dist` while the crate is built, so clippy and test fail on a clean tree if the
# UI has not been built yet. build-ui must therefore come first, and a stale `dist`
# left over from an earlier run must not be what makes this target pass.
check: build-ui lint typecheck test ## Full gate: ui build + lint + type + test
	cd $(RUST) && cargo check

dev: ## Run the desktop app in dev mode (requires llama-server running)
	cd $(RUST) && cargo tauri dev

build: ## Build release bundle
	cd $(SIDECAR) && $(PY) run python -m build_sidecar
	cd $(RUST) && cargo tauri build

clean: ## Remove build artifacts
	rm -rf $(SIDECAR)/.venv $(SIDECAR)/.pytest_cache $(SIDECAR)/.ruff_cache
	rm -rf $(UI)/node_modules $(UI)/dist
	rm -rf $(RUST)/target
