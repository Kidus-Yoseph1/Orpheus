#!/usr/bin/env bash
# One-command launcher: make sure the TTS worker is up, then open the GUI.
#
# Usage:
#   ./run-gui.sh                 # open the library
#   ./run-gui.sh book.epub       # open a book
#   ORPHEUS_PYTHON=/path ./run-gui.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
URL="http://127.0.0.1:8765"

# Python that has torch + kokoro. Override with ORPHEUS_PYTHON.
find_worker_py() {
    if [ -n "${ORPHEUS_PYTHON:-}" ]; then echo "$ORPHEUS_PYTHON"; return; fi
    for c in \
        "$HERE/tts-worker/.venv312/bin/python" \
        "$HOME/Documents/epub-to-voice/.venv/bin/python" \
        python3 python; do
        if command -v "$c" >/dev/null 2>&1 && "$c" -c "import kokoro" 2>/dev/null; then
            command -v "$c"; return
        fi
    done
}

health() { curl -s --max-time 2 "$URL/health" 2>/dev/null | grep -q '"ok": true'; }

if ! health; then
    PY="$(find_worker_py || true)"
    if [ -n "$PY" ]; then
        echo "starting TTS worker ($PY)…"
        setsid "$HERE/tts-worker/run.sh" --python "$PY" </dev/null >"${TMPDIR:-/tmp}/orpheus-worker.log" 2>&1 &
        disown 2>/dev/null || true
        for _ in $(seq 1 60); do
            if health; then break; fi
            sleep 1
        done
        if ! health; then echo "warning: worker did not come up; see ${TMPDIR:-/tmp}/orpheus-worker.log"; fi
    else
        echo "warning: no Python with kokoro found; narration will not work"
    fi
fi

# Prefer a built binary; fall back to cargo run.
if [ -x "$HERE/target/release/orpheus-gui" ]; then
    BIN="$HERE/target/release/orpheus-gui"
elif [ -x "$HERE/target/debug/orpheus-gui" ]; then
    BIN="$HERE/target/debug/orpheus-gui"
else
    exec cargo run --manifest-path "$HERE/Cargo.toml" -p orpheus-gui -- "$@"
fi
exec "$BIN" "$@"
