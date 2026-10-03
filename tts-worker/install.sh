#!/usr/bin/env bash
# Reproducible worker env: reuse an env that already has torch+CUDA (here
# ml_base), keeping the base env untouched via an isolated venv.
# OpenVoice's pins are ancient (numpy 1.22 source build) so it installs
# with --no-deps; runtime-tested imports pull the modern remainder.
#
# Usage: ./install.sh [conda-env-name]   (default: ml_base)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WANT_ENV="${1:-ml_base}"

find_conda_base() {
    if [ -n "${CONDA_EXE:-}" ]; then
        dirname "$(dirname "$CONDA_EXE")"
        return 0
    fi
    if command -v conda >/dev/null 2>&1; then
        conda info --base 2>/dev/null && return 0
    fi
    for d in "$HOME/miniconda3" "$HOME/anaconda3" /opt/miniconda3; do
        [ -x "$d/bin/python" ] && { echo "$d"; return 0; }
    done
    return 1
}

BASE="$(find_conda_base)" || {
    echo "error: no conda found; install miniconda or pass a python explicitly" >&2
    exit 1
}
PY="$BASE/envs/$WANT_ENV/bin/python"
[ -x "$PY" ] || {
    echo "error: no python at $PY (conda env '$WANT_ENV' missing?)" >&2
    exit 1
}
"$PY" --version

"$PY" -m venv --system-site-packages "$HERE/.venv312"
VENV="$HERE/.venv312/bin/pip"
"$VENV" install --no-deps git+https://github.com/myshell-ai/OpenVoice.git
"$VENV" install -r "$HERE/requirements.txt"
"$HERE/.venv312/bin/python" -c "from openvoice import se_extractor; from openvoice.api import ToneColorConverter; print('worker env OK')"
