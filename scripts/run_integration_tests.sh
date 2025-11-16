#!/usr/bin/env bash
set -euo pipefail

cargo test --quiet -p integration-tests "$@"
