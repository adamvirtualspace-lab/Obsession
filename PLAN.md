# Obsession – Plan

A markdown notes app for game and animation production, written in Rust.
Your notes are plain `.md` files. A folder can have a node graph (`_nodes.md`)
where files are cards and the wires between them can run small Blueprint-style
functions that read and change those files.

The file syntax lives in [Syntax.md](Syntax.md). This file is about how the app
is built and in what order.

## Goals

- **Plain markdown first.** Every file stays readable and editable in Obsidian,
  VS Code, GitHub and git. Nothing important is hidden in a database.
- **Never break a file.** The app only rewrites the exact characters it means
  to change. Anything it doesn't understand is left byte for byte.
- **Power users welcome.** Full markdown plus Obsidian's extras, and links that
  resolve exactly the way Obsidian resolves them.
- **Logic you can see.** No hidden rules in task files. Every automatic change
  comes from a function on a wire in a graph.
- **Light, fast and good-looking.** One native binary, a small memory
  footprint, smooth animations at the display's frame rate (60 fps by default,
  up to 120 fps where the display supports it), careful with CPU and disk, and
  nothing running when idle. Budgets are in [Performance](#performance).
- **Simple to build.** Two crates, few dependencies, no migrations, no mixed
  languages.

## Not in version 1

- Real-time collaboration (teams share through git or a synced folder).
- Plugins.
- Embeds (`![[file]]`), math, Mermaid diagrams.
- Functions calling other saved functions.
- Mac and Linux builds (iced supports them, but Windows comes first).

## Stack

| Need | Choice | Why |
|---|---|---|
| UI | **iced 0.14** | Native, GPU-drawn, Elm-style state, built-in animation helpers, `Canvas` with dashed strokes, `text_editor` |
| Editor highlighting | **Our own markdown tokenizer** (`core/src/syntax.rs`) | iced's `highlighter` feature loads a grammar database for hundreds of languages: about 30 MB of RAM just to colour markdown. Ours knows tasks, `@people`, `^ids` and links |
| Markdown rendering | **Our own Live Preview renderer** (`core/src/live.rs` + `app/src/live.rs`), used by Live, Split and Reading | iced's `markdown` widget was the first preview; it's gone (and its feature off) so all views draw a note the same way. Footnotes need our own pass later. Code blocks aren't syntax-coloured, to stay off the grammar database |
| Task, link and status parsing | **Our own line parser** | The syntax is line-based. Owning it lets the app patch single lines safely |
| Frontmatter | A read-only YAML reader (`yaml-rust2` or `saphyr`) | The app reads frontmatter but never rewrites it |
| File watching | **notify** with its debouncer | Picks up edits from Obsidian, VS Code and `git pull` |
| Folder picker | **rfd** | Native Windows dialog |
| Graphics backend | **DirectX 12 only, low-power GPU** on Windows | iced asks wgpu for every backend by default, which loads the Vulkan *and* DX12 drivers: about 100 MB extra. `WGPU_BACKEND` / `WGPU_POWER_PREF` still override it |
| Errors | `thiserror` in core, `anyhow` in the app | Standard and small |
| Async | iced's default thread-pool executor, **no tokio** | Fewer threads, smaller binary |
| Profiling | `tracing` + Tracy, behind a `profile` feature | Zero cost in normal builds |

Every dependency is added with `default-features = false` and only the
features we use. iced gets only what we draw with. Milestone 1 uses `wgpu`,
`thread-pool`, `markdown`, `advanced`, `crisp` and `web-colors`. Later
milestones add `canvas`, `image` and `lazy` as they need them.

## Layout

```
obsession/
  Cargo.toml            workspace
  bench/                package "obsession-bench": test vault generator + timings
  core/                 package "obsession-core": no UI, fully testable
    src/
      vault.rs          scan folders, name/path lookups, Obsidian link resolution
      links.rs          links in a note, outlines, the link index, rename rewrites
      store.rs          file store: files in RAM, pinning, budget, disk counters
      syntax.rs         line tokens for editor highlighting
      preview.rs        Obsidian syntax -> plain markdown (unused by the app now; kept for export)
      fuzzy.rs          quick switcher matching
      status.rs         tasks (task_box), status formats, next status
      doc.rs            a note as lines, each with its own line ending (byte-exact)
      history.rs        undo/redo: one snapshot per group of edits, capped at 8 MB
      live.rs           Live Preview model: blocks, drawn lines, click-to-raw mapping
      nodes.rs          _nodes.md: node list, wires, layout, wire functions
      func/
        mod.rs          obsession code blocks (the text format), Library
        types.rs        Text, List, Bool, Number, Task; what an argument accepts
        library.rs      every node in the function library
        check.rs        type checking: decides dashed vs solid
        eval.rs         runs functions in memory: live values, patches
        edit.rs         editor changes as one-line text edits
      crew.rs           _crew/*.md, mention counts, badges
      runner.rs         function index, runs with chains and loop guard, apply/revert
      log.rs            run log in .obsessed/log/ (one file per run)
      write.rs          atomic writes, retry on locked files
  app/                  package "obsession": the iced app
    src/
      main.rs           app state, messages, shortcuts, saving
      theme.rs          colors, spacing, animation timings
      sidebar.rs        file tree (virtual list)
      switcher.rs       quick switcher (Ctrl+P)
      note.rs           one note: Live or Source body, Split's mirror, autosave timers
      highlight.rs      iced highlighter on top of core's syntax.rs, status colours
      status_menu.rs    right-click status menu
      right_click.rs    wrapper that reports where a right-click landed
      new_note.rs       new-note box (Ctrl+N), also the rename box (F2)
      backlinks.rs      backlinks panel
      live.rs           Live Preview: drawn blocks + the active block's editor
      source_scroll.rs  where the Source editor is scrolled to (for Split)
      live_text.rs      a drawn line: styled spans, click → character offset
      timer.rs          one-shot delays without an async runtime
      settings.rs       last opened vault (%APPDATA%\Obsession)
      dev.rs            debug-only hooks: open a note, screenshot, quit
      graph.rs          graph view: cards, pins, wires, pan, zoom, drag, hit testing
      func_editor.rs    function editor (canvas + panel) and the function picker
      runs.rs           file watcher, run state (1 run/s, one pending), Runs panel
      crew_hand.rs      the card hand
```

> The core package can't be named `core`, because that clashes with Rust's own
> `core` crate. Hence `obsession-core`.

## How the main pieces work

**Documents.** Each file is kept as its original text plus a line index. Every
change, whether from a click, a function or a rename, is a *patch*: a line
number, the old text and the new text. A patch only applies if the line still
holds the old text. This is what keeps foreign syntax safe and makes undo
reliable.

**Function runs.** `runner.rs` collects changed files. At most once per second it
starts a run off the UI thread:

1. Find every function with an input in a changed file.
2. Evaluate them in memory. If their patches change other files, keep going
   (a chain), but each target can change at most once per run (loop guard).
3. Write each changed file **once**, atomically.
4. Log the run to `.obsessed/` for undo.

If files change while a run is going, they merge into **one** pending run that
starts when the current one finishes.

**Writes.** Write to a temporary file in the same folder, then rename over the
original. On Windows, retry a few times if Obsidian or antivirus has the file
locked. Skip the write if the content is unchanged. The watcher ignores events
caused by the app's own writes (matched by content hash).

**Editor autosave.** Save after 1 second of no typing, and when switching files
or closing. Skip if unchanged.

**Graph drawing.** Everything in the graph is drawn on an iced `Canvas` with
cached geometry (node cards, wires, pins), and clicks are hit-tested by hand.
The function editor is the same canvas showing a different graph. Wires are
dashed until their function type-checks, then animate to solid.

## Performance

### Budgets

Checked at the end of every milestone, on a mid-range Windows laptop, using
the generated test vault (see Measuring).

| What | Budget |
|---|---|
| CPU when idle | **0%**: no frames, no timers, no polling |
| Disk when idle | **0 bytes** written |
| Disk reads during a warmed-up function run | **0**: everything comes from RAM |
| Writes per changed file per run | **1** |
| Frame time while animating or dragging | **≤ 8.3 ms**, p99. That's enough for 120 fps, which leaves room to spare at 60 fps |
| Graph with 500 nodes and 1,000 wires | No dropped frames while panning and zooming |
| Background work (indexing, checks) | Below-normal thread priority, at most 2 threads |
| Memory, empty vault | < 90 MB working set (was a 60 MB target, before measuring the driver baseline) |
| Memory, 10,000-file vault | Target < 120 MB working set |
| First window on screen | < 300 ms |
| Indexing 10,000 files | < 2 s, in the background, UI usable meanwhile |
| Release binary | < 20 MB |

> On Windows, any GPU-drawn app pays a baseline for the graphics driver
> before drawing anything. Milestone 1 measured ours (see below), and the
> empty-vault budget was adjusted to it once. From here on budgets only go
> down.

### Milestone 1 measurements

Release build, AMD Radeon 760M laptop (integrated GPU), 2880×1800 at 225%,
2026-11-14.

| What | Budget | Measured |
|---|---|---|
| CPU when idle | 0% | **0 ms** of CPU over 15 s |
| Disk when idle | 0 bytes | **0 bytes** read or written over 15 s |
| Memory, small vault, no note open | < 90 MB | **83–85 MB** |
| Memory, note open (edit / preview / split) | | **97 / 93 / 106 MB** |
| Memory, 10,000-note vault | < 120 MB | **85 MB**, 93 MB with a note and the switcher open |
| Scanning 10,000 notes | < 2 s | **28 ms** (tree index ≈ 420 KB) |
| Reading 10,000 notes through the store | | 2 s cold, then **3 ms with 0 disk reads** |
| First window on screen | < 300 ms | **47 ms** warm, 650 ms on a cold first start |
| Release binary | < 20 MB | **7.7 MB** |
| Opening a note changes the file | never | Verified: checksums unchanged |
| Saving preserves the file | byte-for-byte | Verified: only typed text differs, CRLF kept |

Where the memory goes:

- **About 84 MB is fixed**: the DirectX 12 driver plus iced's renderer setup.
  It doesn't change with window size (tested 400×300 to 1600×1000) or vault
  size.
- Before the fixes it was 198 MB. Defaulting to DX12 alone saved about 100 MB,
  and replacing iced's highlighter saved another 25 MB per open note.
- The cold start (650 ms) is mostly the driver compiling shaders the first
  time. Windows caches them after that.

### Milestone 2 measurements

Same machine, 2026-11-14.

| What | Budget | Measured |
|---|---|---|
| CPU when idle, status note open in split view | 0% | **0 ms** over 10 s |
| Memory, status note open in split view (colour emoji in the preview) | | **97 MB** |
| Opening a note with tasks changes the file | never | Verified: checksum unchanged |
| A status change touches only its character | one character | Verified on LF and CRLF notes; CRLF kept |
| Two status changes within a second | 1 write | **1 write** |

How milestone 2 works, in short:

- `core/src/status.rs` reads `statuses:` (block or flow list) from the note,
  then `_settings.md`, then the default. `task_box` is the one definition of
  a task, shared by highlighting, the preview, clicks and the menu.
- **Editor:** task boxes are coloured by status (blue open, green finished,
  red unknown). A click on the box advances it; a right-click anywhere on a
  task line opens the menu. The change selects the one character, replaces
  it and puts the caret back.
- **Live, Split and Reading:** each `[c]` is drawn as its symbol; a click
  advances it and a right-click opens the menu. ⚠ marks unknown characters.
- `statuses:` is re-read when you type in the frontmatter, and the team
  format when `_settings.md` is saved from the app. Edits made in other apps
  are picked up when the file watcher arrives in milestone 6.

### Live Preview (milestone 2½) measurements

Same machine, 2026-11-14. A 3,000-line note (the showcase note repeated).

| What | Before culling | Now |
|---|---|---|
| CPU for 50 keystrokes | 860 ms (≈ 17 ms each) | **≈ 310 ms (≈ 6 ms each)** |
| Memory with the note open | 130 MB | **102 MB** (84.5 MB with no note) |
| Idle CPU, reading or with a line being edited | | **0 ms** over 10 s |
| Opening a note changes the file | never | Verified |

How Live Preview works:

- `core/src/doc.rs` holds the note as lines with their own endings, so mixed
  or missing line endings survive. `core/src/live.rs` splits it into blocks
  (a line, a code fence, a table, the frontmatter, a `%%` comment) and turns
  each line into styled pieces with the markup hidden. Every piece knows its
  raw bytes, so a click lands on the matching raw character.
- `app/src/live.rs` draws every block except the one being edited, which is
  a small `text_editor` holding just that block. Each edit is written back
  into the doc at once, so autosave and undo always see the current text.
  Only lines touched by an edit (or whose block changed) are re-drawn.
- **Culling:** only blocks within ~900 px of the visible area are built.
  `app/src/live_text.rs` records each line's real height when it's laid out;
  lines not seen yet use an estimate. The edited block is always built, and
  scrolls itself into view after a change.
- **Undo** (`core/src/history.rs`) stores a snapshot of the text once per
  group of edits (typing on one line without a 1 s pause is one group).
  It works in Live and Source, and across switching between them.
- **Reading and Split** reuse the same renderer with `read_only` set: presses
  on links and chips act, nothing starts editing. Split's right pane is a
  second, read-only `Live` rebuilt from the Source text 150 ms after typing
  stops (and at once after a status click), keeping its scroll position.
- **Split scrolling together:** iced keeps the Source editor's scroll
  private, so `app/src/source_scroll.rs` follows it with cosmic-text's own
  rules (wheel, caret reveal, clamping), using line heights laid out with the
  editor's font and width. Each pane then puts an anchor at the same height:
  the editor's middle after a wheel, the caret's line after typing or a click.
  Scrolling the view scrolls the editor to match its middle. Heights of view
  lines not drawn yet are estimates, so a long jump is corrected once they are.

Known limits, for later:

- Selecting by dragging works in Live, but the page doesn't scroll by itself
  when you drag past its top or bottom edge yet (scroll with the wheel while
  holding the button). Selecting text in Reading isn't possible yet.
- Up/Down through a blank line forgets the column.
- The scrollbar can shift slightly while scrolling a long note for the first
  time, as estimated heights are replaced by measured ones.
- Images and embeds show as a placeholder (📎, 🖼), not the picture.

How to measure: `cargo build -p obsession --profile measure` builds a release
binary that keeps the debug-only hooks in `app/src/dev.rs`, then read the
process's working set, CPU time and I/O counters (see the README).

### Links (milestone 3)

| What | Measured |
|---|---|
| Building the link index, 10,000 notes (3.5 MB), background thread | **0.86 s** |
| Backlinks of one note, 10,000-note vault | **57 µs** |
| Resolving a link | hash lookups by name and path, not a scan of the vault |

How it works:

- **Resolution** (`core/src/vault.rs`): `./` and `../` relative paths, then
  an exact path from the root or from the linking note's folder, then every
  note whose path ends with the target. Several matches are ambiguous; we open
  the one Obsidian would (the linking note's folder first, then the shortest
  path, then alphabetical) and draw the link amber. Images and other files
  count as attachments, not missing notes.
- **The index** (`core/src/links.rs`) keeps per note only its outgoing links
  (target, line, a 160-character snippet) and its headings and block ids,
  plus a name -> linking notes map, so backlinks only resolve links whose
  name could match. It's read once when a vault opens, off the UI thread,
  and each note is re-indexed when it's saved.
- **Link colours** come from one map per open note (`dest_key` -> state),
  re-checked 150 ms after typing stops and shared by Source highlighting,
  Live, Reading and Split.
- **Rename** saves the open note, moves the file, then rewrites and saves
  (atomically) each note the index says links to it, re-indexes them and
  re-reads the folder.

Known limits, for later:

- Folders can't be renamed yet, only notes (moving a note into another folder
  works).
