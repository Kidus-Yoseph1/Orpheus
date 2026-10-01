Yes. Given those V1 requirements, I would make this **a dedicated terminal-first audiobook/ebook reader**, not a Foliate fork.

 The design I would hand to an agent is below. I’ve also adjusted one earlier recommendation: **PyMuPDF is particularly useful here because it can handle both PDF and EPUB and exposes structured text blocks, while OCR can be used when PDFs are scanned.**  PyMuPDF+2  Chatterbox is also a good first TTS backend because its current family includes a smaller Turbo model and voice cloning, with expressive/paralinguistic controls.  GitHub+1

 # V1 Implementation Specification — Terminal Book Reader with Local Voice-Cloning TTS

 ## 1\. Product Vision

 Build a beautiful, terminal-native book reader for EPUB and PDF books with integrated local text-to-speech.

 The application should feel like a **real reading application**, not a developer utility.

 Primary experience:

 1. Launch the application.
2. See a beautiful library/home screen.
3. See recently read books and reading progress.
4. Open a directory and discover books inside it.
5. Select a book.
6. Read the book directly inside the terminal.
7. Start narration.
8. Follow the currently spoken text visually.
9. Pause, resume, skip backward/forward, and change playback speed.
10. Select different locally stored voices.
11. Add new voice samples for voice cloning.
12. Switch between different TTS models without changing the reader.
13. Resume any book exactly where it was left.

 The application must be completely usable without a GUI.

---

 # 2\. Core Technology Architecture

 Use a two-process architecture.

 ## Reader application

 Implement the main application in:

 **Rust**

 Recommended stack:

 - Rust
- Tokio
- Ratatui
- Crossterm
- SQLite
- Serde
- TOML
- tracing
- anyhow / thiserror

 The Rust application owns:

 - TUI
- library
- book navigation
- reading position
- text representation
- TTS queue
- audio playback control
- voice/model selection
- configuration
- cache
- keyboard shortcuts

 ## TTS worker

 Implement TTS backends separately, initially in:

 **Python + PyTorch**

 The TTS worker owns:

 - loading TTS models
- loading voice reference samples
- voice cloning
- speech generation
- model switching
- GPU/CPU selection
- audio generation

 Do NOT tightly couple the Rust application to one TTS model.

 Create a generic TTS interface.

 Example conceptual interface:

```
TTSBackend
    ├── name()
    ├── capabilities()
    ├── load_model()
    ├── unload_model()
    ├── list_voices()
    ├── synthesize()
    └── health()
```

 Initial backends:

```
Chatterbox
Chatterbox Turbo
Kokoro
Future models
```

 The reader should not know model-specific implementation details.

---

 # 3\. Process Communication

 The Rust reader and Python TTS worker communicate through a local IPC/API interface.

 Prefer a local Unix socket on Linux/macOS and a localhost TCP/HTTP endpoint where necessary for portability.

 Keep the API simple.

 Example:

```
{
  "request_id": "abc123",
  "model": "chatterbox-turbo",
  "voice": "my-narrator",
  "text": "The door slowly opened.",
  "speed": 1.0
}
```

 Response:

```
{
  "request_id": "abc123",
  "audio_path": "/cache/abc123.opus",
  "duration": 3.81
}
```

 The TTS worker should be independently testable.

 Running:

```
book-tts --model chatterbox-turbo
```

 should start the TTS service without the reader.

---

 # 4\. Document Architecture

 Normalize every supported book into a common internal representation.

 Do NOT let EPUB/PDF-specific details leak into the TTS system.

 Use:

```
Document
    └── Chapter[]
          └── Block[]
                └── Sentence[]
```

 Conceptually:

```
Document {
    id,
    title,
    author,
    source_path,
    format,
    chapters
}

Chapter {
    id,
    title,
    blocks
}

Block {
    id,
    text,
    sentences
}

Sentence {
    id,
    text
}
```

 Every sentence must have a stable ID.

 This is important for:

 - highlighting
