#!/usr/bin/env python3
"""Approach B model pull: HF repo -> <models>/<id>/ + orpheus-manifest.json.

The manifest schema MUST match `ModelManifest` in
`crates/orpheus-tts/src/store.rs` (version/id/hf_repo/revision/backend/
files/total_bytes/installed_at). The Rust reader only reads manifests.

Examples:
    python download.py --id kokoro --repo hexgrad/Kokoro-82M --backend kokoro
    python download.py --id hexgrad/Kokoro-82M --repo hexgrad/Kokoro-82M \\
        --backend custom --dry-run
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

MANIFEST_FILE = "orpheus-manifest.json"
MANIFEST_VERSION = 1


def fail(msg: str) -> "NoReturn":  # noqa: F821
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(1)


def validate_id(model_id: str) -> Path:
    """Map a model id to a relative dir. Mirrors ModelStore::model_dir."""
    if not model_id or len(model_id) > 200:
        fail(f"invalid model id: {model_id!r}")
    parts = Path(model_id).parts
    if len(parts) > 2 or any(p in (".", "..", "/") or os.path.isabs(p) for p in parts):
        fail(f"invalid model id: {model_id!r}")
    rel = Path(*parts)
    if str(rel) in (".", ""):
        fail(f"invalid model id: {model_id!r}")
    return rel


def default_models_dir() -> Path:
    xdg = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(xdg) / "orpheus" / "models"


def collect_files(root: Path) -> tuple[list[str], int]:
    files: list[str] = []
    total = 0
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for name in sorted(filenames):
            if name == MANIFEST_FILE:
                continue
            full = Path(dirpath) / name
            rel = full.relative_to(root).as_posix()
            files.append(rel)
            total += full.stat().st_size
    return files, total


def main() -> None:
    ap = argparse.ArgumentParser(description="Pull a TTS model into the Orpheus store.")
    ap.add_argument("--id", required=True, help="model id, e.g. kokoro or org/name")
    ap.add_argument("--repo", required=True, help="HF repo, e.g. hexgrad/Kokoro-82M")
    ap.add_argument("--backend", default="custom", help="kokoro | chatterbox | custom...")
    ap.add_argument("--rev", default="main", help="branch/tag/commit (resolved + recorded)")
    ap.add_argument("--models-dir", default=str(default_models_dir()))
    ap.add_argument("--offline", action="store_true", help="local files only, no network")
    ap.add_argument("--dry-run", action="store_true", help="print plan, download nothing")
    args = ap.parse_args()

    rel = validate_id(args.id)
    model_dir = Path(args.models_dir) / rel

    try:
        from huggingface_hub import HfApi, snapshot_download
    except ImportError:
        fail("huggingface_hub not installed (pip install -r requirements.txt)")

    api = HfApi()
    try:
        info = api.model_info(args.repo, revision=None if args.offline else args.rev)
    except Exception as e:  # network or unknown repo
        fail(f"cannot resolve {args.repo}@{args.rev}: {e}")
    resolved_rev = info.sha
    approx_bytes = sum(s.size or 0 for s in (info.siblings or []))

    print(f"model:   {args.id}")
    print(f"repo:    {args.repo}@{resolved_rev[:12]}")
    print(f"target:  {model_dir}")
    if approx_bytes > 0:
        print(f"approx:  {approx_bytes / 1e6:.1f} MB")
    else:
        # HF doesn't always report sibling sizes; real size is tallied after pull.
        print("approx:  unknown (tallied after pull)")
    if args.dry_run:
        print("dry run — nothing downloaded.")
        return

    if args.offline:
        os.environ["HF_HUB_OFFLINE"] = "1"
    else:
        os.environ.setdefault("HF_HUB_ENABLE_HF_TRANSFER", "1")

    model_dir.mkdir(parents=True, exist_ok=True)
    try:
        snapshot_download(
            repo_id=args.repo,
            revision=args.rev if not args.offline else None,
            local_dir=str(model_dir),
            local_dir_use_symlinks=False,
        )
    except Exception as e:
        fail(f"download failed: {e}")

    files, total_bytes = collect_files(model_dir)
    manifest = {
        "version": MANIFEST_VERSION,
        "id": args.id,
        "hf_repo": args.repo,
        "revision": resolved_rev,
        "backend": args.backend,
        "files": files,
        "total_bytes": total_bytes,
        "installed_at": datetime.now(timezone.utc).isoformat(),
    }
    (model_dir / MANIFEST_FILE).write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"installed: {len(files)} files, {total_bytes / 1e6:.1f} MB")
    print(f"manifest:  {model_dir / MANIFEST_FILE}")


if __name__ == "__main__":
    main()
