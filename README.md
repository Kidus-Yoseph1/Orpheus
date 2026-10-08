# Orpheus

A beautiful private audiobook reader that happens to live in the terminal.

Read EPUB books in a calm, Foliate-style page and listen to them with local
text-to-speech. Terminal-first, keyboard-first, fully offline after models
are pulled.

> Status: reader + library + themes + narration all work. Two engines are
> wired: **Kokoro-82M** (fast, CPU/GPU) and **Chatterbox Turbo** (native
> voice cloning, ~2.8 GB VRAM). Narration runs through a local `book-tts`
> worker that Orpheus starts and stops for you, with cloned narrator
> voices. `mock` model keeps a highlight-only timer for UI exercise.

## Features

- Library: open an `.epub` or a text-layer `.pdf`, scan a directory,
  continue where you left off
- PDF loading: wrapped lines rejoin into paragraphs, standalone page
  numbers drop out, short title lines become headings, and pages group into
  `Pages 1–10` style chapters; image-only (scanned) PDFs fail fast with a
  clear `needs OCR` message instead of a blank book
- Foliate-style reader: centered column, flowing paragraphs, inline
  narration highlight, chapter headings, quotes, lists
- 6 themes + 6 reader styles with live preview (`t`)
- SQLite persistence: books, resume positions, bookmarks
- Search inside a book (`/`)
- Model-agnostic TTS abstraction: switch lightweight (background work) and
  heavy (focused listening) models without losing your place; type any model
  id to queue it for pulling
- Voice cloning: register a 6–30 s clean sample once, then narrate in that
  voice with either engine (clips are model-agnostic, not locked to one model)
- One worker at a time: switching models frees VRAM, spawns the new worker
  and keeps your book position
- Audiobook export: render the whole book to one audio file — press `x` in
  the reader, or run `orpheus book.epub --export` headless

## Install

Requirements: Rust 1.75+ (async traits), a terminal with truecolor for best
results, ffmpeg (provides `ffplay` for audio) for narration. Voice
narration additionally needs the Python worker (`tts-worker/`, needs a
conda env with torch — see “TTS: how it works” below).

```sh
git clone git@github.com:Kidus-Yoseph1/Orpheus.git
cd Orpheus
cargo build --release
# binary is at ./target/release/orpheus
# optional: install to ~/.cargo/bin (on PATH, so `orpheus` works anywhere)
cargo install --path crates/orpheus
```

Run without installing:

```sh
cargo run -p orpheus
```

## Shell commands

All commands start with `orpheus`. `PATH` is optional: a `.epub` file or a
directory. Omit it to open the library home.

```sh
orpheus                          # library home (continue reading + recent)
orpheus ~/Books                   # scan a directory for .epub / .pdf
orpheus ~/Books/dune.epub         # open a book directly

orpheus --theme paper             # start with a theme
orpheus --style minimal           # start with a reader style
orpheus --model kokoro            # start with a TTS model
orpheus --voice kokoro-default    # start with a voice
orpheus --model chatterbox-turbo  # switch to native-cloning engine
orpheus --worker-check            # spawn worker, synthesize a test line, exit
orpheus --export                  # render the whole book to one audio file
orpheus "book.epub" --theme sepia --model chatterbox-turbo

orpheus voices                    # open the voice manager
orpheus models                    # open the model manager
orpheus config                    # print config path + current settings
orpheus --help                    # full CLI help
```

Themes: `midnight`, `paper`, `sepia`, `dark`, `forest`, `minimal`.
Styles: `classic`, `paper`, `sepia`, `midnight`, `focus`, `minimal`.

## Keybindings

`Ctrl+C` quits from anywhere. `Esc` goes back. Position auto-saves.

### Reader

