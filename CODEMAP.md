# Code map

A one-page guide to the code: what each file holds, its main types and
functions, and how a click, a save or a graph change travels through them.
Read this first; open only the files a change needs.

- **What the app does and how files look:** [README.md](README.md),
  [Syntax.md](Syntax.md).
- **Why it's built this way, measurements, milestones:** [PLAN.md](PLAN.md).
- **Keep this file current:** when a file, a main type or a flow changes,
  update its line here in the same commit.

Line counts are approximate (November 2026).

## Packages

| Package | Folder | What |
|---|---|---|
| `obsession-core` | `core/` | Everything that isn't UI. No iced. Fully unit-tested (`cargo test -p obsession-core`). |
| `obsession` | `app/` | The iced 0.14 app (wgpu, canvas). |
| `obsession-bench` | `bench/` | Test vault generator and timings (`obsession-bench gen <dir> <count>`). |

Build and run: `cargo run --release`. Measure: `cargo build -p obsession
--profile measure` (release speed, debug hooks kept). Cargo's home and target
live in `C:\dev\cargo` and `C:\dev\target`.

## core/src

| File | Lines | Holds |
|---|---|---|
| `lib.rs` | 30 | Module list. |
| `vault.rs` | 470 | `Vault` (flat sorted `Vec<Entry>`; `scan`, `note(rel)`, `path(rel)`, `notes()`), Obsidian link resolution `resolve` / `resolve_from` → `Resolved::{One, Ambiguous, NotFound, Attachment}` (`.entry()`), `note_rel_from_input`, `create_note`. |
| `store.rs` | 420 | `Store`: one RAM copy per file. `get(rel)` reads once, `pin`/`unpin`, `set(rel, text)` (only for files already loaded), `take_save(rel)` → `SaveJob` (`run()` off-thread) → `finish_save`. `Stats` for the status bar. |
| `write.rs` | 75 | `atomic_write`: temp file, then swap; retries locked files. |
| `doc.rs` | 150 | `Doc`: a note as lines, each with its own ending; byte-exact round trip. |
| `history.rs` | 150 | `History`: undo/redo snapshots, grouped, capped at 8 MB. |
| `syntax.rs` | 380 | Line tokenizer for highlighting: `scan_line(LineState, line)`, `LineState::{Start, Normal, Frontmatter, Fence, Comment}`, `can_hold_task`. Used by everything that must skip code and frontmatter. |
| `status.rs` | 440 | Tasks and status formats: `task_box(line)`, `tasks(text)` → `(line, byte of [, char)`, `Format` (`builtin`, `index_of`, `get`, `next`, `is_finished`), `from_frontmatter`, `resolve(note, team)`. |
| `live.rs` | 910 | Live Preview model: `blocks(lines)`, `line(raw, format)` → drawn `Line` of `Seg`s with raw-byte mapping, `table_cells`, `trailing_block_id`. |
| `links.rs` | 620 | `links(text)`, `outline(text)` (headings, `^ids`), `LinkIndex` (outgoing links per note; `build`, `update`, `remove`, `targets`, `backlinks`), `link_states`, `rewrite_links` for renames. |
| `preview.rs` | 430 | Obsidian → plain markdown (unused by the app now; kept for export). |
| `fuzzy.rs` | 70 | `score(query, candidate)` for the quick switcher. |
| `nodes.rs` | 600 | `_nodes.md`: `parse(text)` → `Graph { nodes, wires, layout }`, `Node`/`NodeKind::{File, Function}`, `Wire { from, to, function, line }`, `End::{Note{target, sub}, Pin{function, pin}}`. Edits: `set_layout`, `add_item`, `remove_lines`, `add_block`, `set_wire_function`, `section_lines`. |
| `func/mod.rs` | 640 | Function text format: `parse`, `blocks(text)` / `local_blocks(graph)` → `Block { open, close, function }`, `Function { name, inputs, outputs, params, steps, errors }`, `Step { id, op, args, pos, line }`, `Value::{Ref, Lit}`, `Literal`, `parse_call`, `parse_value`, `write_step`. `Library { local, saved }` (`find`, `all`), `SAVED` = `_funct.md`, `SAVED_DEFAULT`. |
| `func/types.rs` | 140 | `Type` (File, Section, Task, Text, Number, Bool, TextList, TaskList) and `Want` (what an argument accepts; `accepts`, `describe`). |
| `func/library.rs` | 210 | `NODES`: every node (`Spec { name, category, inputs, output, help }`), `find(name)`. Add a node here *and* in `eval.rs`'s `apply`. |
| `func/check.rs` | 380 | `check(f)` → `Checked { types, bound, order, problems }` (`ok()`, `first_problem`, `source(name)`), `bind(spec, args)`, `check_call` (a wire's param values), `port_end`. |
| `func/eval.rs` | 820 | `eval(f, checked, binds, params, team)` → `Run { values, patches }`. `Bind { rel, text, sub }` per port, `Val`, `Patch { port, rel, line, old, new }`, `apply_patches`, `section_range`, `Val::show` (hover text). |
| `func/edit.rs` | 330 | Editor changes as one-line edits: `set_pos`, `set_arg`, `args_for`, `add_step`, `remove_step`, `add_header`, `remove_line`, `fresh_id`, `wire_call`. |

## app/src

| File | Lines | Holds |
|---|---|---|
| `main.rs` | 2100 | `App` (state), `Message`, `Workspace { vault, store, tree, note, team, links, … }`. `update` → `handle`; `view`, `view_toolbar`, `view_status`; `subscription` (keys, window). See the flows below for which methods do what. |
| `note.rs` | 1050 | `Note`: the open note in a `Mode` (Live, Source, Split, Reading), its format, history and autosave `Timer`s. `text()`, `apply_text(text)` (undoable replace; returns timers), `perform`, `jump_to`, `set_mode`. |
| `live.rs` | 1350 | Live Preview widget state: drawn blocks plus the active block's small editor; caret, selection by dragging, keys. |
| `live_text.rs` | 350 | One drawn line widget that reports which byte a press hit. |
| `highlight.rs` | 200 | iced highlighter over `core::syntax`; status and link colours. |
| `source_scroll.rs` | 230 | Tracks the Source editor's scroll for Split mode. |
| `right_click.rs` | 235 | Wrapper reporting where a right-click landed. |
| `status_menu.rs` | 85 | Right-click status menu. |
| `sidebar.rs` | 160 | File tree as a virtual list (`Tree`). |
| `switcher.rs` | 135 | Quick switcher (Ctrl+P). |
| `new_note.rs` | 105 | New-note box (Ctrl+N), also the rename box (F2). |
| `backlinks.rs` | 70 | Backlinks panel. |
| `graph.rs` | 1510 | Graph view (`canvas::Program`): `Graph::build(text, rel, vault, store, team, links, library, previous, show_links)` → `cards: Vec<Card>`, `wires: Vec<WireView>` with `WireState::{Plain, Solid, Broken(why)}`. `Message::{Moved, Open, Connect(PinRef, PinRef), Delete, Function(wire)}`. Shared drawing helpers: `View`, `curve`, `draw_grid`, `fit`, `pin_color`. |
| `func_editor.rs` | 1480 | Function editor (`Editor`: canvas + panel), `Message` → `Outcome` (`Edit(closure)`, `Wire(call)`, `Back`, `Change`, `TakeOff`, `Duplicate`, `Status`), `Context::{Wire{line}, Node}`. Also `Picker` (choose or create a function). |
| `theme.rs` | 230 | Colours, fonts, widget styles. |
| `timer.rs` | 20 | One-shot delay future. |
| `settings.rs` | 30 | Last vault path in `%APPDATA%\Obsession`. |
| `dev.rs` | 350 | Debug-only hooks driven by `OBSESSION_DEV_*` variables (open a note, type, click, run graph/editor steps, screenshot). See its header for the list. |

## Flows

**Opening a vault.** `App::scan` → `Vault::scan` off-thread → `Message::Scanned`
→ `Workspace` created, `Tree` built, `LinkIndex::build` off-thread →
`Message::Indexed` → `refresh_links`, `rebuild_graph`.

**Opening a note.** `App::open(rel)`: `save_now` the old one, `store.pin` +
`store.get`, `Note::new`. Closes the function editor and picker. If the note is
a `_nodes.md`, `rebuild_graph`.

**Typing and saving.** Editor actions → `Note::perform` (history, timers) →
`Timer::Save` fires after 1 s → `save_now`: `store.set` + `LinkIndex::update` +
`store.take_save` → `SaveJob::run` off-thread → `Message::Saved` →
`store.finish_save`.

**Clicking a status.** Live: `live::Message` → `Note::perform` with the next
status from `Format::next`. Right-click: `EditorRightClick` → `StatusMenu`.

**Renaming.** `App::rename`: move the file, rewrite links in every note that
links to it (`links::rewrite_links`, saved through the store), rescan.

**The graph.** `rebuild_graph`: `load_library` (graph's `## Functions` +
`_funct.md`) → `Graph::build` (cards read notes through the store; wire states
checked against the library; `previous` graph for the solid animation) →
`refresh_editor`. Canvas events → `graph::Message` → `graph_message`, which
changes the `_nodes.md` text through `edit_graph(change, rebuild)`
(`Note::apply_text`: undoable, autosaved).

**Functions.** Wire ƒ → `graph::Message::Function` → `open_editor` (the wire
has a known function) or `open_picker` (it doesn't; creates `_funct.md` first
via `ensure_saved_functions`). Picker → `picker_message` → `attach` (set the
wire's function, maybe add a block) → `open_editor`. Editor events →
`editor_message` → `Outcome::Edit` → `edit_function`: the open `_nodes.md`
through `edit_graph`, or `_funct.md` through the store and a save; then
`rebuild_graph` → `refresh_editor`, which finds the block again, gathers the
wire's values and the wired notes (`Bind`s), and calls `Editor::refresh`
(check + eval → live values).

## Conventions

- Text first: the app changes files with small line edits and never rewrites
  what it doesn't own. Core functions take `&str` and return the new `String`.
- Everything a user sees as ⚠ is a sentence saying what's wrong and what to do.
- Draw only when something changes; canvases cache their still layer and keep
  hover, drag and animation in an overlay. Idle CPU is measured at 0.
- Tests for anything in core; the app is checked with the `dev.rs` hooks and
  screenshots.