- TTS
- caching
- resume position
- bookmarks
- seeking
- debugging

---

 # 5\. EPUB Support

 Support EPUB as a first-class format.

 Extract:

 - title
- author
- cover if available
- table of contents
- chapters
- paragraphs
- text
- reading order

 PyMuPDF is an option because it supports EPUB and exposes chapter/page locations, but it should be wrapped behind the application's document-loader interface.  PyMuPDF

 Do not assume every EPUB has clean HTML.

 Normalize:

 - HTML entities
- whitespace
- unnecessary line breaks
- page/chapter artifacts
- hidden elements
- repeated headers/footers
- malformed markup

 Preserve meaningful formatting such as:

 - headings
- emphasis
- italics
- block quotes
- lists

 The renderer can choose how much of this to display.

---

 # 6\. PDF Support

 PDF extraction needs its own processing layer.

 For normal PDFs:

 - extract text
- detect blocks
- reconstruct paragraphs
- detect headings
- remove repeated headers/footers
- detect columns where possible

 Do not blindly concatenate PDF text.

 PDF extraction can produce incorrect reading order and unexpected line breaks. PyMuPDF's structured `blocks`, `words`, and `dict` extraction modes should be used when layout information is needed.  PyMuPDF+1

 For scanned PDFs:

```
PDF page
   ↓
detect no usable text
   ↓
OCR
   ↓
normalized text
```

 PyMuPDF provides OCR integration using Tesseract.  PyMuPDF

 OCR should be optional because it is expensive.

---

 # 7\. Library / Home Screen

 When launching:

```
book
```

 show the home screen.

 Example:

```
╭────────────────────────────────────────────────────────────╮
│                         BOOKS                               │
│                                                            │
│  Continue Reading                                          │
│                                                            │
│  ┌──────────────────────────────────────────────────────┐  │
│  │ Dune                                                  │  │
│  │ Frank Herbert                                         │  │
│  │ Chapter 12 · 47%                                      │  │
│  │ ███████████████████░░░░░░░░░░░                       │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                            │
│  Recent                                                    │
│                                                            │
│  The Hobbit          81%                                   │
│  Foundation          32%                                   │
│  Neuromancer         14%                                   │
│                                                            │
│  [Enter] Open  [o] Open Directory  [a] Add Book  [q] Quit │
╰────────────────────────────────────────────────────────────╯
```

 Home screen requirements:

 - recently opened books
- currently reading book
- percentage progress
- chapter
- last-read location
- book title
- author
- optional cover
- search/filter
- open directory
- manually add/open book
- remove from library without deleting source file

---

 # 8\. Directory Mode

 Support:

```
book ~/Books
```

 The application should scan the directory recursively or non-recursively according to configuration.

 Display:

```
Books

> Dune.epub
  Foundation.epub
  The Hobbit.pdf
  Neuromancer.pdf
```

 Allow:

```
Enter       Open
Space       Preview/select
r           Refresh
/           Search
Esc         Back
```

 Optionally support:

```
book --dir ~/Books
```

---

 # 9\. Reading Interface

 This is one of the most important parts of the application.

 The reader should NOT look like a developer tool.

 Provide multiple visual themes/styles.

 At minimum:

 ### Classic

```
────────────────────────────────────────────

                    DUNE

The desert was vast and silent.

Paul looked toward the horizon.

The wind carried sand across the rocks.

────────────────────────────────────────────
Chapter 4                         37%
```

 ### Paper

 Warm background colors, serif-like terminal typography where supported, generous margins, minimal UI.

 ### Dark

 Dark background, soft gray text, warm white current sentence.

 ### Sepia

 Warm brown/cream palette resembling an old paperback.

 ### Focus

 Only the current paragraph and a small amount of surrounding context.

 ### Minimal

 Maximum text area, almost no UI.

 Themes must be configuration-driven.

 Do not hardcode colors throughout the application.

---

 # 10\. Reader Layout

 The reading screen should contain:

