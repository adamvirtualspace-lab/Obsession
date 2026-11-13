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
- **Light, fast and good-looking.** One native binary, smooth UI animations,
  careful with CPU and disk.
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

## Layout

```
obsession/
  Cargo.toml            workspace
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

## Milestones

Each milestone ends with something you can run and use.

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
- **Large graphs on a CPU-drawn fallback.** Cache canvas geometry, redraw only
  what moved.
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
