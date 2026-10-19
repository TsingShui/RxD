#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
exec python3 "$root/scripts/build-ilspy.py" "${1:-osx-arm64}"