```
┌──────────────────────────────────────────────────────────────┐
│ DUNE                                      Chapter 12 · 47%    │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│  The wind moved across the open desert, carrying with it     │
│  the smell of dust and distant rain.                         │
│                                                              │
│  Paul stopped and looked toward the horizon.                 │
│                                                              │
│  "We should go."                                             │
│                                                              │
│  Jessica looked at him but said nothing.                     │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│ ▶  1.0x   Narrator: Sarah       Ch. 12       47%             │
│ Space Play   ←/→ Seek   +/- Speed   v Voices   q Quit        │
└──────────────────────────────────────────────────────────────┘
```

 The current spoken sentence should have a distinct style.

 Do not highlight the entire paragraph if sentence-level synchronization is available.

 Example:

```
Paul stopped and looked toward the horizon.
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
currently being spoken
```

---

 # 11\. TTS Architecture

 The TTS system must be model-agnostic.

 Define:

```
Model
Voice
TTSBackend
SynthesisRequest
SynthesisResult
```

 Example:

```
Model:
    id
    name
    backend
    language
    device
    capabilities

Voice:
    id
    name
    reference_audio
    model
    metadata
```

 The user should be able to switch models:

```
TTS Models

> Chatterbox
  Chatterbox Turbo
  Kokoro
```

 and voices:

```
Voices

> Narrator
  Calm Male
  Sarah
  Deep Voice
  My Voice
```

 Changing the TTS model must not change the book or reading position.

---

 # 12\. Voice Library

 Provide a dedicated voice management screen.

 Example:

```
VOICE LIBRARY

> Sarah
  My Narrator
  Deep Narrator
  Test Voice

[a] Add voice
[d] Delete
[r] Rename
[p] Preview
Enter Select
```

 Adding a voice:

```
Add Voice

Name:
> Sarah

Reference audio:
> ~/voices/sarah.wav

Model:
> Chatterbox

Language:
> English

[p] Preview
[s] Save
[Esc] Cancel
```

 Store metadata in SQLite.

 Store audio samples on disk.

 Do not duplicate large audio files unnecessarily.

---

 # 13\. Voice Reference Audio

 Accept:

 - WAV
- MP3
- FLAC
- M4A where supported

 Normalize reference audio before sending it to the model:

 - mono
- appropriate sample rate
- trim excessive silence
- normalize volume
- optionally remove long leading/trailing silence

 Do not permanently modify the user's original file.

 Create a normalized copy in application storage.

---

 # 14\. Model Switching

 Model switching must be explicit.

 Example:

```
┌───────────────────────────────┐
│ TTS MODEL                     │
│                               │
│ > Chatterbox                  │
│   Chatterbox Turbo            │
│   Kokoro                      │
│                               │
│ Device: Auto                  │
│                               │
│ [Enter] Select                │
└───────────────────────────────┘
```

 Models should be loaded lazily.

 Do not load every model into GPU memory.

 When switching:

```
unload current model
↓
release GPU memory
↓
load new model
↓
verify model
↓
continue playback
```

 Chatterbox currently provides a 350M-parameter Turbo model aimed at lower compute/VRAM usage, while the broader Chatterbox family supports zero-shot voice cloning; use the backend abstraction so these can be swapped without touching the reader.  GitHub+1

---

 # 15\. TTS Queue

 Never synthesize only the current sentence.

 Maintain a rolling queue.

 Example:

```
CURRENT
sentence 105     PLAYING

BUFFER
sentence 106     READY
sentence 107     READY
sentence 108     READY

QUEUE
sentence 109
sentence 110
sentence 111
```

 Target:

 **30–120 seconds of generated audio ahead of playback.**

 Make the buffer configurable.

 If the user jumps forward:

```
discard old queue
↓
locate target sentence
↓
generate target
↓
generate next sentences
```

 Never force the user to wait for the entire book to synthesize.

---

 # 16\. Audio Cache

 Cache generated speech.

 Recommended structure:

```
~/.local/share/bookreader/
    books/
    voices/
    models/
    cache/
```

 Cache key should include:

