# Orpheus

A beautiful private audiobook reader that happens to live in the terminal.

Read EPUB books in a calm, Foliate-style page and listen to them with local
text-to-speech. Terminal-first, keyboard-first, fully offline after models
are pulled.

> Status: reader + library + themes work today. TTS currently runs on a
> `MockBackend` (highlight advances, no audio yet) behind a model-agnostic
> trait — real Kokoro / Chatterbox synthesis via a Python worker is next.

## Features

- EPUB library: open a file or scan a directory, continue where you left off
- Foliate-style reader: centered column, flowing paragraphs, inline
  narration highlight, chapter headings, quotes, lists
- 6 themes + 6 reader styles with live preview (`t`)
- SQLite persistence: books, resume positions, bookmarks
- Search inside a book (`/`)
- Model-agnostic TTS abstraction: switch lightweight (background work) and
  heavy (focused listening) models without losing your place; type any model
  id to queue it for pulling

## Install

Requirements: Rust 1.75+ (async traits), a terminal with truecolor for best
results.

```sh
git clone <this-repo>
cd Orpheus
cargo build --release
# binary is at ./target/release/orpheus
# optional: install to ~/.cargo/bin
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
| `v` | voices |
| `m` | TTS models |
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
voice = "kokoro-default"
device = "auto"        # auto | cpu | cuda

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
bookmarks, voices).

## TTS: current state and next step

The reader only talks to the `TTSBackend` trait (`crates/orpheus-tts`):
`load_model / unload_model / list_voices / synthesize / health`. It never
branches on model names, so backends are swappable.

| Model | Class | Notes |
| ----- | ----- | ----- |
| `kokoro` | light | good while multitasking, CPU-friendly |
| `chatterbox-turbo` | medium | voice cloning, efficient |
| `chatterbox` | heavy | voice cloning, best for focused listening |
| any custom id | medium | type it in Models (`/`) — registered immediately, worker pulls on first synthesis |

Today `MockBackend` is wired: pressing `Space` advances the highlight on a
timer so queue/progress UI can be exercised, but no audio is produced. The
next milestone adds the Python worker (`tts-worker/`, Unix-socket API),
real synthesis, the Opus audio cache, and `mpv`-based playback — without
changing any reader code.

## Development

Workspace crates:

- `crates/orpheus` — binary: CLI, TUI screens, key handling
- `crates/orpheus-core` — `Document → Chapter → Block → Sentence` model,
  EPUB loader, text normalization, themes, config, SQLite, scanner
- `crates/orpheus-tts` — `TTSBackend` trait, `TtsManager`, `MockBackend`

```sh
cargo build              # debug build
cargo test               # 10 tests: segmentation, EPUB + stable IDs, DB resume, TTS
cargo fmt                # format
cargo clippy --all       # lint
```

Spec and milestones live in `GUIDE.md`.
