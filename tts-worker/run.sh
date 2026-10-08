#!/usr/bin/env bash
# Conda-free worker launcher. Finds a Python that has kokoro (or take one with
# --python) and starts book-tts. Extra server flags are passed through.
#
# Usage:
#   ./run.sh                                  # auto-detect a Python with kokoro
#   ./run.sh --python /path/to/venv/bin/python
#   ./run.sh --device cpu --port 9000
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

PY="${ORPHEUS_PYTHON:-}"
if [ "${1:-}" = "--python" ]; then
    PY="$2"
    shift 2
fi
if [ -z "$PY" ]; then
    for c in "$HERE/.venv312/bin/python" python3 python; do
        if command -v "$c" >/dev/null 2>&1 && "$c" -c "import kokoro" 2>/dev/null; then
            PY="$(command -v "$c")"; break
        fi
    done
fi
[ -n "$PY" ] || {
    echo "error: no Python with kokoro found; run ./install.sh first" >&2
    exit 1
}
echo "worker python: $PY"
exec "$PY" "$HERE/server.py" --model kokoro --device auto --preload "$@"