```
book_id
sentence_id
model_id
voice_id
speed
relevant synthesis parameters
```

 Example:

```
sha256(
    book_id +
    sentence_id +
    model +
    voice +
    speed +
    parameters
)
```

 Store generated audio as Opus where practical.

 Do not regenerate the same sentence unnecessarily.

---

 # 17\. Playback Controls

 Minimum controls:

```
Space       Play/Pause
Left        Back 10 seconds
Right       Forward 10 seconds
Shift+Left  Previous sentence
Shift+Right Next sentence
-           Slow down
+           Speed up
0           Reset speed
v           Voice selector
m           Model selector
```

 Speed presets:

```
0.75x
0.85x
0.90x
1.00x
1.10x
1.20x
1.30x
1.50x
1.75x
2.00x
```

 Use audio playback speed adjustment when appropriate rather than regenerating speech for every speed change.

---

 # 18\. Resume Position

 Persist:

```
book
chapter
paragraph
sentence
character offset if necessary
audio timestamp
```

 When closing:

```
save current position
```

 When reopening:

```
Continue reading from:
Chapter 12
Sentence 43
47%
```

 The home screen should show this information.

---

 # 19\. Book Database

 Use SQLite.

 Suggested tables:

```
books
chapters
positions
voices
tts_models
bookmarks
settings
```

 Books:

```
id
path
title
author
format
added_at
last_opened_at
```

 Positions:

```
book_id
chapter_id
block_id
sentence_id
progress
updated_at
```

 Voices:

```
id
name
model_id
reference_path
created_at
```

---

 # 20\. Navigation

 Provide Vim-style navigation.

 Default:

```
j / Down       scroll down
k / Up         scroll up
Ctrl+d         half page down
Ctrl+u         half page up
g              beginning
G              end
n              next chapter
p              previous chapter
/              search
b              bookmark
m              menu
q              quit
```

 Do not force Vim keys on users.

 Support arrow keys as well.

---

 # 21\. Search

 V1 should include basic search.

 Press:

```
/
```

 Then:

```
Search: paul
```

 Display matches:

```
Search results

> Chapter 3
  Chapter 7
  Chapter 12
  Chapter 17
```

 Enter should jump to the result.

---

 # 22\. Bookmarks

 Allow:

```
b
```

 to bookmark the current location.

 Bookmark data:

```
book
chapter
sentence
user note
created_at
```

 V1 can initially support bookmarks without notes.

---

 # 23\. Narration Synchronization

 This is important.

 The reader needs to know:

```
which sentence is currently playing
```

 TTS should return timing information when possible.

 Ideal:

```
Sentence
    ↓
audio
    ↓
word/sentence timing
```

 At minimum synchronize at sentence level.

 The UI should automatically scroll so the active sentence stays visible.

 Do not constantly move the viewport every word.

 Scroll when the active sentence leaves the comfortable reading region.

---

 # 24\. Natural Narration

 Do not simply pass raw PDF/EPUB text directly into TTS.

 Create a preprocessing pipeline:

```
raw document
     ↓
cleanup
     ↓
paragraph reconstruction
     ↓
sentence segmentation
     ↓
dialogue detection
     ↓
abbreviation handling
     ↓
pronunciation replacements
     ↓
TTS
```

 Handle:

 - dialogue
- quotations
- abbreviations
- headings
- footnotes
- page numbers
- chapter titles
- hyphenated line breaks
- URLs
- citations
- mathematical expressions where possible

 Avoid reading obvious PDF artifacts aloud.

---

 # 25\. Pronunciation Dictionary

 Support a user-editable pronunciation dictionary.

 Example:

```
[pronunciation]
"Cthulhu" = "..."
"Nguyen" = "..."
"Worcestershire" = "..."
```

 Allow users to add corrections while reading.

 This should happen before TTS synthesis.

---

 # 26\. Configuration

 Use TOML.

 Example:

