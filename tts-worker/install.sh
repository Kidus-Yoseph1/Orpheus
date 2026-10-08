#!/usr/bin/env bash
# Conda-free worker setup.
#
# Reuses a Python that already has torch, so there is no multi-GB torch
# download. Kokoro English narration needs only torch + kokoro + soundfile.
# Voice cloning needs OpenVoice on top and is optional.
#
# Usage:
#   ./install.sh [python] [--cloning] [--install-torch]
#
# Examples:
#   ./install.sh /path/to/venv/bin/python     # reuse an env that has torch
#   ORPHEUS_PYTHON=/path/to/python ./install.sh
#   ./install.sh python3 --install-torch      # only if you have no torch yet
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

CLONING=0
INSTALL_TORCH=0
PY=""
for a in "$@"; do
    case "$a" in
        --cloning) CLONING=1 ;;
        --install-torch) INSTALL_TORCH=1 ;;
        -*) echo "unknown flag: $a" >&2; exit 2 ;;
        *) PY="$a" ;;
    esac
done

# Locate a Python: explicit arg > $ORPHEUS_PYTHON > first one that can import
# torch > plain python3.
if [ -z "$PY" ] && [ -n "${ORPHEUS_PYTHON:-}" ]; then
    PY="$ORPHEUS_PYTHON"
fi
if [ -z "$PY" ]; then
    for c in "$HERE/.venv312/bin/python" python3 python; do
        if command -v "$c" >/dev/null 2>&1 && "$c" -c "import torch" 2>/dev/null; then
            PY="$(command -v "$c")"; break
        fi
    done
fi
if [ -z "$PY" ]; then
    PY="$(command -v python3 || command -v python || true)"
fi
[ -n "$PY" ] || { echo "error: no Python found; pass one: ./install.sh /path/to/python" >&2; exit 1; }
echo "python: $PY"
"$PY" --version

# Prefer uv (works even when the env has no pip, e.g. uv-created venvs).
install_pkgs() {
    if command -v uv >/dev/null 2>&1; then
        uv pip install --python "$PY" "$@"
    elif "$PY" -m pip --version >/dev/null 2>&1; then
        "$PY" -m pip install "$@"
    else
        echo "error: need uv or pip to install packages" >&2
        return 1
    fi
}

if ! "$PY" -c "import torch" 2>/dev/null; then
    if [ "$INSTALL_TORCH" = "1" ]; then
        echo "installing torch + torchaudio (large download)..."
        install_pkgs torch torchaudio
    else
        echo "error: $PY has no torch." >&2
        echo "Point me at a Python that has torch, or pass --install-torch." >&2
        echo "  ./install.sh /path/to/venv/bin/python" >&2
        exit 1
    fi
fi
"$PY" -c "import torch; print('torch', torch.__version__, 'cuda', torch.cuda.is_available(), torch.cuda.get_device_name(0) if torch.cuda.is_available() else '')"

echo "installing worker deps (packages already present are skipped)..."
install_pkgs -r "$HERE/requirements.txt"

if [ "$CLONING" = "1" ]; then
    echo "installing OpenVoice + librosa (voice cloning)..."
    install_pkgs librosa
    install_pkgs --no-deps "git+https://github.com/myshell-ai/OpenVoice.git"
fi

"$PY" -c "import kokoro, soundfile; print('worker env OK')"
echo
echo "run the worker with:"
echo "  $HERE/run.sh --python $PY"