| Key | Action |
| --- | ------ |
| `Space` | play / pause narration |
| `←` / `→` | seek back / forward (Shift: prev / next sentence) |
| `j` / `k`, `↑` / `↓` | scroll (`Ctrl` + key: 10 lines) |
| `n` / `p` | next / previous chapter |
| `-` / `+`, `0` | speed down / up / reset |
| `/` | search in book |
| `b` | bookmark current sentence |
| `t` | appearance: themes + reader styles (live preview) |
| `v` | voices (enter select · a add sample · p preview · d delete) |
| `m` | TTS models |
| `x` | export the whole book to one audio file (press again: cancel) |
| `g` / `G` | first / last chapter |
| `?` | help |
| `q` | quit |

### Library home

| Key | Action |
| --- | ------ |
| `↑` / `↓`, `j` / `k` | move selection |
| `Enter` | open selected book |
| `o` | open books folder (`~/Books` or current dir) |
| `d` | forget book (removes from library, keeps the file) |
| `t` | appearance |
| `m` / `v` | models / voices |
| `?` | help |
| `q` | quit |

### Shelf (directory)

| Key | Action |
| --- | ------ |
| `↑` / `↓`, `j` / `k` | move selection |
| `Enter` | open selected book |
| `r` | rescan directory |
| `Esc` | back |
| `?` | help |
| `q` | quit |

### Models

| Key | Action |
| --- | ------ |
| `↑` / `↓`, `j` / `k` | move selection |
| `Enter` | select model (book position is kept) |
| `/` or `i` | type a model id to pull on the fly, `Enter` confirms, `Esc` cancels |
| `Esc` / `q` | back |

### Appearance

| Key | Action |
| --- | ------ |
| `Tab` | switch between Themes and Reader styles |
| `↑` / `↓`, `j` / `k` | move (applies instantly for live preview) |
| `Enter` / `Space` | apply + back |
| `Esc` / `q` / `t` | back |

### Search

Type the query, `Enter` jumps to the first hit, `Esc` cancels.

## Configuration

Config file (TOML, created on first quit):

```sh
orpheus config          # shows the path + current values
# typically ~/.config/orpheus/config.toml
```

```toml
[reader]
theme = "midnight"     # midnight | paper | sepia | dark | forest | minimal
style = "paper"        # classic | paper | sepia | midnight | focus | minimal
margin = 6
line_spacing = 1

[playback]
speed = 1.0
seek_seconds = 10
buffer_seconds = 90

[tts]
model = "kokoro"
voice = "kokoro-default"   # kokoro-default (= af_heart) or af_bella, am_adam, ...
manage_worker = true      # Orpheus spawns/kills book-tts per model (4 GB cards)
worker_script = ""        # path to tts-worker/server.py ("" = autodetect)
worker_python = ""        # interpreter for it ("" = repo venv, then python3)
device = "auto"            # auto | cpu | cuda (worker-side, Kokoro only for now)
worker_url = "http://127.0.0.1:8765"

[tts.gpu]
enabled = true
max_vram_mb = 2500

[library]
directories = ["~/Books"]
recursive = true

[cache]
enabled = true
max_size_gb = 10

[pronunciation]        # word -> spoken form, applied before synthesis
"Cthulhu" = "kuh-THOO-loo"
```

Custom themes go in `<config-dir>/orpheus/themes/*.toml`:

```toml
name = "my-theme"
background = "#1e1e2e"
foreground = "#cdd6f4"
muted = "#6c7086"
heading = "#cba6f7"
accent = "#89b4fa"
current_sentence = "#f9e2af"
selection = "#313244"
progress = "#89b4fa"
border = "#45475a"
status = "#6c7086"
```

Library database: `~/.local/share/orpheus/library.db` (books, positions,
bookmarks, voices, TTS model registry).

## TTS: how it works

The reader only talks to the `TTSBackend` trait (`crates/orpheus-tts`):
`load_model / unload_model / list_voices / synthesize / health`. It never
branches on model names, so backends are swappable.