```
[reader]
theme = "paper"
font_size = 1
line_spacing = 1
margin = 6

[playback]
speed = 1.0
seek_seconds = 10
buffer_seconds = 90

[tts]
model = "chatterbox-turbo"
device = "auto"

[library]
directories = [
    "~/Books"
]

[cache]
enabled = true
max_size_gb = 10
```

---

 # 27\. Theme System

 Themes are a major V1 feature.

 Do not implement only "dark" and "light."

 Create a theme abstraction.

 Example:

```
themes/
    dark.toml
    paper.toml
    sepia.toml
    midnight.toml
    forest.toml
    minimal.toml
```

 Theme properties:

```
background
foreground
muted
heading
accent
current_sentence
selection
progress
border
status
```

 Example:

```
name = "paper"

background = "#F4EED8"
foreground = "#302B24"
muted = "#847A68"
accent = "#8B5E34"
current_sentence = "#A34F24"
heading = "#5E4028"
```

 The terminal must gracefully fall back when true color is unavailable.

---

 # 28\. Reader Styles

 V1 should ship with several actual reading experiences, not just color palettes.

 ### Classic

 Traditional book layout.

 ### Paper

 Warm, spacious, book-like.

 ### Sepia

 Old paperback feel.

 ### Midnight

 Dark and elegant.

 ### Focus

 Only a few paragraphs around the current location.

 ### Minimal

 Text dominates the entire screen.

 The reader should be able to switch styles from:

```
m → Appearance → Reader Style
```

---

 # 29\. CLI Interface

 The application should also work well from the shell.

 Examples:

```
book
```

 Open home/library.

```
book ~/Books
```

 Open directory.

```
book ~/Books/dune.epub
```

 Open specific book.

```
book --theme paper
```

 Select theme.

```
book --voice narrator
```

 Select voice.

```
book --model chatterbox-turbo
```

 Select TTS model.

```
book voices
```

 Open voice manager.

```
book models
```

 Open model manager.

```
book config
```

 Open settings.

---

 # 30\. Keyboard-First UX

 Everything must be usable without a mouse.

 Mouse support can exist but must not be required.

 Every screen should have:

 - clear focus
- selected item
- keyboard shortcut hints
- Escape to go back
- q/Ctrl+C to exit safely

---

 # 31\. TUI Screen Architecture

 Implement separate screens/views:

```
HomeScreen
LibraryScreen
ReaderScreen
ChapterScreen
VoiceScreen
VoiceEditorScreen
ModelScreen
SettingsScreen
SearchScreen
BookmarksScreen
HelpScreen
```

 Use a central application state:

```
AppState
    current_screen
    current_book
    current_chapter
    current_sentence
    playback_state
    selected_voice
    selected_model
    theme
```

 Avoid putting application state directly inside individual UI rendering functions.

---

 # 32\. Audio Player

 Do not implement a full audio decoder/player in V1 unless necessary.

 Use a mature local audio player backend.

 Recommended approach:

```
Rust application
       ↓
mpv subprocess / IPC
       ↓
audio device
```

 This gives reliable:

 - play
- pause
- seek
- volume
- speed
- audio device selection

 Later, the player can be replaced with a native Rust audio backend if desired.

---

 # 33\. GPU Resource Management

 The user has a GTX 1650 4 GB.

 The TTS system must have:

```
device = auto
```

 Behavior:

```
if GPU available and within configured memory limit:
    use GPU

otherwise:
    use CPU
```

 Allow:

```
[tts.gpu]
enabled = true
max_vram_mb = 2500
```

 Do not assume the GPU is always available.

 The user may be gaming or running another GPU workload.

---

 # 34\. Model Manager

 Provide a model manager.

 Example:

```
TTS MODELS

Installed

> Chatterbox Turbo
  Chatterbox
  Kokoro

Available

  ...
```

 Actions:

```
Enter       Select
i           Install
d           Delete
r           Reload
p           Preview
```

 Model downloads should be explicit.

 Do not silently download multi-GB models.

 Show:

```
Downloading Chatterbox Turbo

████████████████░░░░░░░  72%

1.2 GB / 1.7 GB
```

