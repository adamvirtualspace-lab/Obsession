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
      crew.rs           _crew/*.md cards, @mentions, add_mention, badges
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

- ~~Selecting by dragging works in Live, but the page doesn't scroll by itself
  when you drag past its top or bottom edge yet (scroll with the wheel while
  holding the button). Selecting text in Reading isn't possible yet.~~ Done
  in milestone 11, item 1.
- ~~Up/Down through a blank line forgets the column.~~ Done in milestone 11,
  item 1.
- ~~The scrollbar can shift slightly while scrolling a long note for the first
  time, as estimated heights are replaced by measured ones.~~ Done in
  milestone 11, item 1 (it shifted a lot in a narrow window).
- Images and embeds show as a placeholder (📎, 🖼), not the picture. →
  Milestone 10 (images; note embeds stay out of version 1).

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

- ~~Folders can't be renamed yet, only notes (moving a note into another folder
  works).~~ Done in milestone 11, item 2.
- ~~Relative markdown links *inside* a moved note aren't adjusted.~~ Done in
  milestone 11, item 2.
- ~~Attachments open from the vault root only (they aren't scanned yet).~~
  Done in milestone 10, item 1.
- ~~Backlink snippets show the raw line, and unlinked mentions aren't
  listed.~~ Done in milestone 11, item 2.
- ~~Edits made by other apps reach the index when the file watcher arrives
  (milestone 6).~~ Done in milestone 6.

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

- ~~A wire to a task or heading (`[[a#^t2]]`) attaches to the card, not the
  row.~~ Done in milestone 9, item 3.
- ~~The view isn't saved: a graph opens fitted to the window.~~ Done in
  milestone 9, item 2.
- ~~Link wires can't be removed from the graph yet (Syntax.md wants it to ask
  whether to remove the link from the note); Delete says where the link is.~~
  Done in milestone 9, item 4.
- ~~The card look is a first pass, to be matched to the Possess mockup.~~
  Done in milestone 11, item 4.

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

- ~~Edits to a saved function in `_funct.md` are saved straight away but
  aren't in Ctrl+Z.~~ Done in milestone 9, item 6.
- ~~Renaming a function (its `fn` line) doesn't update the wires that use
  it.~~ Done in milestone 9, item 5: rename from the editor's title.
- ~~The editor's view and the inputs node's place aren't saved.~~ The view
  is done in milestone 9, item 2; the inputs node's place in milestone 11,
  item 4.
- ~~An `id` can't be renamed from the editor (edit the text).~~ Done in
  milestone 11, item 4.
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

- ~~The index and the out-of-date check run on the UI thread. Fine for dozens
  of functions; a vault with hundreds should move them to the background.~~
  Done in milestone 11, item 3.
- Only the newest run can be undone, from the Runs panel (not Ctrl+Z).
- A note changed in another app while it has unsaved edits here keeps the
  edits here (with a warning), and they are saved over the other change.
- ~~The link index is still built in full at startup (no cache in
  `.obsessed/cache/` yet).~~ Done in milestone 8.

### Crew (milestone 7)

| What | Measured |
|---|---|
| Idle CPU, graph with the crew hand | **0 ms** (one 16 ms blip in 6 s) |
| Core tests | 82 (crew 4 new) |

How it works:

- **Cards** (`core/src/crew.rs`) are read from `_crew/*.md` through the store
  when the vault opens, after rescans, and when a card changes.
- **Mentions** are counted by the link index as it reads each note: every
  `@name` on a task line with the task's status, plus the note's own
  `statuses:`. Open counts (not in the last status) for the whole vault are a
  quick pass over the index, done when links refresh. In a graph they come
  from the node cards' tasks.
- **Badges:** each person against the median of everyone else (2×
  OVERLOADED, 1.5× POWERHOUSE; none under 3 people or with a median of 0).
- **The hand** is drawn on the graph canvas (overlay layer), so dragging a
  card onto a task row is plain hit testing. Dropping adds `@name` to that
  line (`crew::add_mention`, before a trailing `^id`), saves the note once and
  lets functions run as for any edit.
- **`@` autocomplete:** after each keystroke the note reports the `@name`
  being typed before the caret; suggestions come from the crew. Tab (bound in
  both editors only while suggestions show) replaces the typed part with
  `@name `.
- **`@names` in Live Preview** link to their card. Names without a card are
  drawn in amber (they go in the same state map as `[[links]]`).

Known limits, for later:

- Portraits aren't drawn yet: every card shows initials in its colour (bots a
  square face). `portrait:` is read and kept for when images arrive. →
  Milestone 10.
- ~~Highlighting and drop targets cover the first 8 task rows a card
  shows.~~ Done in milestone 11, item 4 ("+N more" opens a card).
- ~~Hover highlighting was checked by reading the code, not by an automated
  screenshot (the dev hooks can't hover the canvas).~~ Checked by hand in
  November 2026 (see "Checked by hand").
- `@` suggestions only list people in the crew folder.
- ~~The Settings page has one setting so far (the crew folder). Status
  formats are still edited in `_settings.md` itself.~~ Done in milestone 11,
  item 4.

### Polish (milestone 8)

10,000-note vault (3.5 MB of notes), `--profile measure`:

| What | Before | After |
|---|---|---|
| Startup indexing, files already in Windows' file cache | 1.9 s, every note read | **0.6 s**, no note read (cache) |
| Startup indexing right after a reboot | 50 s (cold reads) | file times only, no note read |
| Startup CPU | 3.0 s | **1.6 s** |
| Vault search: first use (reads every note) | – | **1.0 s**, 7 MB |
| Vault search: a query | – | instant (well under a frame) |
| Memory: a note open / search built | 110 MB | 115 MB / 138 MB |
| Idle CPU: note, graph, search open | 0 ms | **0 ms** |
| Core tests | 83 | 86 |

What changed:

- **Link index cache** (`core/src/links.rs`): each note's links, headings,
  block ids, task `@mentions` and own `statuses:` are saved in
  `.obsessed/cache/links.txt` with its modified time and size. At startup a
  note whose time and size match is taken from the cache without reading it.
  The cache is written after a build that read anything, and on close if notes
  were saved meanwhile. A saved note is re-stamped after its write; a note
  changed by another app is read again next time.
- **Vault search** (`core/src/search.rs`, `app/src/find.rs`): built on first
  use on a background thread. Each note keeps its text and an ASCII-lowercased
  copy (same byte offsets), so a query is a plain `contains` per line, and the
  match positions point straight into the real line. Up to 300 hits are shown,
  all are counted. It's kept current wherever the link index is.
- **Compact layout:** a preference of this computer (`%APPDATA%\Obsession\
  prefs.txt`), toggled in Settings. Toolbar padding and gaps, the file tree's
  row height and the toolbar's margins shrink; the editor and the graph don't
  change.
- **Animation pass:** wires and function nodes whose function just ran glow
  green and fade over 1.2 s (only runs that changed something). A crew card
  eases up when hovered (140 ms). Both are time-based and draw frames only
  while they move.

Known limits, for later:

- ~~Search ignores case for ASCII letters only (`É` doesn't match `é`).~~
  Done in milestone 11, item 3.
- ~~Memory with search built grows by more than the 7 MB of text (138 MB vs
  115 MB with the results list open); not looked into yet.~~ Found and cut
  in milestone 11, item 3.
- Compact layout doesn't change the editor's line spacing or the graph cards.
- Crew portraits are still initials. → Milestone 10.

### Checked by hand (November 2026)

The `--profile measure` build was driven with a real mouse and keyboard
(Computer use) on a small test vault: four crew cards with OVERLOADED and
POWERHOUSE, a `shots/_nodes.md` with Wait For Done, Copy Status, a broken wire
and `fn Check Lighting`, and an `@zed` with no card. Everything on the list
worked, apart from the bugs below, which are fixed:

| Found | Cause | Fix |
|---|---|---|
| Tab didn't complete an `@name` (Live and Source); a click did | Showing the suggestions wrapped the page in a `stack!`, so the editor moved in the widget tree, lost its state and its focus, and its Tab binding never ran | The page and the window are always a `stack!` (one layer when nothing is open), so widgets keep focus and scroll when popups open or close. An unused Tab also completes, as a fallback (`Shortcut::Tab`) |
| Escape took two presses to close search, the quick switcher and the new-note box | A focused text box captures Escape to drop its own focus | Escape always reaches `App::shortcut` |
| Unknown `@names` lost their amber after Source → Live, and in Split's preview | A new `Live` started with no link states, and `set_link_states` skips unchanged states | `Note::set_mode` and the Split mirror copy the note's link states (and the suggestion flag) into the new `Live` |
| `@name` suggestions stayed up after opening another note | Not cleared on open | `App::open` clears them |
| A crew card stayed raised after being dropped on a task, until the mouse moved | Hover was only worked out on mouse moves | Worked out again when a drag ends (graph and function editor, whose pin value box had the same problem) |
| Hovering a wire to or from a function node's pin offered "Add a function", then refused it | The ƒ button didn't check the wire's ends | No ƒ on those wires (`Graph::takes_function`) |
| A hovered wire's "out of date" reason was hard to read over a nearby card | Captions were drawn with no background | Captions of the hovered wire get a dark backing |
| A crew card made by clicking `@name` (and a new note, and `_funct.md`) showed in Runs as "Changed in another app", twice | The file was written outside the store, so the watcher's events looked like another app's | The store reads the file right after the app creates it (core test `a_file_read_right_after_creating_it_is_not_an_outside_edit`) |

Measured after the fixes: idle CPU **0 ms** and **0 bytes** written over 10 s
each with a note open, the graph with the crew hand, and vault search open. A
hovered wire draws its marching ants (about 30 ms of CPU a second) and stops
when the mouse leaves. Core tests: 87.

Still not right, for later:

- With the graph open, the toolbar is wider than a 1138-pixel-wide window
  (2560 × 1600 at 225 %): the last view buttons are cut off. → Done in
  milestone 9, item 1.
- With six crew cards and the backlinks panel open, the hand overlaps enough
  that badges run into each other. → Milestone 9.
- ~~An unknown `@name` is amber only when its line is drawn, not on the line
  being edited.~~ Done in milestone 11, item 4.
- The run glow (1.2 s, starting about a second after a change) is easy to miss;
  `OBSESSION_DEV_PULSE_MS` slows it down to look at.

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
- **Anti-aliasing: smooth wires, no MSAA.** Wires are meshes with a soft
  edge one screen pixel wide (`app/src/smooth.rs`), the "feathered geometry"
  option. Measured against iced's 4× MSAA (see "Smooth wires" below): the same
  frame rate, but MSAA took ~120 MB more GPU memory at 2560 × 1600 and can
  only be chosen at startup. MSAA stays off.
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
  is event-driven (`notify` 9 on every system; up to 8.2 its Windows backend
  woke every 100 ms), and the function timer only exists while a run is
  pending. Checked by counting each thread's CPU cycles
  (`bench/scripts/idle-threads.ps1`): idle, no thread runs at all.
- **Bursts are spread out.** Many notes changed at once by another app (a git
  pull, a sync) are queued and read a slice at a time, off the UI thread,
  with pauses between; rescans start at most every 250 ms. See "Idle and
  bursts".
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
  feature. Panning a graph: `OBSESSION_DEV_PAN=600` pans the open graph by
  itself and prints frame intervals and how long the cached layer took to
  build; `ICED_PRESENT_MODE=immediate` lifts the display's frame cap.
- **Memory** is read from the Windows working set and peak working set after a
  scripted session: open vault, open graph, pan, zoom, drag, run functions.
- **CPU and disk** come from the process's CPU time and Windows I/O counters
  (bytes read and written) over the same session, plus 5 idle minutes. Those
  idle minutes must show no CPU use and no bytes written. Process CPU time is
  charged in 15.6 ms ticks and misses short wakeups, so idle CPU is checked by
  counting each thread's cycles (`bench/scripts/idle-threads.ps1`), and the UI
  thread's load during a burst with `bench/scripts/ui-load.ps1`.
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
| 7 | **Crew** ✅ | `_crew/` cards, `@` autocomplete, drag a card onto a task, hover to highlight, OVERLOADED and POWERHOUSE badges |
| 8 | **Polish** ✅ | Animation pass, compact mode, vault search, performance on large vaults |
| 9 | **Graph and functions, finished** ✅ | The toolbar fits, views are remembered, wires reach rows, link wires can be deleted, function renames reach their wires, function edits undo, the crew hand stays readable |
| 10 | **Images** ✅ | Pictures in notes, crew portraits, attachments from any folder, within a memory budget |
| 11 | **Known limits** | The editor's rough edges, links, scale and search, the graph and crew finished |

## Next

Milestones 1–8 cover version 1's scope. These finish what's half done
(each milestone's "Known limits, for later") before anything from
[Not in version 1](#not-in-version-1). Each item says where the work goes;
anything that changes file text goes in core with tests.

### Milestone 9: Graph and functions, finished

In this order:

1. **The toolbar fits.** With a graph open, the toolbar is wider than a
   1138-pixel window. The graph's own tools (**+ Node**, **+ Function**,
   **Crew**, **Link wires**, **⚠ Run N out of date**) move onto the graph as a
   small bar in its top-left corner, so the top toolbar is the same for notes
   and graphs. *Done when* every button is reachable at 1000 pixels wide, in
   both layouts. (`main.rs` `view_toolbar`, `graph.rs`)

   ✅ **Done (November 2026).** `App::view_graph_bar` draws the bar on a
   `stack!` over the canvas (always two layers, so pan and zoom survive the
   bar changing). The bar is `opaque`, so a click or drag on it never reaches
   the cards; its buttons wrap onto a second line in a narrow graph. In the
   function editor it only holds the run button, when something is out of
   date. The note's path in the top toolbar takes the space left and is cut
   off, so a deep path can't push the buttons out either. Checked by hand at
   1002 × 632 (laptop screen, 225 %), normal and compact: every top button
   visible, each bar button works, a drag on the bar doesn't pan, the graph
   keeps its view. Idle: 0 ms CPU, 0 bytes written over 10 s with the graph,
   and with the function editor. Found on the way: after **+ Node** and Esc,
   the next Ctrl+P added the chosen note to the graph instead of opening it
   (Esc didn't clear the add-node flag); fixed.
2. **Views are remembered.** Each graph's pan and zoom, and each function's
   view in the editor, are kept in `.obsessed/views.txt` (this machine only,
   as Syntax.md section 11 says). Written when you leave a graph or close the
   app, never while idle. A graph seen for the first time still opens fitted.
   (`settings.rs` or a small `views` module in core with a parse/write test,
   `graph.rs`, `func_editor.rs`)

   ✅ **Done (November 2026).** `core/src/views.rs` keeps each view as the
   graph point in the middle of the canvas plus the zoom, so it comes back
   centred whatever the window's size; functions are keyed by the file that
   holds them (`_funct.md#Wait For Done`, `shots/_nodes.md#Check Lighting`).
   The canvases publish `Viewed` after a pan or a zoom; the app keeps it in
   memory and writes the file only on leaving a graph, switching vaults and
   closing, dropping views of files that are gone. Renaming a graph carries
   its views. Coming back from the function editor no longer re-fits the
   graph either (the editor replaces the graph's canvas, which lost its
   view). Checked by hand: a zoomed and panned graph comes back the same
   after the editor, after opening another note and after a restart; a
   function opens at its own view after a restart; a graph never seen opens
   fitted; nothing is written while panning or zooming, only on leaving or
   closing. Idle: 0 ms CPU, 0 bytes written over 10 s with the graph and with
   the editor. Core tests: 92 (5 new).
3. **Wires reach their rows.** A wire to a task or a heading
   (`[[a#^t2]]`, `[[a#Notes]]`) ends at that row of the card, or at the card's
   edge when the row isn't shown. (`graph.rs`; the row lookup in core with a
   test)
   ✅ **Done (November 2026).** `links::task_at(text, sub, task_lines)` finds
   the task a `#^block` points at (the task on its line), or the first task
   in a heading's section (up to the next heading of the same or a higher
   level, subsections included). Each `WireView` keeps `from_row` / `to_row`
   and `wire_ends` uses the row's middle; a wire that leaves a row starts
   with a small dot. A link wire uses the row only when every link from that
   note to the card points at the same row. Ends stay at the card for a
   heading with no task under it, a block on a line that isn't a task, a row
   past the eighth (the card shows "+N more") and a missing card. Checked by
   hand: `[[anim/sh040#^lip]] → [[lighting/sh040#^t2]]` runs from the Lip
   sync row to the Rim light tweak row; a `[[comp/sh040#Grade]]` link in the
   FX note leaves comp's card at Final grade, the first task under
   `## Grade`; hover still lights the moved wires. Idle: 0 ms CPU, 0 bytes
   written over 10 s. Core tests: 93 (1 new).
4. **Link wires can be deleted.** Delete on a link wire asks "Remove the link
   to [[b]] from a?" (Remove / Cancel), as Syntax.md section 7 says. Removing
   keeps the link's words: `[[b|the shot]]` becomes `the shot`, `[[b]]`
   becomes `b`. Undoable like any edit. (`links::remove_link(text, line,
   target)` in core with tests, then `main.rs`)
   ✅ **Done (November 2026).** Delete on a link wire opens "Remove the link
   to [[comp/sh040]] from fx/sh040-water?" (or "the 2 links" when the note
   links there more than once) with Cancel / Remove; Escape or a click
   outside keeps it. Remove takes out every link in that note that leads to
   the card's note, keeping its words: `links::remove_links(text, pick)`
   with `link_words` (alias or label, else the note, `b > Heading` for a
   heading, a `^block` dropped). Undo: the other note isn't open, so it has
   no editor history; `history::Elsewhere` keeps each such change (that
   note's text before and after) with a fingerprint of the open note's text
   at the time, and Ctrl+Z takes it only while the open note is back at
   that text, so it and the graph's own edits undo in the order they were
   made. Undo refuses, with a message, when the other note changed since.
   Steps are dropped when another note opens. The crew drop (`assign`) now
   goes through the same path, so it undoes too. Checked by hand: the
   dialog's count, Escape, Remove (the FX note read "Waiting on comp/sh040 >
   Grade. Ask the comp team too."), Ctrl+Z (byte for byte back), Ctrl+Y,
   a card move after it undoing first, the crew drop's undo, and the refusal
   after an outside edit. Idle: 0 ms CPU, 0 bytes written over 10 s. Core
   tests: 95 (2 new).
5. **Renaming a function reaches its wires.** The function editor's title
   becomes editable. Renaming rewrites the `fn` line and every wire, `fn Name`
   node, `Name.pin` end and layout line that uses it: in its own graph for a
   local function, in every graph for a saved one. A name already taken is
   refused with a sentence saying so. (`nodes::rename_function(text, old,
   new)` in core with tests, then `main.rs`)
   ✅ **Done (November 2026).** The editor's title is a text field that looks
   like the title until hovered or clicked; while it differs, a line below
   says "Enter renames it, and every wire and node that uses it". An empty
   name, or one with `( ) . : [ ]`, is refused in the panel. Core:
   `func::edit::rename(text, block, new, heading)` rewrites the block's own
   `fn` line (indent and comment kept) and, in `_funct.md`, the `## Name`
   heading just above it when it carries the old name;
   `nodes::rename_function(text, old, new)` rewrites `- fn Old`, the call on
   a wire (arguments kept), `Old.pin` ends and the `fn Old x,y` layout line,
   matching names without case and keeping the spacing. A local function is
   renamed in its graph only: one edit, undone like any other. A saved one
   is renamed in `_funct.md` and in every graph that uses it, except a graph
   with its own function of that name (its wires mean that one). Taken names
   are refused in the status line: "This graph already has a function called
   X", "_funct.md already has …" or "fx/water_nodes has its own function
   called X"; a change of case alone is allowed. Undo: an `Elsewhere` step
   now holds several files, and a `joined` step goes with the open graph's
   own edit, so one Ctrl+Z (or Ctrl+Y) takes the whole rename. The editor
   follows its function through a rename and its undo (`refresh_editor`
   falls back to the block at the same lines), and the function's saved view
   moves with it (`Views::move_view`). Checked by hand: *Wait For Done*
   renamed in `_funct.md` (fn line and heading) and two graphs, one Ctrl+Z
   back byte for byte in all three, Ctrl+Y; the local *Check Lighting*
   renamed in its node, three pin ends, block and layout line, the card
   staying in place with its wires, then undone and redone; "Copy Status"
   refused with nothing written. Idle: 0 ms CPU, 0 bytes written over 10 s.
   Core tests: 97 (2 new).
6. **Function edits undo.** Ctrl+Z in the function editor undoes its last edit
   wherever the function lives, `_funct.md` included (today only the open
   note has history). (`main.rs`, `history.rs`)
   ✅ **Done (November 2026).** A local function already lived in the open
   graph, so its edits were in the graph's history. A saved function's edit
   now goes through `edit_files` as an `Elsewhere` step ("Edited Wait For
   Done in _funct.md", shown only by Undone/Redone), so Ctrl+Z and Ctrl+Y
   take it in turn with the graph's own edits, newest first, and refuse with
   a message when `_funct.md` changed outside since. Saving goes through
   `put_files` like any other note change (search and links kept up to date,
   the run index rebuilt). `Elsewhere` now keeps to the same 8 MB cap as a
   note's history, as each step holds `_funct.md` before and after. Checked
   by hand: on *Wait For Done*, added a node and set its value; Ctrl+Z (with
   the cursor still in the value box) took the value off, again took the
   node, `_funct.md` back byte for byte; Ctrl+Y twice; then a wire value
   (the graph) changed and three Ctrl+Z undid the wire value, the node value
   and the node in that order, then "Nothing to undo"; an outside edit to
   `_funct.md` made undo refuse and was kept. Idle: 0 ms CPU, 0 bytes
   written over 10 s. Core tests: 98 (1 new).
7. **The crew hand stays readable.** With many people or a narrow graph, cards
   overlap but each name and badge stays visible; the raised card shows in
   full. *Done when* eight people fit a 700-pixel-wide graph. (`graph.rs`
   `draw_crew`)
   ✅ **Done (November 2026).** A resting card that the next one covers
   draws its face, name and badge stacked and centred in the strip that
   still shows (`draw_hand_strip`): the name shortened with … to fit, the
   badge as its word, a short one (OVERLOAD, POWER) when that doesn't fit,
   or a solid pill in its colour when no word does; the role (or `@name`)
   when there's no badge. Below 24 px a strip keeps just the initials and a
   badge bar. The hand keeps 20 px free at each side (was 40). The raised
   card is drawn whole, and cards it covers lay their words out in what
   it leaves showing. Text in an iced canvas is drawn above every shape in
   it, so graph labels (a card's title, the ƒ) showed over the hand and a
   covered card's words through the raised one: the hand now draws in a
   canvas of its own stacked over the graph (`HandLayer`), which only draws;
   the graph canvas keeps every event and leaves what the hand looks like
   this frame in `hand_now`. Checked by hand with eight people on a 696 px
   graph: every name (two shortened) and every badge or role readable;
   hovering a middle card raises it whole with nothing showing through;
   dragging a card still deals the `@bob` token and lights Bob's nodes;
   **Crew** still hides the hand; on a 390 px graph the strips keep their
   initials and badge colours. Idle: 0 ms CPU, 0 bytes written over 10 s.

*Milestone done when* each item is checked by hand in the real window (as in
"Checked by hand" above), core tests cover the new text edits, and idle CPU is
0 ms with 0 bytes written over 10 s with a graph and the function editor open.

Left for later: the inputs node's place in the editor (needs a syntax line),
renaming an `id` from the editor, matching the cards to the Possess mockup.
→ All three done in milestone 11, item 4.

### Milestone 10: Images

1. **Attachments are scanned**, so `![[shot.png]]` and `[[ref.pdf]]` resolve
   from any folder, the way Obsidian finds them. (`vault.rs`, with tests)
   ✅ **Done (November 2026).** `Vault::scan` also lists files with an
   attachment extension (`is_attachment`: pictures, PDFs, video, audio,
   `.blend`…) in `Vault::files`, sorted, with name and path lookups of their
   own; they stay out of `entries`, so the tree and note lists don't change.
   `resolve_from` finds them with the same steps as notes (`find`, shared):
   `./` and `../`, a path from the root or the note's folder, then by name,
   the note's folder first, then the shortest path. `Resolved::Attachment`
   carries the file's path, or `None` when it isn't in the vault: that link
   shows as missing (it used to look fine), and a click says "Can't find
   lost.png in the vault" instead of looking only at the vault root.
   `Vault::attachment(target, from)` is the lookup item 2 will draw from. A
   rescan (any file created, removed or renamed) picks new files up.
   Checked by hand: `![[plate.png]]` from `anim/` finds `comp/plate.png`
   (bright) and `![[lost.png]]` shows missing; clicking `[[lost.png]]` says
   so and creates no note; copying a `lost.png` into `refs/` while open
   turns the link bright. Scan: 10,000 notes and 10,000 pictures in 47 ms
   (`OBSESSION_DEV_TIMING` now times the scan too). Idle: 0 ms CPU, 0 bytes
   written over 10 s. Core tests: 99 (1 new, 2 extended).
2. **Pictures in notes.** `![[shot.png]]` and `![](shot.png)` draw the picture
   in Live, Split and Reading, fitted to the text column. Decoded off the UI
   thread at the size drawn, not full size; pictures far off screen are
   dropped from memory. (`app/src/live.rs`, iced's `image` feature)
   ✅ **Done (November 2026).** Core: `live::line` lists a line's picture
   embeds in `Line::pictures` (`Picture { seg, target, width }`, from
   `![[x.png|300]]` and `![alt|300](path%20x.png)`; web pictures, code and
   other embeds aren't), and their placeholder pieces draw nothing.
   `app/src/pictures.rs`: `Pictures` (decoded pictures by file and width,
   at most 64 MB, the least recently wanted that aren't near the screen
   going first; the open note's target → file lookups) and `decode` (off
   the UI thread, two at a time, EXIF rotation, shrunk with a box filter to
   the width drawn times the scale factor, never enlarged), and the
   `PictureColumn` widget (fits each picture to the column, writes its
   height back for the page's sums, a 200 px box while loading, a ⚠ box
   when missing or unreadable; a press is the embed's: Live edits the line,
   Reading opens the file). The width decoded for is the column the
   pictures were last laid out in, rounded up to 100 px, so a 360 px
   column keeps a 3000×2000 photo as 2.1 MB, not 8.2 MB at the full 800
   px. After every message `want_pictures` looks up the embeds `view`
   builds (visible ± 900 px) and starts decoding new ones; a picture shown
   before it's been looked up asks once (`PicturesShown`). Decoding waits
   for the window's scale factor (`window::scale_factor`, `Rescaled`); a
   changed picture file is decoded again; a rescan retries failures. iced's
   `image-without-codecs` plus the `image` crate with PNG, JPEG, GIF, WebP
   and BMP only. Checked by hand: the test note (a 64 px PNG at its size, a
   4000×2250 JPEG fitted to the column, `|300` and `|200`, an EXIF-rotated
   JPEG standing upright, a missing one's ⚠ box), a click in Live showing
   `![[photo01.jpg]]` raw, Split's narrower preview, Reading, an outside
   edit to a photo redrawn. Measured: a note with twenty 3000×2000 photos
   opens at once and decodes the ten near the screen, about 65 ms each;
   scrolling through all of it and back held 49.7 MB in 25 pictures
   (process 147 MB); reopening it decoded nothing. Idle: 0 bytes written;
   CPU 0 ms in Source view, but with pictures on screen an occasional
   15.6 ms tick (one in about a minute). Dug into after milestone 10: not
   the pictures. Counting cycles per thread (`QueryThreadCycleTime`, finer
   than the 15.6 ms ticks process CPU time is charged in) shows the same
   with no picture ever shown: `notify`'s Windows watcher thread waits with
   a 100 ms timeout (`WaitForSingleObjectEx(wakeup_sem, 100, …)`, which is
   how it picks up watch and stop commands), so it wakes about 10 times a
   second, ~70,000 cycles (~25 µs) each, since milestone 6. Ticks only
   catch it now and then, so "0 ms idle" readings since then were short of
   that. Fixed after milestone 10 (see "Idle and bursts" below). Core
   tests: 100 (1 new).
3. **Crew portraits.** `portrait:` on a card is drawn on the crew hand, the
   graph and the card's note; bots without one keep their bot face.
   (`graph.rs`)
   ✅ **Done (November 2026).** `portrait_of` (main.rs) finds the file the
   way a link would: as the card gives it (relative to the card), else by
   its name anywhere in the vault; only pictures count. Portraits are
   decoded like note pictures but cropped to their middle square (a third
   of the way down a tall one, where faces are) at 96 px times the scale
   factor (`Key::portrait`, 0.2 MB each at 225%). The graph gets the
   decoded ones from `want_pictures` (`Graph::portraits` / `set_faces`,
   redrawing only when they change) and draws them with `draw_face`: a
   circle (a rounded square for a bot) on the hand's cards, whole or in a
   strip, on the card being dragged, and at the top right of a node card
   whose note is a crew card. A face is drawn in a clip of its own, since
   an image's corner radius rounds its clip; that clip keeps the frame's
   coordinates. Without a portrait, or when the file isn't there, the card
   keeps its initials (or bot square). A crew card's note shows the
   portrait, 96 px round, above its text in Live, Split and Reading; the
   page counts it in its sums (`Live::header`), so clicks still land on the
   right character. Checked by hand: Ika's PNG beside her card, Bob's
   `portrait: bob.jpg` found in `refs/` by name, Unit 7's bot square, Maya's
   missing file keeping her initials; the raised card, the dragged token and
   Ika's node card with their faces; Ika's note with the portrait and a
   click landing in "ani|mation"; a move to the 100% monitor and back
   re-decoding for the new scale. Idle on the graph with portraits: 0 ms
   CPU, 0 bytes written over 60 s.

*Budgets*, to be confirmed by measuring first: decoded pictures held in at
most 64 MB, a note with 20 pictures opening in under 100 ms after the first
time, idle CPU 0 ms. Note embeds (`![[other note]]`) stay out of version 1.
Measured in item 2: 49.7 MB held after scrolling a 20-photo note; opening it
doesn't wait for pictures, and the second time they're all held already;
idle CPU see item 2.

### Idle and bursts (after milestone 10)

✅ **Done (November 2026).**

- **The idle wakeup.** Counting CPU cycles per thread showed `notify`'s
  Windows watcher thread waking ten times a second (a 100 ms timeout on a
  wait nothing signals; that's how it picks up its own commands), with or
  without pictures. `notify` 9.0 fixed it (its Windows thread waits with no
  timeout and every command wakes it), so the app moved from 8.2 to 9.0, on
  every system alike. (In between, a Windows-only watcher of our own did the
  same; it went once 9.0 was found, so all systems share one path.) The
  watcher (`app/src/watch.rs`) lets a burst gather for 50 ms per batch, as
  each batch is a message and each message a redraw, and turns a "rescan"
  event (the system dropped changes) into a rescan of the vault. Checked on
  9.0: an outside edit to the open note, a new note, a rename, a delete and
  a changed picture all reach the app; the app's own save isn't logged as an
  outside change. Idle with photos on screen: no app thread runs in 60 s.
- **Bursts.** `files_changed` used to read every changed note at once on the
  UI thread. Now notes go in a queue (`OutsideQueue`); `drain_outside` takes
  a slice: notes the store holds are checked there (up to 4 ms), up to 64
  others are read one after another off the UI thread (`Message::OutsideRead`;
  a read can wait ~5 ms on the disk or a virus scan), then a 12 ms pause. The
  burst ends after 150 ms with nothing new: then it's logged and links, runs
  and the graph are refreshed once. Rescans start at most every 250 ms
  (`App::rescan`); a burst of creations or deletions used to start one per
  watcher batch. UI thread, in Mcycles per 100 ms (about 300 is one core):

  | 2,000 notes changed outside | Before | After |
  |---|---|---|
  | Created | peaks 380, ~40 steady, then the closing work per note | peaks 75, 15–45; 7.2 s |
  | Modified | each read blocked the UI thread (~5 ms each) | 15–55; 12 s |
  | Deleted | peaks 290, 74 rescans | peaks 126 for ~1.2 s, 12 rescans |

  `OBSESSION_DEV_TIMING` prints each burst (notes, slices, time) and each
  rescan's refresh. The startup link index already read notes one at a time
  on one background thread and is unchanged.

### Smooth wires (before milestone 11)

✅ **Done (November 2026).**

Wires in the graph and the function editor had stepped edges: the canvas
draws with MSAA off. Now `smooth::Shapes` builds each wire as a mesh whose
colour fades to clear across a one-pixel edge (lines, dashes with soft
ends, the arrowhead, the row dot, the ƒ dot and its ring), and hands it to
the canvas as its own geometry (`iced_wgpu::Geometry::Live`, cached like a
canvas `Cache`). The graph's cached layer is now three: grid and the wires'
words, the wires, then the cards; the overlay's wires (hovered, dragged,
being drawn, glowing after a run) go under the dragged card. Marching ants
now move smoothly along the line. The function editor does the same.

Panning a 300-card graph with 565 dashed wires by itself
(`OBSESSION_DEV_PAN=600`), 2282 × 1500 window at 225 %, Radeon 760M:

| | Frame (display, 40 Hz now) | Frame, no vsync | Cached layer built | GPU memory | Private bytes |
|---|---|---|---|---|---|
| Before (aliased) | 25.0 ms | — | 5.6 ms | 104 MB | 122 MB |
| iced 4× MSAA | 25.0 ms | 10.4 ms (p95 13.0) | 6.4 ms | 222 MB | 242 MB |
| Smooth meshes | 25.0 ms | 9.9 ms (p95 12.9) | 8.4 ms | 120 MB | 151 MB |

So about 100 frames a second while panning even this graph: the soft edges
stay on while panning. Idle is unchanged (no app thread runs).

### Milestone 11: Known limits

The rest of the earlier milestones' "Known limits, for later", in this order:

1. **The editor's rough edges.** Dragging a selection past the page's edge,
   selecting in Reading, Up/Down through blank lines, the scrollbar shifting.

   ✅ **Done (November 2026).** All in `app/src/live.rs`:

   - **Scrolling while selecting.** With a selection being dragged, the page
     scrolls when the pointer is within 24 px of its top or bottom edge or
     past it, faster the further in (2–60 px a step, a step every 16 ms);
     the selection's end follows the line at the pointer's height. The app
     listens to the pointer only while a selection is dragged
     (`Note::live_dragging`), and the step timer only runs at an edge
     (`Message::Pointer`, `Message::EdgeScroll`). On release the page stays
     where the drag ended: the editor isn't revealed back at the
     selection's first line.
   - **Selecting in Reading** (and Split's mirror): a drag selects; the
     selection stays highlighted until the next press or Escape, and Ctrl+C
     (when no text box has the keyboard, `Shortcut::Copy`) copies its text
     as drawn: lines joined with newlines, table cells with tabs, no
     markdown. Plain text shows the text cursor there too.
   - **Up/Down keep their column** (in characters) through blank and shorter
     lines, from the first of a run of them; any other key or a click starts
     afresh.
   - **Steadier heights.** Lines not drawn yet were estimated at half an em
     a character on an 800 px page; with the sidebar and backlinks open the
     page is much narrower, so a long note grew as it was scrolled. Now each
     line's words are wrapped at the page's real width with each
     character's real width (measured once per character), cached per line
     until it changes or the width does (a resize measures again). Table
     rows are measured too (their tallest cell) and estimated per cell.
     Sweeping a copy of this file (1,180 lines) in a 1000 × 640 window:

     | | Page height as lines get drawn |
     |---|---|
     | Before | grew from 31,800 to 45,600 px, jumping up to 2,100 px at tables |
     | After | stays between 53,168 and 53,240 px |

     `OBSESSION_DEV_TIMING` prints the estimates' error on each scroll;
     `OBSESSION_DEV_VSCROLL` takes a list of offsets.

   Known limits, for later: table cells don't show the selection highlight
   while dragging (they're still selected); copying a whole list line from
   Reading keeps its `•`.

2. **Links.** Folders can be renamed; relative links inside a moved note are
   adjusted; backlink snippets are drawn formatted; unlinked mentions are
   listed.

   ✅ **Done (November 2026).**

   - **Renaming a folder.** Right-click a folder in the file tree: the same
     box as renaming a note, its path ready to edit (`/` moves it, e.g.
     `anim` → `work/animation`). It refuses a folder inside itself or a name
     already taken. The folder moves on disk in one step, with everything in
     it, then the links are fixed and the vault re-read.
   - **One way to fix links after any move** (`links::rewrite_moved`, used by
     note and folder renames alike). Every note that might link to anything
     moved (by name, from the index) and every moved note with links is
     checked against the vault before and after the move: a link that still
     leads to the same note or attachment (where it went) is left alone;
     one that doesn't is rewritten, keeping `#sections`, aliases, labels and
     `!` embeds. A relative link (`../x`, or a path from the note's own
     folder) stays relative from where the note is now; others get the
     shortest unique form, as Obsidian does. So a note's own
     `[refs](../refs/a.md)` becomes `../../refs/a.md` when it moves a level
     down, links between notes that moved together stay as they are, and
     pictures in a moved folder are found again.
   - **What follows a move:** the store keeps the moved notes' text and pins
     (`Store::rename`), the link and search indexes move their entries
     without re-reading (`LinkIndex::rename`, `SearchIndex::rename`), graph
     views, open folders in the tree, the runs' pinned files, and the crew
     folder setting: renaming the crew folder rewrites `crew:` in
     `_settings.md` (creating it if needed). The watcher's events for the
     moved files count as the app's own, so nothing shows as "Changed in
     another app".
   - **Backlink snippets drawn formatted:** each linking line is drawn as Live
     Preview would (bold, italics, code, links, tags, `@names`, status
     symbols), worked out once when the panel refreshes.
   - **Unlinked mentions:** under the linked ones, lines in other notes that
     write the note's name as whole words (ASCII case ignored) outside
     links, code, comments, frontmatter, `#tags`, `@names` and web
     addresses, the name highlighted; a click opens the line. They need
     every note's text, which only search keeps in memory, so the panel
     offers "Find unlinked mentions" (building search if it isn't yet), and
     from then on looks for them on each note opened: 0.8 ms a note on a
     10,000-note vault (`obsession-bench links`). At most 200 lines shown.

   Checked by hand in a scratch vault: `anim` (3 notes, a picture, a
   subfolder) renamed to `work/animation` updated 4 links in 2 notes, all
   leading where they did; renaming the crew folder pointed `crew:` at it
   and `@names` stayed known; nothing was logged as an outside change; idle
   stayed at 0.

   Known limits, for later: renaming isn't undoable (as before for notes);
   unlinked mentions don't refresh while the same note stays open (open it
   again), have no "link it" button yet, and match names beyond ASCII only
   in the same case (item 3).
3. **Scale and search.** The function index and the out-of-date check move
   off the UI thread; search ignores case beyond ASCII (`É` matches `é`);
   find out why memory with search built grows by more than its text.

   ✅ **Done (November 2026).** Measured on a 10,000-note vault with 1,000
   function wires in 100 graphs (`obsession-bench gen`, then `graphs <dir>
   1000`; `runs` and `search` time the parts):

   | What | Before | After |
   |---|---|---|
   | Function index (vault open, a graph edited) | ~340 ms on the UI thread (2,000 notes read and pinned) | **1–6 ms** on it; 60–680 ms in the background |
   | Out-of-date check (after a run, an output changed) | 29 ms on the UI thread | **0 ms** on it; 25 ms in the background |
   | Function index at startup, link index read cold | waited for the link index (3–29 s) | lands in under a second |
   | Search: a query / unlinked mentions of a note | 1–5.6 ms / 0.5 ms, ASCII case only | the same, case beyond ASCII ignored |
   | Memory: search built / 300 hits shown | +10.4 MB / +8.6 MB | +9.1 MB / **+0.4 MB** |

   - **The function index in the background** (`App::rebuild_index` →
     `Message::IndexBuilt` → `index_built`). Graph texts the store holds go
     along (they may be ahead of the disk); everything else, the notes the
     jobs touch included, is read on that thread (`store::load`) and handed
     to the store as if it had read them (`Store::insert`, which never
     replaces what it holds), so pinning them reads nothing on the UI
     thread. Notes changed on disk meanwhile (`Runs::touched`) are read by
     the store itself. One build at a time; a graph changed meanwhile
     builds again when it lands. A run that comes due while the index is
     built waits for it, and the new index decides what it runs.
   - **The out-of-date check in the background** (`check_out_of_date` →
     `Message::Checked` → `checked`): one at a time, asked for again
     meanwhile it runs once more after; the graph is redrawn only if what's
     out of date changed. Each piece of work has a `runs::ticket`, so a
     result for an older request or another vault is dropped.
   - **Blocking work on threads of its own** (`timer::on_thread`). iced polls
     every task one `update` returns as one future, so a long job blocked
     the others batched with it: at startup the function index waited for
     the whole link index, and opening search held up the save batched with
     it. The vault scan and rescans, the link index, search, function runs,
     outside reads, saves and picture decoding now each run on their own
     thread. The vault is shared with them as an `Arc<Vault>`.
   - **Case beyond ASCII** (`search::fold`): every letter whose lowercase
     has the same UTF-8 length is lowered (`É` → `é`, `Σ` → `σ`, `Ж` → `ж`,
     a final `ς` → `σ`), so offsets still match the real line; the few
     others (`İ`, the Kelvin sign) match only as written. Used by search and
     unlinked mentions.
   - **Where search's memory went.** The "7 MB of text" was already two
     copies: 3.5 MB of notes and a lowercased copy with the same offsets,
     plus 1.8 MB of paths and lookups (8.8 MB, counted with a counting
     allocator in `obsession-bench search`). The copy stays: folding every
     note on each query instead saved 4 MB but made a query 6–10 ms and
     unlinked mentions 10× slower. The lists are shrunk after building
     (8.45 MB). The rest of the growth was the results list: all 300 rows
     were laid out, about 28 KB of text layout each. It's a virtual list
     now (only the rows on screen), every row one line high; Up and Down
     scroll the selected row into view, which they didn't before.

   Checked in a scratch instance (`--profile measure`, scratch `APPDATA`):
   the 1,000-wire vault opens with the function index landing first; an
   edit made in another app while the link index was still reading ran its
   wire and set the target task; the graph shows "Run 1 out of date" from
   the background check; search's results scroll with Down past the first
   screen without gaps; idle is 0 again once startup work is done.
   `OBSESSION_DEV_VSEARCH=ms;query[;downs]` drives search for memory checks.

   Known limits, for later: background work runs at normal priority (the
   "below normal" in CPU above isn't done yet); a search result's line is
   cut at the popup's edge instead of wrapping; reading 10,000 notes the
   first time after they were written can take tens of seconds on Windows
   (the virus scanner), which delays the link index and search, not the UI.
4. **The graph and crew, finished.** Cards matched to the Possess mockup;
   the inputs node's place saved; an `id` renamed from the editor;
   highlighting and drop targets past a card's first 8 rows; status formats
   on the Settings page; an unknown `@name` amber on the line being edited.

   ✅ **Done (November 2026).**

   - **Cards as in the Possess mockup** (`css/graph.css`, `crates/ui/src/
     graph.rs` there): 264 wide, 10 px corners and a soft shadow; a 54 px
     header with the department's colour as a strip, the title and the
     note's path in the mono font (⚠ at the right for unknown statuses, or a
     crew card's face); 30 px task rows with the status centred in a 22 px
     box, the text cut with "…", and the row's `@names` apart at the right
     (mono, amber); "No tasks" in italics; a 28 px footer with "in N · out
     M" (the wires drawn) and "K open of T", the done share as a thin green
     line along its top. A missing note's card is just its dashed header
     ("missing · double-click to create"). Wire ends, hover bands and drop
     targets follow the new rows.
   - **Past the eighth row.** A card longer than 8 tasks ends with "+N more":
     a click opens it to every row ("Show less" closes it; it stays open
     across rebuilds), and a crew card dragged onto "+N more" opens it after
     350 ms, so any row takes the drop. Hovering a crew member lights their
     rows, and a closed card's "+N more" when a hidden row is theirs. A wire
     to a hidden row ends at the card's edge until the row shows.
   - **An `id` renamed from the editor.** The selected step's id is a field
     beside its node's name; Enter renames it and every input that reads it
     (`func::edit::rename_step`), changing only those names on each line
     (spacing, `@x,y`, quoted text and comments stay). A name that isn't
     one, or is taken by another step, an `in`, an `out` or a param, is
     refused with a sentence.
   - **The inputs node's place saved.** It drags like a step, and its place
     goes at the end of the `fn` line (`fn Wait For Done  @-240,60`,
     Syntax.md section 8); renaming the function keeps it.
   - **Status formats on the Settings page.** A row per status (character,
     symbol, label; up, down, remove; "+ Add status", which starts from the
     built-in two), saved with the crew folder in one write as a `statuses:`
     list in `_settings.md` (`status::set_statuses`, replacing a block or
     flow list where it was; an empty list removes it). Refused with a
     sentence: two statuses with one character, a missing or spaced symbol.
   - **Unknown `@names` amber while editing.** The editor's highlighter marks
     an `@name` the link states call missing; each save re-checks the note's
     `@names` against the crew (`mention_states`), so a new one turns amber
     about a second after it's typed.

   Checked in scratch vaults (`--profile measure` with the mouse, and dev
   hooks `rows`, `edid`, `edentry`, `stadd` / `stset` for screenshots): a
   12-task card opens and closes; resting a dragged crew card on "+4 more"
   opened it and the drop wrote `@ika` on task 11; renaming `t` to
   `task_row` changed only Wait For Done's two lines; the inputs node
   dragged by mouse saved `@130,781`; Settings saved three statuses and the
   open note took them; idle stayed at 0.

## Risks

- **iced's editor is not Obsidian's.** Fine for notes and tasks. If it gets in
  the way, the preview stays the main reading view.
- **Large graphs.** Layered caches, culling and level of detail first, the
  `shader` widget if measurements say so.
- **GPU baseline memory on Windows.** Partly outside our control. Measure it in
  milestone 1 and set honest budgets from it.
- **Anti-aliasing cost.** MSAA can cost more memory than the rest of the app
  (measured: ~120 MB). Wires are smoothed with soft-edged meshes instead.
- **Several teammates running the app.** Every action is safe to run twice
  (Set Status, Replace Text, Ensure Line, Write Section), so two apps firing the
  same function after a `git pull` produce the same file.
- **Link resolution differences with Obsidian.** Test against a real Obsidian
  vault with duplicate file names.
- **A crash when a tool moves and resizes the window across monitors with
  different scaling in one step** (found November 2026, left as is). Windows
  then briefly reports a negative inner height (the window smaller than its
  own frame); winit 0.30.13 reads it as unsigned (−40 becomes 65,496),
  iced_winit passes it on (it only skips 0), and wgpu panics on a surface
  over 8,192. Dragging the window or Win+Shift+arrow between a 100% and a
  225% monitor is safe (tried five times); window-layout tools that set
  position and size together (e.g. FancyZones) can hit it. The fix, if it
  matters: a patched copy of iced_winit that treats a size of 0x8000 or more
  as 0, so the frame is skipped like a minimised window's.

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
- Clicking a task's last status keeps it there only in a format of the note's
  or the team's own. With the default `[ ]` / `[x]` a click toggles, as a
  checkbox in Obsidian does (`Format::click`; November 2026, after a checked
  task in a plain note couldn't be unchecked by clicking).
- Windows first.
