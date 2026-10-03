#!/usr/bin/env python3
"""book-tts — Orpheus TTS worker.

Serves one model family at a time over localhost HTTP. The Rust reader only
speaks this API (GUIDE §3) and never touches model code.

First backend: Kokoro-82M, loaded from the app-owned flat store
(`<models>/kokoro/{config.json,kokoro-v1_0.pth,voices/*.pt}` + manifest).
No HuggingFace cache is touched at serve time.

Endpoints:
    GET  /health      -> {ok, model, loaded, device}
    GET  /voices      -> {voices: [{id, name}]}
    POST /synthesize  -> {request_id, audio_path, duration_secs}
    POST /unload      -> {ok}   (drop model, free VRAM on model switch)

Synthesize request:  {request_id, model, voice, text, speed}
- Always renders at 1.0x; playback speed is applied by the player (ffplay
  atempo), so the on-disk cache key is sha256(model|voice|text) and the
  returned duration is the 1x duration.
- `voice`: "kokoro-default" (= af_heart) or a built-in stem (af_bella, ...).

Run:
    book-tts --model kokoro [--port 8765] [--device auto] [--preload]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

MANIFEST_FILE = "orpheus-manifest.json"
SAMPLE_RATE = 24000
DEFAULT_VOICE = "af_heart"
VOICE_RE = re.compile(r"^[a-z][a-z0-9_]*$")
# Clone ids are user slugs ("sarah-the-narrator"); allow hyphens there.
CLONE_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
# Neutral base voice rendered before conversion; timbre comes from the clone.
CLONE_BASE_VOICE = "af_heart"


def disable_watermark() -> None:
    """OpenVoice imports wavmark (extra model download + VRAM) for audio
    watermarking. Personal audiobooks don't need it — stub before import."""
    import types

    if "wavmark" in sys.modules:
        return

    class _NoWM:
        def to(self, device):
            return None

    fake = types.ModuleType("wavmark")
    fake.load_model = lambda *a, **k: _NoWM()
    sys.modules["wavmark"] = fake


def log(msg: str) -> None:
    print(f"[book-tts] {msg}", flush=True)


def default_models_dir() -> Path:
    xdg = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(xdg) / "orpheus" / "models"


def default_cache_dir() -> Path:
    xdg = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(xdg) / "orpheus" / "cache" / "audio"


def cache_key(model: str, voice: str, text: str) -> str:
    h = hashlib.sha256()
    h.update(b"orpheus-v1\x00")
    h.update(model.encode() + b"\x00" + voice.encode() + b"\x00" + text.encode())
    return h.hexdigest()[:32]


def write_wav(path: Path, pcm_bytes: bytes) -> None:
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm_bytes)


def wav_duration(path: Path) -> float:
    with wave.open(str(path), "rb") as w:
        return w.getnframes() / float(w.getframerate() or SAMPLE_RATE)


