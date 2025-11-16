SHELL := /bin/bash
.DEFAULT_GOAL := help

.PHONY: help build fmt fmt-check clippy unit-test integration-test test serve clean clean-data clean-all ensure-pristine deps-update deps-upgrade

help:
	@echo "Available targets:"
	@echo "  make build              # cargo build for all crates"
	@echo "  make fmt                # format the workspace"
	@echo "  make fmt-check          # check formatting"
	@echo "  make clippy             # run clippy with warnings as errors"
	@echo "  make unit-test          # run workspace unit tests (ex integration)"
	@echo "  make integration-test   # run integration test crate"
	@echo "  make test               # run unit + integration tests"
	@echo "  make serve              # launch HTTP session service"
	@echo "  make clean              # cargo clean"
	@echo "  make clean-data         # remove persisted session data"
	@echo "  make clean-all          # clean build + session data"
	@echo "  make ensure-pristine    # clean, format, clippy --fix, build, and test"
	@echo "  make deps-update        # refresh Cargo.lock within current semver ranges"
	@echo "  make deps-upgrade       # bump Cargo.toml deps via cargo-upgrade (cargo-edit)"

build:
	cargo build --workspace

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

clippy:
	cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets --all-features -- -D warnings

unit-test:
	scripts/run_unit_tests.sh

integration-test:
	scripts/run_integration_tests.sh

test:
	scripts/run_all_tests.sh

serve:
	cargo run -- serve

clean:
	cargo clean

clean-data:
	scripts/clean_sessions.sh

clean-all: clean clean-data

ensure-pristine:
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) build
	$(MAKE) test

deps-update:
	cargo update

deps-upgrade:
	@command -v cargo-upgrade >/dev/null 2>&1 || (echo "cargo-upgrade not found. Install via 'cargo install cargo-edit'." && exit 1)
	cargo upgrade