---

 # 35\. V1 Non-Goals

 Do NOT build these in V1:

 - cloud synchronization
- mobile app
- web application
- multiplayer/shared libraries
- automatic character voice assignment
- full audiobook production editor
- advanced LLM-based emotional narration
- DRM circumvention
- complicated PDF visual rendering
- annotations system
- recommendation engine

 Keep V1 focused.

---

 # 36\. Recommended Development Order

 Implement in this exact order.

 ## Milestone 1 — TUI skeleton

 Build:

```
book
```

 with:

 - Ratatui
- theme system
- navigation
- screen architecture
- keyboard handling

 No TTS yet.

 ## Milestone 2 — EPUB

 Implement:

```
book file.epub
```

 and render the book.

 Add:

 - chapters
- scrolling
- progress
- resume

 ## Milestone 3 — SQLite library

 Add:

 - home screen
- recently read
- directory scanning
- persistence
- bookmarks

 ## Milestone 4 — PDF

 Add:

 - normal PDF extraction
- paragraph reconstruction
- scanned PDF detection
- OCR fallback

 ## Milestone 5 — TTS abstraction

 Implement the generic TTS API.

 Do NOT make Chatterbox part of the reader code.

 ## Milestone 6 — Chatterbox backend

 Implement:

```
ChatterboxBackend
```

 with:

 - model loading
- voice reference
- synthesis
- audio output
- errors
- CPU/GPU selection

 Chatterbox supports reference-audio voice cloning and is MIT licensed; its current family also includes Turbo for lower compute/VRAM use.  GitHub+1

 ## Milestone 7 — Audio queue

 Implement:

```
current
next
next
queued
```

 with caching.

 ## Milestone 8 — Playback

 Add:

---

 # 31\. TUI Screen Architecture

 Implement separate screens/views:

```
HomeScreen
LibraryScreen
ReaderScreen
ChapterScreen
VoiceScreen
VoiceEditorScreen
ModelScreen
SettingsScreen
SearchScreen
BookmarksScreen
HelpScreen
```

 Use a central application state:

```
AppState
    current_screen
    current_book
    current_chapter
    current_sentence
    playback_state
    selected_voice
    selected_model
    theme
```

 Avoid putting application state directly inside individual UI rendering functions.

---

 # 32\. Audio Player

 Do not implement a full audio decoder/player in V1 unless necessary.

 Use a mature local audio player backend.

 Recommended approach:

```
Rust application
       ↓
mpv subprocess / IPC
       ↓
audio device
```

 This gives reliable:

 - play
- pause
- seek
- volume
- speed
- audio device selection

 Later, the player can be replaced with a native Rust audio backend if desired.

---

 # 33\. GPU Resource Management

  The TTS system must have:

```
device = auto
```

```
if GPU available and within configured memory limit:
    use GPU

otherwise:
    use CPU
```

 Allow:

```
[tts.gpu]
enabled = true
max_vram_mb = 2500
```

 Do not assume the GPU is always available.

 The user may be gaming or running another GPU workload.

---

 # 34\. Model Manager

 Provide a model manager.

 Example:

```
TTS MODELS

Installed

> Chatterbox Turbo
  Chatterbox
  Kokoro

Available

  ...
```

 Actions:

```
Enter       Select
i           Install
d           Delete
r           Reload
p           Preview
```

 Model downloads should be explicit.

 Do not silently download multi-GB models.

 Show:

```
Downloading Chatterbox Turbo

████████████████░░░░░░░  72%

1.2 GB / 1.7 GB
```

---

 # 35\. V1 Non-Goals

 Do NOT build these in V1:

 - cloud synchronization
- mobile app
- web application
- multiplayer/shared libraries
- automatic character voice assignment
- full audiobook production editor
- advanced LLM-based emotional narration
- DRM circumvention
- complicated PDF visual rendering
- annotations system
- recommendation engine

---

 # 36\. Recommended Development Order

  ## Milestone 1 — TUI skeleton

```
book
```

 with:

 - Ratatui