def probe_duration(path: Path) -> float:
    """Duration for opus hits (no mutagen dep — ffprobe ships with ffmpeg)."""
    if path.suffix == ".wav":
        try:
            return wav_duration(path)
        except Exception:
            return 0.0
    ffprobe = shutil.which("ffprobe")
    if not ffprobe:
        return 0.0
    try:
        r = subprocess.run(
            [ffprobe, "-v", "error", "-show_entries", "format=duration",
             "-of", "default=noprint_wrappers=1:nokey=1", str(path)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return float(r.stdout.strip())
    except Exception:
        return 0.0


def opus_encode(wav_path: Path, opus_path: Path) -> bool:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return False
    try:
        r = subprocess.run(
            [ffmpeg, "-y", "-loglevel", "error",
             "-i", str(wav_path), "-c:a", "libopus", "-b:a", "48k", str(opus_path)],
            capture_output=True,
            timeout=120,
        )
        return r.returncode == 0 and opus_path.is_file()
    except Exception:
        return False


class KokoroEngine:
    """Kokoro renderer + optional OpenVoice conversion for cloned voices.

    Built-in voices render straight from Kokoro. A voice id matching
    `<voices-dir>/<id>.wav` takes the clone path: render neutral base with
    Kokoro, then convert timbre (speaker embedding cached as `<id>.se.pt`).
    The converter loads lazily on first clone use, so kokoro-only sessions
    stay at ~1.4 GB VRAM.
    """

    def __init__(self, model_id: str, model_dir: Path, device: str,
                 voices_dir: Path, ov_model_dir: Path):
        self.model_id = model_id
        self.model_dir = model_dir
        self.voices_dir = voices_dir
        self.ov_model_dir = ov_model_dir
        self.want_device = device
        self.device = "cpu"
        self.pipe = None
        self.kmodel = None
        self.converter = None
        self.lock = threading.Lock()
        manifest = json.loads((model_dir / MANIFEST_FILE).read_text())
        self.revision = manifest.get("revision", "")

    def voices(self) -> list[dict]:
        out = [{"id": "kokoro-default", "name": "Default (af_heart)"}]
        vdir = self.model_dir / "voices"
        if vdir.is_dir():
            for p in sorted(vdir.glob("*.pt")):
                stem = p.stem
                if VOICE_RE.match(stem) and stem != "kokoro-default":
                    out.append({"id": stem, "name": stem})
        if self.voices_dir.is_dir():
            for p in sorted(self.voices_dir.glob("*.wav")):
                stem = p.stem
                if CLONE_RE.match(stem):
                    out.append({"id": stem, "name": f"{stem} (clone)"})
        return out

    def resolve_voice(self, voice: str) -> tuple[str, Path]:
        """-> (kind, path): kind is 'kokoro' (voice .pt) or 'clone' (ref .wav)."""
        if voice in ("kokoro-default", "default", ""):
            return ("kokoro", self.model_dir / "voices" / f"{DEFAULT_VOICE}.pt")
        if VOICE_RE.match(voice):
            p = self.model_dir / "voices" / f"{voice}.pt"
            if p.is_file():
                return ("kokoro", p)
        if CLONE_RE.match(voice):
            p = self.voices_dir / f"{voice}.wav"
            if p.is_file():
                return ("clone", p)
        raise ValueError(f"unknown voice: {voice!r}")

    def ensure_loaded(self):
        if self.pipe is not None:
            return
        import torch

        from kokoro import KPipeline
        from kokoro.model import KModel

        want = self.want_device
        if want == "auto":
            want = "cuda" if torch.cuda.is_available() else "cpu"
        cfg = self.model_dir / "config.json"
        weights = self.model_dir / "kokoro-v1_0.pth"
        missing = [str(p) for p in (cfg, weights) if not p.is_file()]
        if missing:
            raise RuntimeError(f"model files missing in {self.model_dir}: {missing}")
        try:
            kmodel = KModel(config=str(cfg), model=str(weights)).to(want).eval()
        except Exception as e:
            if want == "cuda":
                log(f"cuda load failed ({e}); falling back to cpu")
                want = "cpu"
                kmodel = KModel(config=str(cfg), model=str(weights)).to(want).eval()
            else:
                raise
        self.kmodel = kmodel
        self.pipe = KPipeline(lang_code="a", model=kmodel, device=want)
        self.device = want
        log(f"loaded {self.model_id} on {want} (rev {self.revision[:12]})")

    def unload(self):
        import torch

        self.pipe = None
        self.kmodel = None
        self.converter = None
        if torch.cuda.is_available():
            torch.cuda.empty_cache()
        self.device = "cpu"
        log(f"unloaded {self.model_id}")

    def ensure_converter(self):
        if self.converter is not None:
            return
        import torch

        disable_watermark()
        from openvoice.api import ToneColorConverter

        ckpt = self.ov_model_dir / "converter" / "checkpoint.pth"
        cfg = self.ov_model_dir / "converter" / "config.json"
        if not ckpt.is_file() or not cfg.is_file():
            raise RuntimeError(
                f"openvoice files missing in {self.ov_model_dir}/converter "
                f"(pull: download.py --id openvoice-v2 --repo myshell-ai/OpenVoiceV2)"
            )
        conv = ToneColorConverter(str(cfg), device=self.device)
        conv.watermark_model = None
        conv.load_ckpt(str(ckpt))
        self.converter = conv
        log(f"openvoice converter ready ({round(torch.cuda.memory_allocated()/1e6)} MB total)")

    def clone_embedding(self, voice_wav: Path):
        """Speaker embedding for a reference clip, cached next to the voice."""
        import torch

        self.ensure_converter()
        assert self.converter is not None
        se_path = voice_wav.with_suffix(".se.pt")
        if se_path.is_file():
            try:
                se = torch.load(str(se_path), map_location="cpu", weights_only=True)
                # Cached file is CPU; the converter may live on CUDA.
                return se.to(self.device) if isinstance(se, torch.Tensor) else se
            except Exception:
                pass
        se = self.converter.extract_se([str(voice_wav)], se_save_path=str(se_path))
        if isinstance(se, torch.Tensor) and str(se.device) != self.device:
            try:
                return se.to(self.device)
            except Exception:
                pass
        return se

    def synthesize(self, text: str, voice: tuple[str, Path]) -> tuple[bytes, float]:
        """Returns (pcm16 mono bytes @24k, duration secs). Sentences are short;
        guard against pathological input length."""
        import torch

        kind, vpath = voice
        clean = " ".join(text.split())
        if not clean:
            raise ValueError("empty text")
        if len(clean) > 2000:
            clean = clean[:2000]
        with self.lock:
            self.ensure_loaded()
            assert self.pipe is not None
            if kind == "kokoro":
                return self._render(clean, str(vpath))
            return self._render_clone(clean, vpath)

    def _render(self, clean: str, voice_ref: str) -> tuple[bytes, float]:
        import torch

        assert self.pipe is not None
        with torch.no_grad():
            # pipe(text, voice) handles G2P + chunking; one sentence
            # normally yields one segment; concat defensively.
            chunks = []
            for res in self.pipe(clean, voice=voice_ref, speed=1):
                if res.output is not None and res.output.audio is not None:
                    chunks.append(res.output.audio.detach().cpu().float())
        if not chunks:
            raise ValueError("nothing speakable in text")
        audio = torch.cat(chunks) if len(chunks) > 1 else chunks[0]
        return self._to_pcm(audio)

    def _render_clone(self, clean: str, voice_wav: Path) -> tuple[bytes, float]:
        import torch

        assert self.pipe is not None
        # 1. Neutral base render with Kokoro (prosody + pronunciation).
        base_ref = str(self.model_dir / "voices" / f"{CLONE_BASE_VOICE}.pt")
        with torch.no_grad():
            chunks = []
            for res in self.pipe(clean, voice=base_ref, speed=1):
                if res.output is not None and res.output.audio is not None:
                    chunks.append(res.output.audio.detach().cpu().float())
        if not chunks:
            raise ValueError("nothing speakable in text")
        base = torch.cat(chunks) if len(chunks) > 1 else chunks[0]
        # 2. Convert timbre to the clone (embeddings cached on disk).
        self.ensure_converter()
        assert self.converter is not None
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            src_wav = Path(tmp) / "base.wav"
            write_wav(src_wav, self._to_pcm(base)[0])
            src_se = self.converter.extract_se([str(src_wav)])
            tgt_se = self.clone_embedding(voice_wav)
            out_wav = Path(tmp) / "out.wav"
            with torch.no_grad():
                self.converter.convert(str(src_wav), src_se, tgt_se, output_path=str(out_wav))
            import librosa

            y, _ = librosa.load(str(out_wav), sr=SAMPLE_RATE, mono=True)
            pcm_t = (torch.from_numpy(y).clamp(-1.0, 1.0) * 32767.0).to(torch.int16)
            return self._to_pcm_raw(pcm_t), len(pcm_t) / SAMPLE_RATE

    @staticmethod
    def _to_pcm(audio: "torch.Tensor") -> tuple[bytes, float]:
        import torch

        pcm = (audio.clamp(-1.0, 1.0) * 32767.0).to(torch.int16)
        return KokoroEngine._to_pcm_raw(pcm), len(pcm) / SAMPLE_RATE

    @staticmethod
    def _to_pcm_raw(pcm: "torch.Tensor") -> bytes:
        try:
            import numpy  # noqa: F401
            return pcm.numpy().tobytes()
        except ImportError:
            import array

            return array.array("h", pcm.tolist()).tobytes()


class Handler(BaseHTTPRequestHandler):
    server_version = "book-tts/0.1"

    def _send(self, code: int, obj: dict) -> None:
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self) -> dict:
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = 0
        if n <= 0 or n > 1_000_000:
            return {}
        try:
            return json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            return {}

    def log_message(self, *args) -> None:  # quieter; errors still surface
        pass

    def do_GET(self):
        eng: KokoroEngine = self.server.engine  # type: ignore[attr-defined]
        path = urlparse(self.path).path
        if path == "/health":
            self._send(200, {
                "ok": True,
                "model": eng.model_id,
                "loaded": eng.pipe is not None,
                "device": eng.device,
                "revision": eng.revision,
            })
        elif path == "/voices":
            self._send(200, {"voices": eng.voices()})
        else:
            self._send(404, {"error": "unknown route"})

    def do_POST(self):
        eng: KokoroEngine = self.server.engine  # type: ignore[attr-defined]
        cache_dir: Path = self.server.cache_dir  # type: ignore[attr-defined]
        path = urlparse(self.path).path
        if path == "/unload":
            try:
                eng.unload()
                self._send(200, {"ok": True})
            except Exception as e:
                self._send(500, {"error": str(e)})
            return
        if path != "/synthesize":
            self._send(404, {"error": "unknown route"})
            return
        req = self._read_json()
        request_id = str(req.get("request_id") or "r")
        want_model = str(req.get("model") or "")
        if want_model and want_model != eng.model_id:
            self._send(400, {"error": (
                f"worker serves '{eng.model_id}', got '{want_model}'; "
                f"restart as: book-tts --model {want_model}"
            )})
            return
        voice = str(req.get("voice") or "kokoro-default")
        text = str(req.get("text") or "")
        if not text.strip():
            self._send(400, {"error": "empty text"})
            return
        try:
            kind_path = eng.resolve_voice(voice)
        except ValueError as e:
            self._send(400, {"error": str(e)})
            return
        kind, vpath = kind_path
        # Cache voice must match the Rust key (worker_cache_candidates):
        # kokoro stems ("kokoro-default" -> "af_heart"), clone ids verbatim.
        key_voice = vpath.stem if kind == "kokoro" else voice
        key = cache_key(eng.model_id, key_voice, " ".join(text.split()))
        opus_path = cache_dir / f"{key}.opus"
        wav_path = cache_dir / f"{key}.wav"
        try:
            cache_dir.mkdir(parents=True, exist_ok=True)
            if opus_path.is_file():
                self._send(200, {
                    "request_id": request_id,
                    "audio_path": str(opus_path),
                    "duration_secs": probe_duration(opus_path),
                    "cached": True,
                })
                return
            if wav_path.is_file():
                self._send(200, {
                    "request_id": request_id,
                    "audio_path": str(wav_path),
                    "duration_secs": probe_duration(wav_path),
                    "cached": True,
                })
                return
            pcm, dur = eng.synthesize(text, kind_path)
            tmp = wav_path.with_suffix(".tmp.wav")
            write_wav(tmp, pcm)
            tmp.rename(wav_path)
            if opus_encode(wav_path, opus_path):
                try:
                    wav_path.unlink()
                except OSError:
                    pass
                self._send(200, {
                    "request_id": request_id,
                    "audio_path": str(opus_path),
                    "duration_secs": dur,
                    "cached": False,
                })
            else:
                self._send(200, {
                    "request_id": request_id,
                    "audio_path": str(wav_path),
                    "duration_secs": dur,
                    "cached": False,
                })
        except Exception as e:
            log(f"synthesize failed: {e}")
            self._send(500, {"error": f"synthesis failed: {e}"})


def default_voices_dir() -> Path:
    xdg = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(xdg) / "orpheus" / "voices"


def main() -> None:
    ap = argparse.ArgumentParser(description="Orpheus TTS worker (book-tts).")
    ap.add_argument("--model", default="kokoro")
    ap.add_argument("--models-dir", default=str(default_models_dir()))
    ap.add_argument("--voices-dir", default=str(default_voices_dir()),
                    help="normalized clone references (<id>.wav)")
    ap.add_argument("--ov-model", default="openvoice-v2",
                    help="model id holding the OpenVoice converter (lazy)")
    ap.add_argument("--cache-dir", default=str(default_cache_dir()))
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--device", default="auto", help="auto | cuda | cpu")
    ap.add_argument("--preload", action="store_true", help="load model at startup")
    args = ap.parse_args()

    model_dir = Path(args.models_dir) / args.model
    if not (model_dir / MANIFEST_FILE).is_file():
        raise SystemExit(
            f"no manifest at {model_dir / MANIFEST_FILE}\n"
            f"pull first: python download.py --id {args.model} "
            f"--repo <org/name> --backend kokoro"
        )
    engine = KokoroEngine(
        args.model,
        model_dir,
        args.device,
        Path(args.voices_dir),
        Path(args.models_dir) / args.ov_model,
    )
    if args.preload:
        engine.ensure_loaded()
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.engine = engine  # type: ignore[attr-defined]
    server.cache_dir = Path(args.cache_dir)  # type: ignore[attr-defined]
    log(f"serving {args.model} on {args.host}:{args.port} "
        f"(models {args.models_dir}, cache {args.cache_dir})")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
