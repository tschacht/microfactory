#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/run_unit_tests.sh" "$@"
"$SCRIPT_DIR/run_integration_tests.sh" "$@"
