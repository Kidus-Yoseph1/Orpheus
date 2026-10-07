# tts-worker (`book-tts`)

Python side of Orpheus TTS. The Rust reader never runs model code — it only
speaks this worker's localhost HTTP API:

| Endpoint | Purpose |
|---|---|
| `GET /health` | `{ok, model, loaded, device, revision}` |
| `GET /voices` | built-in stems + `<voices-dir>/*.wav` clones |
| `POST /synthesize` | `{request_id, model, voice, text, speed}` → `{request_id, audio_path, duration_secs, cached}` |
| `POST /unload` | drop models, free VRAM (model switch) |

Synthesis always renders at 1x; playback speed is the player's job
(ffplay `atempo`), so the Opus cache (`~/.local/share/orpheus/cache/audio/`,
keyed by model+voice+text) stays speed-independent.

One process serves one model (`--model` picks the engine family):

| Model | Engine | VRAM | Cold load | Notes |
|---|---|---|---|---|
| `kokoro` | `KokoroEngine` | ~1.4 GB | ~5 s | built-in voices + OpenVoice conversion for clones (~1.7 GB) |
| `chatterbox-turbo` | `ChatterboxTurboEngine` | ~2.8 GB | ~32 s | native zero-shot cloning, reference must be ≥6 s, no preset voices |

- **OpenVoice V2 conversion** (`models/openvoice-v2/`, lazy): Kokoro renders
  a neutral base, converter swaps timbre to a clone reference
  (`~/.local/share/orpheus/voices/<id>.wav`, embedding cached as
  `<id>.se.pt`). Watermarking is stubbed out — your audiobooks stay clean.
- Both engines load with `from_local` / explicit paths: the HuggingFace
  cache is never used, weights come from the app-owned store.
- Chatterbox and Kokoro cannot be resident simultaneously on a 4 GB card;
  the Rust reader supervises this process (see `[tts] manage_worker`).

The manifest schema must stay identical to
`crates/orpheus-tts/src/store.rs::ModelManifest`.

## Setup

```sh
./install.sh [conda-env-name]   # reuses a torch+CUDA env via isolated .venv312
```

Needs system `libespeak-ng` (phonemization) and `ffmpeg` (opus + `ffplay`).

## Pull models (explicit — nothing downloads silently)

```sh
python3 download.py --id kokoro --repo hexgrad/Kokoro-82M --backend kokoro
python3 download.py --id openvoice-v2 --repo myshell-ai/OpenVoiceV2 --backend openvoice
```

Options: `--dry-run` to preview, `--rev <commit>` to pin (resolved commit
is recorded in the manifest), `--models-dir`, `--offline`.

## Serve

```sh
.venv312/bin/python server.py --model kokoro --preload   # :8765
server.py --help   # --port, --device auto|cuda|cpu, --voices-dir, --ov-model
```