| Model | Class | Notes |
| ----- | ----- | ----- |
| `kokoro` | light | Kokoro-82M, local worker, CPU/GPU, ~1.4 GB VRAM, ~1.4s/sentence |
| `chatterbox-turbo` | heavy | native zero-shot cloning, ~2.8 GB VRAM (measured), 32s cold load |
| `chatterbox` | heavy | not wired yet (same engine, slower) |
| any custom id | medium | type it in Models (`/`) — registered immediately |
| `mock` | — | highlight-only timer, no audio (UI exercise) |

**Cloning, two ways:**
- `kokoro` + a clone = Kokoro renders, OpenVoice V2 converter swaps the timbre
  (≈1.7 GB VRAM total, latency ~1.4s/sentence)
- `chatterbox-turbo` clones natively from the reference clip — deeper speaker
  match, no conversion stage, but heavier and needs a ≥6s sample

**One model at a time.** Two models don't fit in 4 GB of VRAM, so Orpheus
supervises the worker: switching models kills the old worker (freeing VRAM)
and spawns one for the new model, polling until it answers `⏳` then plays.
The worker is located by walking up from the current directory (and from the
binary, so `cargo run` works). Launching the *installed* binary from somewhere
outside the checkout? Set `worker_script` / `worker_python` in `[tts]`, or
`ORPHEUS_WORKER_SCRIPT`. Logs: `~/.local/share/orpheus/logs/worker-<model>.log`.

Set `[tts] manage_worker = false` if you'd rather run the worker yourself
(`tts-worker/.venv312/bin/python tts-worker/server.py --model kokoro --preload`).

### 1. Pull the models (explicit, once)

```sh
cd tts-worker
python3 download.py --id kokoro --repo hexgrad/Kokoro-82M --backend kokoro
python3 download.py --id openvoice-v2 --repo myshell-ai/OpenVoiceV2 --backend openvoice
# optional, native cloning instead of conversion (~2.8 GB VRAM, 4 GB download)
python3 download.py --id chatterbox-turbo --repo ResembleAI/chatterbox-turbo --backend chatterbox-turbo
# --dry-run to preview, --rev <commit> to pin, --help for all flags
```

Files land in `~/.local/share/orpheus/models/<id>/` with a manifest;
nothing hides in the HuggingFace cache. Kokoro is ~360 MB, OpenVoice ~130 MB.

### 2. Set up the worker env (once)

Reuse a conda env that already has torch+CUDA through an isolated venv —
no giant torch re-download, base env untouched (needs system
`libespeak-ng` for phonemization and `ffmpeg` for opus + `ffplay`):

```sh
./tts-worker/install.sh [conda-env-name]   # default env: ml_base
```

### 3. Listen (the worker starts itself)

```sh
cargo run -p orpheus -- "mybook.epub"     # Space plays real audio
orpheus --worker-check                    # or verify the whole chain headlessly
```

With `[tts] manage_worker = true` (the default) Orpheus spawns `book-tts` for
the selected model on the first tick, waits for `⏳ starting …` to clear, and
kills it again when you quit — so VRAM is free when you are not listening.
Prefer to run it yourself? Set `manage_worker = false`:

```sh
tts-worker/.venv312/bin/python tts-worker/server.py --model kokoro --preload   # :8765, --help for flags
```

Playback: sentences synthesize to `~/.local/share/orpheus/cache/audio/`
(Opus, keyed by model+voice+text so repeats are free), 3-ahead prefetch
in background, speed via `ffplay atempo` without re-rendering. Seeks and
chapter jumps restart instantly; synthesis happens off the key path so the
UI never freezes — first play shows `▶ buffering…` while Kokoro loads.

### 4. Clone a narrator

**With kokoro** (default): Kokoro renders, OpenVoice converts timbre (~1.7 GB
total — measured). Get a 10–30s clean single-speaker clip (LibriVox
volunteers are ideal: public domain books *and* voices):

