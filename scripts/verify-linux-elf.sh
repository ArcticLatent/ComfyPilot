#!/usr/bin/env bash
set -euo pipefail

binary="${1:-}"
expected_interpreter="${2:-}"

if [[ -z "$binary" || ! -f "$binary" ]]; then
  echo "Usage: $0 <ELF binary> [expected interpreter]" >&2
  exit 2
fi

if ! command -v readelf >/dev/null 2>&1; then
  echo "Required command not found: readelf" >&2
  exit 2
fi

program_headers="$(readelf -l "$binary")"
dynamic_section="$(readelf -d "$binary")"
interpreter="$(
  printf '%s\n' "$program_headers" |
    sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p'
)"

if [[ -z "$interpreter" ]]; then
  echo "Unable to determine the ELF interpreter for $binary" >&2
  exit 1
fi

if [[ -n "$expected_interpreter" && "$interpreter" != "$expected_interpreter" ]]; then
  echo "Unexpected ELF interpreter for $binary: $interpreter" >&2
  echo "Expected: $expected_interpreter" >&2
  exit 1
fi

if printf '%s\n%s\n' "$program_headers" "$dynamic_section" |
  grep -qE '/nix/store|/home/'; then
  echo "Build-machine path leaked into $binary" >&2
  printf '%s\n' "$program_headers" "$dynamic_section" |
    grep -E '/nix/store|/home/' >&2 || true
  exit 1
fi

echo "Verified Linux ELF: $binary"
echo "  Interpreter: $interpreter"
