#!/usr/bin/env bash
# Builds and runs the app from source with the catalog settings from .env,
# which the build embeds the same way release builds do. Needs Qt 6 (qmake6
# or qmake on PATH) for cxx-qt. Extra arguments go to the app.
#
#   scripts/dev.sh              use your normal app data (settings, logs, cache)
#   scripts/dev.sh --isolated   use ~/arctic-test-data instead, leaving the
#                               installed app's data alone
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

env_file="${ARCTIC_ENV_FILE:-$ROOT_DIR/.env}"
if [[ -f "$env_file" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a
else
  echo "No $env_file: the catalog will be unavailable (see .env.sample)." >&2
fi

if [[ "${1:-}" == "--isolated" ]]; then
  shift
  export XDG_DATA_HOME="$HOME/arctic-test-data"
  echo "App data: $XDG_DATA_HOME/io.github.ArcticLatent.ComfyPilot"
fi

# QML warnings go to the terminal even when stderr isn't one.
export QT_FORCE_STDERR_LOGGING=1
exec cargo run --manifest-path app/Cargo.toml -- "$@"