```sh
ffmpeg -ss 90 -t 20 -i chapter03.mp3 -ar 24000 -ac 1 ~/voices/narrator.wav
```

Or **with chatterbox-turbo** (`m` → select it): cloning is native, no
conversion step, but it wants a ≥6s sample (10–30s ideal) and costs ~2.8 GB
VRAM with a 32s cold load.

**Where samples go.** Drop clips in `~/voices/` (the add form suggests the
first file it finds there), or type any absolute path — wav/mp3/flac/m4a all
work. In Orpheus: `v` → `a` → name it → point at the file → `Enter`. The clip
is normalized (silence trimmed, gain to −20 dB, limiter) into
`~/.local/share/orpheus/voices/<name>.wav` — the original is untouched — and
registered in the database.

**A clip is model-agnostic**: add it once and it is available to *both*
engines (kokoro clones it through OpenVoice, chatterbox clones natively).
Length rules are enforced per model: ≥6 s for chatterbox (10–30 s is the
sweet spot), ≥3 s otherwise, ≤300 s. Too short and the form tells you; cut a
slice with `ffmpeg -ss 90 -t 25 -i in.mp3 -ar 24000 -ac 1 ~/voices/x.wav`.

Then `p` previews in the current model, `Enter` selects it (`Space` narrates),
`d` deletes the clone everywhere. Switching models keeps the selection only
if the clip still meets the new model's minimum — otherwise Orpheus picks the
first usable one and says so in the status line. The OpenVoice converter
loads lazily on first clone use, so kokoro-only sessions stay at ~1.4 GB.

## Export an audiobook

Turn the open book into a single audio file and listen anywhere (car, gym,
phone player). The text is rendered sentence by sentence with the current
model and voice, then joined with ffmpeg into one file.

**In the reader:** press `x`. The status line shows the phases —
`waiting for the worker…` (cold start, up to ~3 min for chatterbox),
`rendering 40% (120/300) - x cancels`, `joining 300 clips…`, then
`✓ exported → …`. Press `x` again at any point to cancel: nothing is lost,
every sentence already rendered stays in the audio cache.

**Headless (no TUI, scriptable):**

```sh
orpheus ~/Books/dune.epub --export                  # current config voice/model
orpheus ~/Books/dune.epub --export --model kokoro --voice clarke
OUT=$(orpheus dune.epub --export)   # path on stdout, progress on stderr
```

It exits 0 and prints the path on success, non-zero with the reason on
stderr on failure (worker never ready, synthesis error, ffmpeg missing).

**Where the file goes:** `~/.local/share/orpheus/exports/<book>-<voice>-<model>.m4a`
(AAC in an MP4 container, one file, phone-friendly). Path is also in the
reader's status line / the command's stdout.

**Cost:** sentences you already heard come from the audio cache and cost
nothing. A cold book is `#sentences` syntheses — patience with a 4 GB GPU
and chatterbox (they queue behind whatever else is on the device).

## Adding a new model

Two levels, depending on how different the model is from what is wired.

### A. Same engine family — no Rust changes

If the weights are a Kokoro-style repo (or another Chatterbox variant), pull
it with an explicit backend and it shows up in Models (`m`) immediately:

```sh
tts-worker/.venv312/bin/python tts-worker/download.py \
  --id my-kokoro --repo hexgrad/Kokoro-82M --backend kokoro
```

- `--id` is the model id everywhere (config, database, cache keys, worker arg)
- `--backend` is recorded in `orpheus-manifest.json` and must be a family the
  worker understands: `kokoro`, `openvoice`, `chatterbox`, `chatterbox-turbo`
- `--repo` / `--rev` pin exactly what landed in
  `~/.local/share/orpheus/models/<id>/`; `--dry-run` previews, `--offline`
  verifies without network
- Models are discovered from their manifest on start — no registration step

