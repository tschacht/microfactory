#!/usr/bin/env bash
set -euo pipefail

cargo test --quiet --workspace --exclude integration-tests "$@"