- Relative markdown links *inside* a moved note aren't adjusted.
- Attachments open from the vault root only (they aren't scanned yet).
- Backlink snippets show the raw line, and unlinked mentions aren't listed.
- Edits made by other apps reach the index when the file watcher arrives
  (milestone 6).

### Graph (milestone 4)

10,000-note vault, release build with the dev hooks (`--profile measure`):

| What | Measured |
|---|---|
| Idle CPU, 300-card graph open | **0 ms** over 6 s |
| Memory, 300-card graph open | **101 MB** (102 MB with an ordinary note) |
| Startup CPU including reading every note's links | **2.0 s** (1.7 s with an ordinary note) |

How it works:

- **The file** (`core/src/nodes.rs`) is read into nodes, wires and the
  layout comment. Edits change only what they must: one list item added or
  removed, or the layout comment rewritten; every other byte (line endings
  included) stays. A missing `## Nodes` or `## Wires` is added before the
  layout comment.
- **The view** (`app/src/graph.rs`) is one iced `canvas`. The grid, cards and
  wires are drawn into a cache that's kept until the view pans or zooms or
  the graph changes; the card being dragged (and its wires), the hovered or
  selected wire and the selection outline are drawn in a small overlay every
  frame. Cards and wires off screen are skipped; below 50% zoom cards are
  blocks with a title. Only the hovered or selected wire animates, and only
  while it is.
- **Cards** read each note through the store once (tasks with the note's own
  status format, done count). **Link wires** come from the link index: a
  `[[link]]` from note B to note A, both on the graph, draws A → B.
- **Placement:** cards without a saved position go in columns along the
  `## Wires` (link wires don't move cards), below anything already placed.
  Dragging a card saves every card's position.

Known limits, for later:

- A wire to a task or heading (`[[a#^t2]]`) attaches to the card, not the row.
- The view isn't saved: a graph opens fitted to the window.
- Link wires can't be removed from the graph yet (Syntax.md wants it to ask
  whether to remove the link from the note); Delete says where the link is.
- The card look is a first pass, to be matched to the Possess mockup.

### Function editor (milestone 5)

| What | Measured |
|---|---|
| Idle CPU, graph with function wires open | **0 ms** over 6 s |
| Idle CPU, function editor open | **0 ms** over 6 s |
| Core tests | 73, of which 15 for functions |

How it works:

- **Text first.** A function is parsed from its ```` ```obsession ```` block
  (`core/src/func/mod.rs`); the editor never keeps its own copy. Every change
  (move a node, connect a pin, set a value, add or remove a node, port or
  param) is a one-line edit from `func/edit.rs`, applied to the file that holds
  the function: the open `_nodes.md` (so Ctrl+Z undoes it) or `_funct.md`.
  Comments, blank lines and alignment on other lines stay as they were.
- **Checking** (`func/check.rs`) binds each node's arguments to the library's
  inputs, finds loops, types every node in order, and requires an action. An
  action's target must be an `out` or a task found in one. The wire is solid
  only if the function checks, it has the right shape for where it sits (one
  in, one out for a wire), the wire's ends are the right kinds (`[[a]]` File,
  `[[a#^id]]` Task, `[[a#Heading]]` Section), and the wire's values fit the
  params. A `fn` card is ready once every pin is wired.
- **Live values** (`func/eval.rs`) run the function against the wired notes
  (read through the store, so from RAM) each time the editor refreshes. Actions
  produce patches (line, old lines, new lines) and a sentence saying what they
  would change; nothing is written. `apply_patches` is ready for milestone 6
  and the tests use it to check that running twice changes nothing.
- **Turning solid** animates over 450 ms: the wire is drawn in the overlay with
  its gaps closing, then joins the cached layer. Frames are requested only
  while a wire is animating.

Known limits, for later:

- Edits to a saved function in `_funct.md` are saved straight away but aren't
  in Ctrl+Z (only the open note has undo history).
- Renaming a function (its `fn` line) doesn't update the wires that use it.
- The editor's view and the inputs node's place aren't saved.
- An `id` can't be renamed from the editor (edit the text).
- `Find Task` and `Set Status`'s `find` ignore case (Syntax.md says so now).

### Running functions (milestone 6)

Measured on a scratch vault (a graph with three functions, one of them a
`fn` node with pins):

| What | Measured |
|---|---|
| Opening the vault | **0 writes**; 2 wires shown out of date |
| Outside edit → run → saved | under 2 s (watcher, 1 s gap, one write per note) |
| Chain: anim → Check Lighting → lighting → Wait For Done → comp | one run, 2 changes, logged once |
| Idle CPU, watcher on | **0 ms** over 6 s |
| Core tests | 78 (runner 3, log 1, store 1 new) |

How it works:

- **The index** (`core/src/runner.rs`, `Index::build`) reads every graph file
  and `_funct.md` when the vault opens and whenever one of them is saved or
  changed outside. Each wire with a function, and each `fn` node with every pin
  wired, becomes a `Job`: compiled and checked once, its ports resolved to
  notes. Functions that don't check out are left out (their wires are dashed
  with ⚠). Every note a job touches is pinned in the store.
- **Triggers.** A note saved here (autosave, status clicks) or changed outside
  (`notify` watcher → `Store::check_outside`, which tells our own writes apart
  by content hash) adds it to a pending set. If a job reads it, a timer starts
  the run once a second has passed since the last one. Changes that arrive
  meanwhile join that one pending run.
- **A run** gathers the pinned texts (the open note's editor text included),
  runs `runner::run` off the UI thread, then applies the changes to what each
  note holds *now* (`runner::apply`: a change applies only where its old lines
  still are), saves each changed note once, updates the open note in place
  (undoable there too) and logs the run.
- **Editing a function or a wire doesn't run anything.** Its wire shows "out of
  date" instead, until you run it or a note it reads changes.
- **Undo** (`runner::revert`) goes newest change first and skips lines that
  changed since. It doesn't start chains.

Known limits, for later:

- The index and the out-of-date check run on the UI thread. Fine for dozens of
  functions; a vault with hundreds should move them to the background.
- Only the newest run can be undone, from the Runs panel (not Ctrl+Z).
- A note changed in another app while it has unsaved edits here keeps the
  edits here (with a warning), and they are saved over the other change.
- The link index is still built in full at startup (no cache in
  `.obsessed/cache/` yet).

### Drawing: only when something changes

- **Reactive rendering.** iced redraws only after an event. When nothing moves,
  the app draws nothing and uses no CPU.
- **Frame ticks only while animating.** The per-frame subscription is switched
  on while an animation runs and off when it ends. Animations are time-based,
  so they look the same at any frame rate.
- **Frame rate follows the display, 60 fps by default.** On a 60 Hz display
  (the common case) the app draws at 60 fps. Above 60 fps only happens when the
  display supports it, and it's capped at 120 fps even on 144 or 240 Hz
  displays. A **Limit to 60 fps** setting turns it off for battery.
- **Animations stay inside their widget.** Hover glows, wire dashes and card
  lifts live in the canvas or widget state and ask for a redraw themselves.
  They don't send app messages, so the whole view isn't rebuilt 120 times a
  second.
- **Animate paint, not layout.** Fade, move, scale and color animate. Nothing
  animates the size of a row or panel, because that re-runs layout every frame.
- **`lazy` for big, rarely-changing parts.** The sidebar, preview and card hand
  are wrapped in iced's `lazy`, so they're only rebuilt when their data changes.
- **Long lists are virtual.** The file tree, search results and backlinks only
  build the rows on screen.

### The graph canvas

- **Layers with separate caches:**
  1. background grid: rebuilt only on zoom
  2. node cards: rebuilt when a card's content changes
  3. wires: rebuilt when nodes move or wires change
  4. overlay (hover, selection, the dragged node and its wires, animations):
     the only layer redrawn every frame
- **Dragging** moves the node into the overlay layer, so the other 499 cards stay
  cached.
- **Culling.** Only nodes and wires inside the visible area are built.
- **Level of detail.** Zoomed out, cards become colored blocks with a title, and
  task text is skipped. Text is the most expensive thing to draw.
- **Dashes are animated only where you look.** Dashed lines cost more to build
  than solid ones. The "marching ants" run only on the hovered or selected
  wire. Every other dashed wire is drawn once and cached.
- **Escape hatch.** If rebuilding geometry while panning and zooming ever misses
  the budget, cards and wires move to iced's `shader` widget, a custom wgpu
  pipeline where panning is just a matrix change. Only if measuring shows we
  need it.

### GPU settings

- **Power preference: low power.** Uses the integrated GPU on laptops: less
  memory, less battery, plenty for 2D at 120 Hz.
- **Anti-aliasing is measured, not assumed.** 4× MSAA smooths wires, but at
  1440p it needs about 60 MB of extra framebuffer, which is shared RAM on
  integrated GPUs. Milestone 4 compares MSAA on, off, and drawing wires as
  slightly feathered geometry. The default is the lightest option that still
  looks clean, with MSAA as a setting.
- **Present mode follows vsync**, so frames line up with the monitor and the app
  never draws faster than the display shows. On displays faster than 120 Hz,
  redraw requests are spaced so the app stays at 120 fps.

### Memory: keep an index, not the vault

- **Only open files are fully in memory.** For every other file the app keeps a
  small index entry: path, modified time, size, task count, links, `@` mentions
  and status characters.
- **The file store keeps function files in RAM** (see
  [Disk](#disk)). Shot files are small, typically a few KB, so even 1,000 of
  them are only a few MB.
- **Spans, not copies.** Tasks, links and lines are stored as byte ranges into
  the file's text, not as separate strings.
- **Interned names.** Paths, `@names` and status characters are stored once and
  referred to by a small number everywhere else.
- **Compact types.** `Box<str>` instead of `String` for text that never grows,
  `u32` indexes instead of pointers, and no `HashMap` where a sorted `Vec`
  will do.
- **Portraits decoded small.** Images are decoded off the UI thread at card size
  (about 256 px), never at full resolution, and their textures are freed when
  the card hand is hidden.
- **Fonts:** one embedded UI font. Emoji come from Windows' own Segoe UI Emoji
  instead of a bundled emoji font.

### Functions and files

- **Compile once.** Each function is compiled from its text into a flat list of
  steps in run order. It's recompiled only when its text changes.
- **Run only what's affected.** An index maps each file to the functions that
  read it, so a change runs just those functions.
- **No copying while running.** `Lines`, `Split` and `Tasks` return ranges into
  the file text. Only actions build new text.
- **Out-of-date checks in the background.** On opening, the open graph is checked
  first, then the rest of the vault a few functions at a time at low priority,
  with each file released after its check.
- **Indexing is incremental.** The index is saved in `.obsessed/cache/` with
  each file's modified time and size. At startup only changed files are parsed,
  on a few background threads (no `rayon`).

### CPU

- **Nothing runs when idle.** No polling or repeating timers. The file watcher
  is event-driven, and the function timer only exists while a run is pending.
- **Background work is polite.** Indexing, out-of-date checks and function runs
  use at most 2 threads at below-normal priority, so they never compete with
  the UI or with Maya, Blender or Unreal running next to the app.
- **Debounced work.** The preview re-renders 150 ms after typing stops, not on
  every key. Search waits for a pause in typing too.
- **Incremental everything.** A changed file re-parses only that file. Syntax
  highlighting re-runs only from the edited line down, and only for lines on
  screen.
- **Vault search is built on first use**, not at startup.

### Disk

**RAM first: read once, then work in memory**

All file access goes through one **file store** in `obsession-core`. It holds
one copy in RAM of every file the app is working with:

| Kept in RAM | How long |
|---|---|
| Files open in the editor | While open |
| Every file a function reads or writes | **Pinned** for as long as the function exists |
| Other recently used files | Until a 16 MB budget is full, then least recently used first |

- **A file is read from disk once.** After that, function runs, chains, live
  pin values and out-of-date checks all read the copy in RAM. A warmed-up run
  does **zero disk reads**.
- **Disk is read again only when a file changed outside the app.** On a watcher
  event the store first compares modified time and size, which costs no file
  read. It re-reads only if they differ from what it last read or wrote.
- **Changes happen in RAM first.** Edits, status clicks and function results
  update the store's copy. Chains run entirely in RAM.
- **The disk gets the result, once.** At the end of a run, each changed file is
  written once. Runs happen at most once a second, so functions write any file
  at most once a second.
- **Editor and functions share one save.** If you're typing in a file that a
  function also changes, both changes land in the same copy in RAM and go out
  in a single write.
- **Own writes don't cause re-reads.** After writing, the store records the new
  modified time, size and content hash. When the watcher reports that write,
  it's recognised and ignored.
- **Writes can't be skipped entirely.** Files have to reach the disk so
  Obsidian, git and teammates see them. What we avoid is writing the same file
  repeatedly or writing when nothing changed.

**Other reads**

- The saved index means startup reads only files whose modified time or size
  changed.
- Files are read in full only when opened or needed by a function, never just
  to draw a card or a list row. Those come from the index.
- The watcher ignores `.git/`, `.obsidian/` (Obsidian rewrites
  `workspace.json` constantly) and `.obsessed/`.

**Writes**

- **Notes:** autosave 1 second after typing stops, skipped if unchanged. Function
  runs write each changed file once. See [How the main pieces work](#how-the-main-pieces-work).
- **Undo log:** appended, never rewritten. Each run's entries are buffered and
  flushed once at the end of the run. When the log grows past its limit,
  the oldest file is dropped.
- **Index cache:** saved on close, and at most every 5 minutes while it has
  unsaved changes. Never after every edit.
- **Window and graph state:** saved on close only.
- **Nothing is written while idle.**

### Build settings

```toml
[profile.release]
opt-level = 3        # speed first: we need the frame budget
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

A faster allocator (`mimalloc`) gets tried in milestone 1 and is kept only if it
measurably lowers memory or frame times.

### Measuring

- **`obsession-bench`**, a small tool in the workspace, generates test vaults
  (100, 1,000 and 10,000 files, a 500-node graph, long files with 5,000 tasks)
  and times indexing, parsing and function runs.
- **Frame times** come from iced's debug tools and from Tracy with the `profile`
  feature.
- **Memory** is read from the Windows working set and peak working set after a
  scripted session: open vault, open graph, pan, zoom, drag, run functions.
- **CPU and disk** come from the process's CPU time and Windows I/O counters
  (bytes read and written) over the same session, plus 5 idle minutes. Those
  idle minutes must show no CPU use and no bytes written.
- **The file store counts its own disk reads and writes** per run, so
  `obsession-bench` can check that a warmed-up run reads nothing from disk and
  writes each changed file exactly once.
- A milestone isn't done until its budgets pass.

## Milestones

Each milestone ends with something you can run and use, and with the
performance budgets checked.

| # | Milestone | Done when |
|---|---|---|
| 1 | **Vault and editor** ✅ | Open a folder, file tree, raw editor with highlighting, preview, autosave, theme, quick switcher |
| 2 | **Tasks and statuses** ✅ | Status chips in editor and preview, click to advance, right-click menu, status formats, ⚠ for unknown characters |
| 2½ | **Live Preview** ✅ | Formatted editing with the clicked line raw, Source, Split and Reading views, undo/redo |
| 3 | **Links** ✅ | Shortest-name resolution matching Obsidian, ⚠ on ambiguous links, backlinks panel, renaming a file updates links |
| 4 | **Graph** ✅ | Read and write `_nodes.md`, node cards (like the Possess mockup), pan, zoom, drag, layout saved, dashed wires from `_nodes.md` and from `[[links]]` |
| 5 | **Function editor** ✅ | Hover a wire, "ƒ" button, inner canvas, the full node library, type checking, dashed-to-solid animation, live values on hover, `_funct.md` saved functions |
| 6 | **Running functions** ✅ | File watcher, throttled runs, chains, loop guard, batched atomic writes, undo log in `.obsessed/`, outside edits |
| 7 | **Crew** | `_crew/` cards, `@` autocomplete, drag a card onto a task, hover to highlight, OVERLOADED and POWERHOUSE badges |
| 8 | **Polish** | Animation pass, compact mode, vault search, performance on large vaults |

## Risks

- **iced's editor is not Obsidian's.** Fine for notes and tasks. If it gets in
  the way, the preview stays the main reading view.
- **Large graphs.** Layered caches, culling and level of detail first, the
  `shader` widget if measurements say so.
- **GPU baseline memory on Windows.** Partly outside our control. Measure it in
  milestone 1 and set honest budgets from it.
- **Anti-aliasing cost.** MSAA can cost more memory than the rest of the app.
  Measured in milestone 4.
- **Several teammates running the app.** Every action is safe to run twice
  (Set Status, Replace Text, Ensure Line, Write Section), so two apps firing the
  same function after a `git pull` produce the same file.
- **Link resolution differences with Obsidian.** Test against a real Obsidian
  vault with duplicate file names.

## Decisions so far

- Rust + iced. Start fresh, nothing carried over from Possess.
- Graph files are any note named `…_nodes.md` (`_nodes.md`, `lighting_nodes.md`);
  a folder can hold several.
- Files starting with `_` mean something to the app: `_nodes.md`, `_funct.md`,
  `_crew/`, `_settings.md`.
- `.obsessed/` holds machine-local data (undo log, window state, caches) and
  ignores itself in git.
- No `needs::`, no `sync::`, no status roles. Functions replace all of them.
- No exec wires. Actions have a `when` input.
- A function with one input and one output is drawn on its wire. More inputs or
  outputs make it a visible node.
- Plain `[[links]]` show as dashed wires. Wires dragged in the graph are saved in
  `_nodes.md`.
- Saved functions are vault-wide in `_funct.md`. Editing one updates every wire
  using it. Duplicate it to make a variant.
- Links use Obsidian's shortest unique name.
- Crew badges compare `@` mentions on unfinished tasks with the median of the
  rest of the crew.
- Windows first.