- theme system
- navigation
- screen architecture
- keyboard handling

 - play
- pause
- seek
- speed
- previous/next sentence

 ## Milestone 9 — Voice library

 Add:

 - voice samples
- voice names
- previews
- voice switching

 ## Milestone 10 — Model manager

 Add:

 - model list
- installation
- selection
- unloading
- model switching

 ## Milestone 11 — Reader polish

 Implement:

 - Paper
- Sepia
- Dark
- Midnight
- Focus
- Minimal

 Then refine spacing, typography, borders, progress indicators, animations and status bars.

---

 # 37\. V1 Definition of Done

 V1 is complete when the following workflow works:

```
$ book ~/Books
        ↓
Library
        ↓
select Dune
        ↓
Reader
        ↓
select voice "Sarah"
        ↓
select model "Chatterbox Turbo"
        ↓
Play
        ↓
text scrolls automatically
        ↓
current sentence highlighted
        ↓
Space pauses
        ↓
Right seeks forward
        ↓
+ increases speed
        ↓
v switches voice
        ↓
m switches TTS model
        ↓
q exits
        ↓
launch book again
        ↓
Continue from exact previous position
```

 This workflow should work reliably before adding anything else.

---

 # 38\. Important Engineering Principle

 The architecture should have this boundary:

```
                 BOOK READER
                      │
       ┌──────────────┼───────────────┐
       │              │               │
    Document        Player          TTS API
       │                              │
       │                              │
 EPUB/PDF                         ┌───┴────┐
                                  │        │
                             Chatterbox  Kokoro
                                  │        │
                             Future models
```

 The reader should never contain:

```
if model == chatterbox:
    ...
elif model == kokoro:
    ...
```

 Instead:

```
tts.synthesize(request)
```

 The model-specific code belongs entirely behind the TTS backend interface.

 That decision will make the project much easier to evolve.

---

 # 39\. Product Philosophy

 The application should feel like:

 **"A beautiful private audiobook reader that happens to live in the terminal."**

 It should NOT feel like:

 **"A Python TTS demo with a terminal interface."**

 Prioritize:

 1. Reading experience
2. Narration quality
3. Fast interaction
4. Reliable resume
5. Beautiful themes
6. Easy voice switching
7. Model flexibility
8. Low resource usage

 The terminal is the medium, not the aesthetic.

---

 # 40\. First Implementation Target

 Do not attempt all of V1 immediately.

 The first working vertical slice should be:

```
EPUB
 ↓
Rust parser
 ↓
TUI reader
 ↓
sentence selection
 ↓
Chatterbox worker
 ↓
audio
 ↓
sentence highlighting
 ↓
pause/resume
```

 Once that works end-to-end, build outward.

 The application should be runnable with:

```
book mybook.epub
```

 and the first time the user presses Space, it should actually read the book aloud.

 ### One architectural change I'd strongly recommend

 I would **not make Chatterbox the center of the application**. Make the **TTS interface** the center.

 That's what gives you the "swappable models" feature you explicitly want. Chatterbox is a particularly sensible first backend because its current implementation supports zero-shot reference-audio cloning, and the Turbo model is designed to reduce compute/VRAM requirements.  GitHub+1

---

 For PDFs, also don't treat extracted text as automatically trustworthy: PDF text can come out in the wrong reading order or with unwanted line breaks, so the extraction/normalization layer deserves to be a real subsystem rather than a one-line `get_text()` call.  PyMuPDF+1

 ### The one thing I'd tell your coding agent explicitly

 **Do not build V1 as a giant monolithic Rust application.**

 Use:

```
book                 ← Rust TUI/application
book-tts             ← Python TTS service
                     ↑
              stable local API
```

 That gives you a beautiful, lightweight Rust application while letting you experiment aggressively with Chatterbox, Chatterbox Turbo, Kokoro, and future models without rewriting the reader.

 And because you specifically want **"reader styles"**, I would make the theme/reader rendering system a first-class subsystem from day one—not something bolted on after the functionality works.
