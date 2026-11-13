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
| UI | **iced** (pin the latest release when scaffolding) | Native, GPU-drawn, Elm-style state, built-in animation helpers, `Canvas` with dashed strokes, `text_editor` with highlighting |
| Markdown rendering | **pulldown-cmark** | The parser iced's own markdown widget uses. Handles tables, footnotes, task lists, strikethrough |
| Task, link and status parsing | **Our own line parser** | The syntax is line-based. Owning it lets the app patch single lines safely |
| Frontmatter | A read-only YAML reader (`yaml-rust2` or `saphyr`) | The app reads frontmatter but never rewrites it |
| File watching | **notify** with its debouncer | Picks up edits from Obsidian, VS Code and `git pull` |
| Folder picker | **rfd** | Native Windows dialog |
| Errors | `thiserror` in core, `anyhow` in the app | Standard and small |
| Async | iced's default thread-pool executor, **no tokio** | Fewer threads, smaller binary |
| Profiling | `tracing` + Tracy, behind a `profile` feature | Zero cost in normal builds |

Every dependency is added with `default-features = false` and only the
features we use. iced gets only what we draw with: `wgpu`, `canvas`,
`markdown`, `highlighter`, `image`, `lazy`.

## Layout

```
obsession/
  Cargo.toml            workspace
  bench/                package "obsession-bench": test vault generator + timings
  core/                 package "obsession-core": no UI, fully testable
    src/
      vault.rs          scan folders, index files, watch for changes
      doc.rs            a file's original text + line index + line patches
      parse/
        task.rs         - [c] text @people ^id
        status.rs       status formats (frontmatter + _settings.md)
        link.rs         [[links]], shortest-name resolution, renames
        nodes.rs        _nodes.md: node list, wires, functions, layout
        func.rs         obsession code blocks (the function text format)
      func/
        types.rs        Text, List, Bool, Number, Task
        library.rs      every node in the function library
        check.rs        type checking: decides dashed vs solid
        eval.rs         runs functions in memory, collects patches
      crew.rs           _crew/*.md, mention counts, badges
      runner.rs         throttle (1 run/s), one pending run, loop guard
      log.rs            undo log in .obsessed/
      write.rs          atomic writes, retry on locked files
  app/                  package "obsession": the iced app
    src/
      main.rs
      theme.rs          colors, spacing, animation timings
      sidebar.rs        file tree, search, quick switcher
      editor.rs         raw editor + preview, status chips
      graph/
        canvas.rs       pan, zoom, node cards, wires, hit testing
        wire.rs         dashed/solid drawing, hover "ƒ" button
        func_editor.rs  inner canvas for a function (reuses canvas.rs)
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
| Frame time while animating or dragging | **≤ 8.3 ms**, p99. That's enough for 120 fps, which leaves room to spare at 60 fps |
| Graph with 500 nodes and 1,000 wires | No dropped frames while panning and zooming |
| Background work (indexing, checks) | Below-normal thread priority, at most 2 threads |
| Memory, empty vault | Target < 60 MB working set |
| Memory, 10,000-file vault | Target < 120 MB working set |
| First window on screen | < 300 ms |
| Indexing 10,000 files | < 2 s, in the background, UI usable meanwhile |
| Release binary | < 20 MB |

> On Windows, any GPU-drawn app pays a baseline of roughly 40 MB or more for
> the graphics driver and swap chain before drawing anything. Milestone 1
> measures our real baseline, and the memory budgets get adjusted to it once.
> After that they only go down.

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
- **A small cache of recent files** (budget about 16 MB) for function inputs.
  Least-recently-used files are dropped first.
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

**Reads**

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
- A milestone isn't done until its budgets pass.

## Milestones

Each milestone ends with something you can run and use, and with the
performance budgets checked.

| # | Milestone | Done when |
|---|---|---|
| 1 | **Vault and editor** | Open a folder, file tree, raw editor with highlighting, preview, autosave, theme, quick switcher |
| 2 | **Tasks and statuses** | Status chips in editor and preview, click to advance, right-click menu, status formats, ⚠ for unknown characters |
| 3 | **Links** | Shortest-name resolution matching Obsidian, ⚠ on ambiguous links, backlinks panel, renaming a file updates links |
| 4 | **Graph** | Read and write `_nodes.md`, node cards (like the Possess mockup), pan, zoom, drag, layout saved, dashed wires from `_nodes.md` and from `[[links]]` |
| 5 | **Function editor** | Hover a wire, "ƒ" button, inner canvas, the full node library, type checking, dashed-to-solid animation, live values on hover, `_funct.md` saved functions |
| 6 | **Running functions** | File watcher, throttled runs, chains, loop guard, batched atomic writes, undo log in `.obsessed/`, outside edits |
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
- Graph files are `_nodes.md`, one per folder.
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
