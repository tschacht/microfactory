#!/usr/bin/env bash
set -euo pipefail

ROOT="${MICROFACTORY_HOME:-${HOME:-}}"
if [[ -z "$ROOT" ]]; then
  echo "MICROFACTORY_HOME or HOME must be set to clean sessions" >&2
  exit 1
fi
DATA_DIR="$ROOT/.microfactory"
if [[ -d "$DATA_DIR" ]]; then
  rm -rf "$DATA_DIR"
  echo "Removed session data at $DATA_DIR"
else
  echo "No session data found at $DATA_DIR"
fi