The worker picks its engine from the id: any id containing `chatterbox` runs
the Chatterbox engine, everything else runs Kokoro. Pick it with `m` →
`Enter`, or start with `orpheus --model my-kokoro` / `model = "..."` in
config. Verify before touching the UI:

```sh
orpheus --worker-check --model my-kokoro
```

### B. A genuinely new engine

1. **Pull the weights** — `download.py --id <id> --repo <org/name> --backend <family>`
   (any label is fine as long as step 2 understands it).
2. **Teach the worker** (`tts-worker/server.py`):
   - add an engine class with the same surface as `ChatterboxTurboEngine`:
     `resolve_voice(voice) -> (kind, path)`, `generate(text, ref)`,
     `ensure_loaded()`
   - branch on the id/backend in `build_engine(args, model_dir, ...)`
   - load from the app-owned store (`from_local` / explicit paths) — never the
     HuggingFace cache — and report real VRAM + cold-load in the tables above
3. **Teach the reader** (`crates/orpheus-tts/src/lib.rs`):
   - `BackendKind::MyEngine` plus `as_str`, `from_id`, `weight_class`
   - add it to `worker_supported()` in `crates/orpheus/src/app.rs`, otherwise
     the model lists as installed but `Space` refuses with "no voice yet"
4. **Verify**: `orpheus --worker-check --model <id>` (spawn → load → synthesize
   one line → exit), then `m` → select → `Space` on a real book.

Clone-only engines (no built-in voices) are detected from `BackendKind`: the
voices screen then lists only real samples and never a fake `<id>-default`.
Register a ≥6 s sample with `v` → `a` before expecting audio.

## Troubleshooting

Start with the headless check — it spawns the worker, loads the model,
synthesizes one line and exits, printing exactly where things break:

```sh
orpheus --worker-check --model chatterbox-turbo
```

| Symptom | Cause / fix |
| ------- | ----------- |
| `tts-worker/server.py not found` | launch cwd is outside the checkout; set `worker_script` / `worker_python` in `[tts]`, or `ORPHEUS_WORKER_SCRIPT` |
| `⏳ starting …` never clears | worker failed to load; see `~/.local/share/orpheus/logs/worker-<model>.log` |
| `CUDA out of memory` | two models resident at once; quit other GPU apps, keep `manage_worker = true` |
| `worker serves 'kokoro', got '…'` | a manually started worker is on the wrong model; let Orpheus manage it, or restart it with `--model` |
| `chatterbox has no built-in voices` | no usable clone: `v` → `a` with a ≥6 s sample |
| `ffplay not found` | install `ffmpeg` |
| PDF: `no selectable text … OCR` | scanned/image-only PDF — OCR is not implemented yet; export a text layer first (e.g. `ocrmypdf in.pdf out.pdf`) |
| silent / zero-length audio | inspect `logs/worker-<model>.log`; `ffprobe` on a cached `.opus` |
| model shows `pull on use` | not downloaded yet — run `download.py` (see above) |

## Development

Workspace crates:

- `crates/orpheus` — binary: CLI, TUI screens, key handling
- `crates/orpheus-core` — `Document → Chapter → Block → Sentence` model,
  EPUB loader, text normalization, themes, config, SQLite, scanner
- `crates/orpheus-tts` — `TTSBackend` trait, `TtsManager`, `MockBackend`,
  `WorkerBackend` (HTTP), `ModelStore` (app-owned `models/<id>/` + manifest)
- `tts-worker/` — `download.py` (explicit pulls), `server.py` (`book-tts`
  HTTP worker: `/health`, `/voices`, `/synthesize`, `/unload`)

```sh
cargo build              # debug build
cargo test               # segmentation, EPUB + stable IDs, DB resume,
                         # store manifests, Rust/Python cache-key parity,
                         # voice selection rules, worker discovery, ffplay stub
cargo fmt                # format
cargo clippy --all       # lint
```

Spec and milestones live in `GUIDE.md`.
