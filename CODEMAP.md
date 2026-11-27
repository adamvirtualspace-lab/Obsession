# Code map

A one-page guide to the code: what each file holds, its main types and
functions, and how a click, a save or a graph change travels through them.
Read this first; open only the files a change needs.

- **What the app does and how files look:** [README.md](README.md),
  [Syntax.md](Syntax.md).
- **Why it's built this way, measurements, milestones:** [PLAN.md](PLAN.md).
  What comes next (milestones 9 and 10) is in its "Next" section.
- **Keep this file current:** when a file, a main type or a flow changes,
  update its line here in the same commit.

Line counts are approximate (November 2026).

## Packages

| Package | Folder | What |
|---|---|---|
| `obsession-core` | `core/` | Everything that isn't UI. No iced. Fully unit-tested (`cargo test -p obsession-core`). |
| `obsession` | `app/` | The iced 0.14 app (wgpu, canvas). |
| `obsession-bench` | `bench/` | Test vault generator and timings (`obsession-bench gen <dir> <count>`). `bench/scripts/`: `idle-threads.ps1` (each thread's CPU cycles over a window: idle should show none) and `ui-load.ps1` (the UI thread's load every 100 ms during a burst). |

Build and run: `cargo run --release`. Measure: `cargo build -p obsession
--profile measure` (release speed, debug hooks kept). Cargo's home and target
live in `C:\dev\cargo` and `C:\dev\target`.

## core/src

| File | Lines | Holds |
|---|---|---|
| `lib.rs` | 30 | Module list. |
| `vault.rs` | 628 | `Vault` (flat sorted `Vec<Entry>`; `scan`, `note(rel)`, `path(rel)`, `notes()`; attachments apart in `files` with their own lookups, `with_files`), Obsidian link resolution `resolve` / `resolve_from` (steps shared by notes and attachments in `find`) → `Resolved::{One, Ambiguous, NotFound, Attachment(Option<rel>)}` (`.entry()`), `attachment(target, from)`, `is_attachment`, `note_rel_from_input`, `folder_rel_from_input`, `create_note`. Moves: `moved(rel, from, to)` (where a path is after a note or folder moves), `folder`, `notes_in`, `files_in`, `count_files_named`. |
| `store.rs` | 546 | `Store`: one RAM copy per file. `get(rel)` reads once, `pin`/`unpin`, `set(rel, text)` (only for files already loaded), `take_save(rel)` → `SaveJob` (`run()` off-thread) → `finish_save`. `check_outside(rel)` → `Outside::{Same, Changed, Conflict, Deleted, NotLoaded}` (own writes recognised by hash). `rename(old, new)` after a move keeps text and pins. `Stats` for the status bar. |
| `write.rs` | 75 | `atomic_write`: temp file, then swap; retries locked files. |
| `doc.rs` | 150 | `Doc`: a note as lines, each with its own ending; byte-exact round trip. |
| `history.rs` | 316 | `History`: undo/redo snapshots, grouped, capped at 8 MB. `Elsewhere` + `Step { files: Vec<Change>, open, what, joined }` + `fingerprint`: undo for changes made from the open note's view to other notes (one step can hold several files), taken only while the open note's text matches its fingerprint from then; capped at 500 steps and 8 MB (`Step::bytes`). A `joined` step goes with the open note's own edit in one press (`undo_joined` after its undo; redone just before its redo). |
| `syntax.rs` | 380 | Line tokenizer for highlighting: `scan_line(LineState, line)`, `LineState::{Start, Normal, Frontmatter, Fence, Comment}`, `can_hold_task`. Used by everything that must skip code and frontmatter. |
| `status.rs` | 440 | Tasks and status formats: `task_box(line)`, `tasks(text)` → `(line, byte of [, char)`, `Format` (`builtin`, `index_of`, `get`, `next`, `is_finished`), `from_frontmatter`, `resolve(note, team)`. |
| `live.rs` | 1023 | Live Preview model: `blocks(lines)`, `line(raw, format)` → drawn `Line` of `Seg`s (and `pictures`: `Picture { seg, target, width }`, `is_picture`) with raw-byte mapping, `table_cells`, `trailing_block_id`. |
| `links.rs` | 1237 | `links(text)`, `outline(text)` (headings, `^ids`), `LinkIndex` (outgoing links and task `@mentions` per note; `build`, `build_cached` + `to_cache` + `save_cache` (`.obsessed/cache/links.txt`), `stamp`, `update`, `remove`, `targets`, `backlinks`, `mention_counts`), `linking_names`, `has_links`, `rename`), `link_states`, `rewrite_moved` (fixes links after a note or folder moves, checked against the vault before and after; `relative_path`), `unlinked_mentions(text, lower, name)`, `remove_links(text, pick)` + `link_words` (unlinking keeps the words), `task_at(text, sub, task_lines)` (which task a `#^block` / `#heading` link points at, for wires that end at a row). |
| `preview.rs` | 430 | Obsidian → plain markdown (unused by the app now; kept for export). |
| `fuzzy.rs` | 70 | `score(query, candidate)` for the quick switcher. |
| `search.rs` | 312 | Vault search: `SearchIndex::build(root, notes)`, `update`, `remove`, `rename`, `query(words, limit)` → `Results { hits: Vec<Hit { rel, line, text, ranges }>, total, notes }`; `unlinked(rel, limit)` (a note's unlinked mentions). |
| `nodes.rs` | 727 | Graph files (any `…_nodes.md`, `is_graph(rel)`): `parse(text)` → `Graph { nodes, wires, layout }`, `Node`/`NodeKind::{File, Function}`, `Wire { from, to, function, line }`, `End::{Note{target, sub}, Pin{function, pin}}`. Edits: `set_layout`, `add_item`, `remove_lines`, `add_block`, `set_wire_function`, `section_lines`, `rename_function(text, old, new)` (`- fn Old`, wire calls, `Old.pin` ends, the layout line; `pin_function`). |
| `func/mod.rs` | 640 | Function text format: `parse`, `blocks(text)` / `local_blocks(graph)` → `Block { open, close, function }`, `Function { name, inputs, outputs, params, steps, errors }`, `Step { id, op, args, pos, line }`, `Value::{Ref, Lit}`, `Literal`, `parse_call`, `parse_value`, `write_step`. `Library { local, saved }` (`find`, `all`), `SAVED` = `_funct.md`, `SAVED_DEFAULT`. |
| `func/types.rs` | 140 | `Type` (File, Section, Task, Text, Number, Bool, TextList, TaskList) and `Want` (what an argument accepts; `accepts`, `describe`). |
| `func/library.rs` | 210 | `NODES`: every node (`Spec { name, category, inputs, output, help }`), `find(name)`. Add a node here *and* in `eval.rs`'s `apply`. |
| `func/check.rs` | 380 | `check(f)` → `Checked { types, bound, order, problems }` (`ok()`, `first_problem`, `source(name)`), `bind(spec, args)`, `check_call` (a wire's param values), `port_end`. |
| `func/eval.rs` | 820 | `eval(f, checked, binds, params, team)` → `Run { values, patches }`. `Bind { rel, text, sub }` per port, `Val`, `Patch { port, rel, line, old, new }`, `apply_patches`, `section_range`, `Val::show` (hover text). |
| `runner.rs` | 560 | `Index::build(vault, graphs, saved)` → `jobs: Vec<Job>` (`Job { graph, line, function, checked, params, ports }`), `affected(changed)`, `files()`, `reads(rel)`, `find(graph, line)`. `run(index, files, start, team)` → `Outcome { texts, changes, ran, blocked, warnings }` (chains, loop guard), `pending` (out of date), `apply` / `revert` (`Change { rel, line, old, new, by }`), `params_from`, `end_kind`. |
| `crew.rs` | 510 | `Member { key, rel, name, role, portrait, color, skills, bot, exists }`, `card(rel, text)`, `is_card(rel, folder)`, `seats(folder, entries)` (notes and people's folders), `new_card_rel`, `key_of`, `folder_setting` / `frontmatter_value` / `set_frontmatter` (`crew:` in `_settings.md`), `template`, `mentions(line)`, `task_mentions(text)`, `add_mention(line, name)`, `badges(counts)` → `Badge::{Overloaded, Powerhouse}`, `initials`. |
| `log.rs` | 250 | `.obsessed/log/`: `Record { id, time, kind: Kind::{Run, Outside}, summary, changes, undone }`, `append` (keeps 500), `load`, `mark_undone`, `ensure` (folder + `.gitignore`). |
| `views.rs` | 263 | `.obsessed/views.txt`: `Views` (`parse`, `to_text`, `get`, `set` (marks dirty only on a real change), `prune`, `rename`, `move_view` (a renamed function's view), `load`, `save` (only if dirty)), `View { x, y, zoom }` (the graph point in the middle of the canvas), `graph_key(rel)`, `function_key(file, name)`. |
| `func/edit.rs` | 337 | Editor changes as one-line edits: `set_pos`, `set_arg`, `args_for`, `add_step`, `remove_step`, `add_header`, `remove_line`, `fresh_id`, `wire_call`, `rename(text, block, new, heading)` (the `fn` line, and the `## Name` heading in `_funct.md`). |

## app/src

| File | Lines | Holds |
|---|---|---|
| `main.rs` | 4468 | `App` (state), `Message`, `Workspace { vault, store, tree, note, team, links, … }`. `update` → `handle`; `view`, `view_toolbar` (the same for notes and graphs), `view_graph_bar` (the graph's own tools, an `opaque` bar over the canvas), `view_status`; `subscription` (keys, window); `shortcut_from_event` → `Shortcut` (Escape always arrives, even from a focused text box; Tab only when no widget used it). `view` always returns a `stack!` (one layer when no popup is open) and the page is always a `stack!` too, so widgets keep their focus and scroll when a popup or the `@name` suggestions appear. See the flows below for which methods do what. |
| `note.rs` | 1163 | `Note`: the open note in a `Mode` (Live, Source, Split, Reading), its format, history and autosave `Timer`s. `text()`, `apply_text(text)` (undoable replace; returns timers), `perform`, `jump_to`, `set_mode` (a new `Live` or Split mirror gets the note's link states, so unknown `@names` stay amber). `@` autocomplete: `mention_prefix`, `set_completing`, `complete_mention`; the Source editor sends `SourceEvent::{Action, Complete}`. Selecting in the Live view on screen (the note's or Split's mirror): `live_dragging` (the app then subscribes to the pointer), `selection_text` (Reading's Ctrl+C, `Shortcut::Copy`), `clear_selection` (Escape); `LiveOutcome::edge_step` asks for the next `live::Message::EdgeScroll`. |
| `live.rs` | 1838 | Live Preview widget state: drawn blocks plus the active block's small editor; caret, selection by dragging, keys (`goal`: the column Up/Down aim for). Selecting: `press` → `drag_to` / `drag_head` → `release` (editable: `select` opens the covered blocks; read-only: `kept`, `selection_text`, `clear_selection`); at the page's edges `pointer` → `edge_scroll` (`EDGE`, `EDGE_STEP`). Heights: measured (`heights`) or estimated (`estimates`, `estimate` → `wrapped_rows` with `advance`, at `text_column`; table rows measured by their tallest cell), cleared on a width change in `scrolled`. Pictures: `picture_column` (a `PictureColumn` under the line, or alone for a line of only pictures), `picture_heights` in `line_height`, `pictures_near` (what `view` builds), `Message::PicturesShown`. |
| `pictures.rs` | 575 | Pictures in notes: `Pictures` (decoded by `Key { rel, width }`, 64 MB `BUDGET`, `want` / `loaded` / `trim` / `forget`, the open note's targets, `decode_width`, scale factor), `decode` (off-thread, two at a time, EXIF, shrunk to the width drawn), `PictureColumn` widget. |
| `live_text.rs` | 348 | One drawn line widget that reports which byte a press hit; `measure` (its height into a cell) or `measure_tallest` (a table row's cells share one). |
| `highlight.rs` | 200 | iced highlighter over `core::syntax`; status and link colours. |
| `source_scroll.rs` | 230 | Tracks the Source editor's scroll for Split mode. |
| `right_click.rs` | 235 | Wrapper reporting where a right-click landed. |
| `status_menu.rs` | 85 | Right-click status menu. |
| `sidebar.rs` | 170 | File tree as a virtual list (`Tree`; `rename` keeps moved folders open). Right-click a note or folder: `Message::Rename`. |
| `switcher.rs` | 135 | Quick switcher (Ctrl+P). |
| `find.rs` | 150 | Vault search popup (Ctrl+Shift+F): `Find` (query, results, selection), `Message::{Query, Submit, Pick, Close}`. |
| `new_note.rs` | 125 | New-note box (Ctrl+N), also the rename box for a note (F2) or a folder (`Purpose::{New, Rename, RenameFolder}`). |
| `backlinks.rs` | 249 | Backlinks panel: `Panel` (`set_links` draws each snippet with `live::line`; unlinked mentions once `want_unlinked`, `unlinked_stale`), `Message::{Open, FindUnlinked}`. |
| `graph.rs` | 2394 | Graph view (`canvas::Program`): `Graph::build(text, rel, vault, store, team, links, library, previous, show_links)` → `cards: Vec<Card>`, `wires: Vec<WireView>` with `WireState::{Plain, Solid, Broken(why)}`. `Message::{Moved, Open, Connect(PinRef, PinRef), Delete, Function(wire), Assign{member, card, task}, OpenCrew}`. The crew hand: `HandCard`, `set_crew(&[Member])`, `show_hand`. `draw_crew` lights the person's nodes in the overlay and leaves `hand_now` (`HandNow`: size, raised card and lift, the dragged token); `HandLayer`, a draw-only canvas stacked over the graph in `Graph::view` (its own layer, so graph text can't show through), draws it with `draw_hand` → `draw_hand_card` (whole) or `draw_hand_strip` (face, name and badge in the visible strip of a covered card; `hand_step`, `hand_visible`, `badge_short`). `takes_function(wire)`: no ƒ on wires to or from a function node's pin. `WireView::from_row` / `to_row` (via `row_in` → `links::task_at`): a `#^block` / `#heading` end meets that task row (`row_y`); `wire_ends` uses it. Hover is worked out again when a drag ends. Drawing: the cached layer is `cache` (grid, wire words), `wire_mesh` (the wires as smooth meshes) and `cards_cache`, rebuilt when `DrawKey` changes; the overlay's wires are smooth too. `draw_wire(frame, shapes, …)`. `pan_bench` (`OBSESSION_DEV_PAN`). Shared drawing helpers: `View`, `curve_screen`, `draw_grid`, `fit`, `pin_color`. |
| `smooth.rs` | 376 | Smooth lines without MSAA: `Shapes` (`line`, `dashed`, `ring`, `fill` convex, `circle`, `circle_ring`) builds a mesh with a soft edge one screen pixel wide; `into_geometry(size)` → an `iced_wgpu::Geometry` for a canvas. `bezier` (points along a curve), `set_scale` / `scale` (the window's scale factor, from `Message::ScaleFactor`). |
| `func_editor.rs` | 1573 | Function editor (`Editor`: canvas + panel; the title is a `text_input` on `name_draft`, `NameDraft`/`NameSubmit`), `Message` → `Outcome` (`Edit(closure)`, `Wire(call)`, `Back`, `Change`, `TakeOff`, `Duplicate`, `Status`, `Rename(new)`), `Context::{Wire{line}, Node}`. Also `Picker` (choose or create a function). Drawing: `cache` (grid, inputs node), `wire_mesh` (smooth wires, `draw_wires_into(shapes, …)`), `steps_cache`. |
| `watch.rs` | 81 | The vault watcher (`notify` 9, every system): `watch(root)` subscription → `Vec<FsEvent>` (batches gathered for 50 ms, dot folders left out, a "rescan" event as the vault folder). |
| `runs.rs` | 207 | `Runs` (per vault: `index`, `pinned`, `pending`, `running`, `scheduled`, `last`, `out_of_date`, `log`, `show`), `FsEvent`, `rel_of`, the Runs panel (`view`, `Message::{Undo, Close}`). |
| `theme.rs` | 260 | Colours, fonts, widget styles. Compact layout: `set_compact`, `compact`, `row_height`, `tool_pad`, `tool_gap`, `bar_pad`. |
| `timer.rs` | 20 | One-shot delay future. |
| `settings.rs` | 60 | `%APPDATA%\Obsession`: the last vault, and `Prefs { compact }` (`prefs`, `save_prefs`). |
| `dev.rs` | 391 | Debug-only hooks driven by `OBSESSION_DEV_*` variables (open a note, type, click, run graph/editor steps, screenshot; `OBSESSION_DEV_KEYS` logs key presses; `OBSESSION_DEV_PAN` / `OBSESSION_DEV_MSAA` measure panning). See its header for the list. |

## Flows

**Opening a vault.** `App::scan` → `Vault::scan` off-thread → `Message::Scanned`
→ `Workspace` created, `Tree` built, `LinkIndex::build` off-thread →
`Message::Indexed` → `refresh_links`, `rebuild_graph`.

**Opening a note.** `App::open(rel)`: `save_now` the old one, `store.pin` +
`store.get`, `Note::new`. Closes the function editor and picker. If the note is
a `_nodes.md`, `rebuild_graph`.

**Pictures.** `App::update` → `want_pictures` after every message:
`Note::pictures_near` (the Live view on screen; nothing in Source, a graph,
or before the scale factor is known) → `Vault::attachment` for each target
→ `Pictures::set_targets` + `want` → a `pictures::decode` task per new one →
`Message::PictureLoaded` → `Pictures::loaded` (trims to the budget).
`Live::view(statuses, pictures)` draws from `Pictures::shown`. The scale
factor: `window::scale_factor` at boot, `Rescaled` events →
`Message::ScaleFactor`. `files_changed` forgets a changed picture;
`VaultRefreshed` retries failed ones; a new vault clears them.

**Portraits.** `portrait_of(vault, member)` resolves `portrait:`.
`rebuild_graph` → `Graph::set_crew(crew, portraits)` (each hand card's file,
and `card_faces` for crew card notes as nodes). `want_pictures` also wants
`Key::portrait` for `Graph::portraits()` (when the graph shows) and for the
open crew card (`Pictures::set_header`), then `Graph::set_faces` with the
decoded ones. Drawn by `draw_face` (hand, dragged token, `draw_card`) and,
for the card note, at the top of `Live::view` (`Pictures::header`,
`Live::header` in the page sums).

**Typing and saving.** Editor actions → `Note::perform` (history, timers) →
`Timer::Save` fires after 1 s → `save_now`: `store.set` + `LinkIndex::update` +
`store.take_save` → `SaveJob::run` off-thread → `Message::Saved` →
`store.finish_save`.

**Clicking a status.** Live: `live::Message` → `Note::perform` with the next
status from `Format::next`. Right-click: `EditorRightClick` → `StatusMenu`.

**Renaming.** `App::rename` (a note, F2 or right-click) or `rename_folder`
(right-click a folder) → `move_path(from, to, folder)`: save the open note,
gather the notes that may link to what moves (`LinkIndex::linking_names`)
and the moved notes with links, move it on disk, `Vault::scan`, move what's
held under the old paths (store, link and search indexes, views, tree, runs'
pins, the open note), rewrite each source with `links::rewrite_moved`
(before and after vaults; saved through the store), then `load_crew`,
`rebuild_index`, `refresh_links`, `rebuild_graph`; `move_crew_folder` if the
crew folder moved.

**Backlinks.** `refresh_links` → `Panel::set_links` (snippets drawn once) →
`refresh_unlinked`: once `FindUnlinked` was pressed (`build_search` if
needed; `SearchBuilt` calls it too), `SearchIndex::unlinked` for each note
opened.

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
through `edit_graph`, or `_funct.md` through `edit_files` (an `Elsewhere`
step, so Ctrl+Z undoes it; the status line stays quiet); then
`rebuild_graph` → `refresh_editor`, which finds the block again, gathers the
wire's values and the wired notes (`Bind`s), and calls `Editor::refresh`
(check + eval → live values). If the function's name isn't found, it takes
the block at the same lines (a rename or its undo) and moves the view key
(`Views::move_view`).

**Renaming a function.** Editor title + Enter → `Outcome::Rename` →
`rename_function`: `name_taken` refuses a name in use. Local: one `edit_graph`
(`func::edit::rename` + `nodes::rename_function`). Saved: `_funct.md` and every
other graph without its own function of that name (`graph_rels`) go to
`edit_files(files, what, joined)` as one `Elsewhere` step; the open graph's own
change is then an `edit_graph`, and `joined` ties the two into one Ctrl+Z.

**Outside changes.** `watch::watch` → `Message::Fs` → `files_changed`:
pictures forgotten, a structural change → `App::rescan` (at most every
250 ms; `Rescans`, `Message::Rescan`, `rescan_landed` after
`VaultRefreshed`), notes queued in `ws.outside` → `drain_outside` slices:
held notes via `Store::check_outside`, others read off-thread →
`Message::OutsideRead` → `outside_read`, then `Message::DrainOutside` after
a pause; 150 ms with nothing new ends the burst: `runs.record`, `note_changed`
for each, `refresh_links`, `rebuild_graph`.

**Runs.** `rebuild_index` (on scan, and when a graph, `_funct.md` or
`_settings.md` changes) → `Index::build`, pins job files, `check_out_of_date`
(`runner::pending` → `runs.out_of_date`, drawn by `rebuild_graph` as
`WireView::stale` / `Card::stale`). A changed note → `note_changed` (from
`save_now`, or from the end of an outside burst) → `runs.pending` → `schedule_run` (timer, 1 s gap) →
`Message::RunNow` (saves the open note, `index.affected`) → `start_run`
(`runner::run` off-thread) → `Message::RunDone` → `run_done` (`runner::apply`
to current texts, `put_text` + save each note once, `runs.record`, recheck).
`RunGraph` runs the open graph's out-of-date jobs. Undo:
`runs::Message::Undo` → `undo_run` (`runner::revert`, `log::mark_undone`).
Files created or deleted → `Vault::scan` → `Message::VaultRefreshed`.

**Crew.** `Workspace::load_team` reads `crew:` from `_settings.md` into
`ws.crew_folder`; `Workspace::load_crew` (scan, rescans, a card or
`_settings.md` changes) → `crew::seats` → `ws.crew`. The Settings page
(`SettingsMessage`, `view_settings`, `settings_message`) writes `crew:` with
`crew::set_frontmatter` through `put_text`. `refresh_links` marks `@names` without a card in the link states and
refreshes `ws.mentions` (vault-wide open counts). `rebuild_graph` →
`Graph::set_crew`. Hand drop → `graph::Message::Assign` → `assign` (a row) or
the `Assigning` box → `AssignTo` → `assign` → `edit_elsewhere`. Delete on a
link wire → `Unlinking` box ("Remove the link to [[b]] from a?") →
`Message::Unlink` → `unlink` (`links::remove_links`, `leads_to`) →
`edit_elsewhere` → `edit_files`, which pushes a `history::Step` on
`ws.elsewhere`, then `put_files` (`put_text` + `note_changed` for each, then
`refresh_links` + `rebuild_graph` once). `undo` asks `ws.elsewhere` first (by
`open_fingerprint`) → `undo_elsewhere`, which refuses if a file changed since;
otherwise `undo_open` (the open note's history), then `undo_joined` for a step
joined to it. `App::open` clears `ws.elsewhere`.
`@name` click → `follow_link("@name")` → `open_crew` (opens or creates
`_crew/name.md`). Typing → `update_completion` → `Completion`; Tab
(`live::Intent::Complete` / `SourceEvent::Complete`, or `Shortcut::Tab` if the
editor didn't take it) or a click → `Message::CompleteMention` →
`Note::complete_mention`. `App::open` clears the suggestions.

**Files the app creates** (a crew card, a new note, `_funct.md`) are read
through `store.get` right after, so the watcher's events for them count as
the app's own writes, not as "Changed in another app".

**Search.** `Shortcut::FindInVault` → `open_find` (saves the open note;
builds `SearchIndex` off-thread the first time → `Message::SearchBuilt`) →
`find::Find`. Typing → `find_message` → `SearchIndex::query`. Enter or a click
→ `open_at(rel, Jump::Line)`. Every `ws.links.update` is followed by
`ws.search.update`, so search stays current.

**Views.** Panning or zooming a graph or the function editor publishes
`graph::Message::Viewed` / `func_editor::Message::Viewed` → `ws.views.set`
(in memory) and the canvas's `saved_view`. A canvas starting fresh (a graph
opened, coming back from the editor, a function opened) uses `saved_view`
instead of fitting. `save_views` writes `.obsessed/views.txt` on leaving a
graph (`open`, `ToggleGraph`, `SetMode`), switching vaults and closing
(`save_blocking`); never while idle. Renames move the keys.

**Startup cache.** `Message::Scanned` → background `LinkIndex::build_cached`
with `.obsessed/cache/links.txt` → `save_cache` if anything was read.
`Message::Saved` → `links.stamp`; `save_blocking` (close) writes the cache if
`links_dirty`.

## Conventions

- Text first: the app changes files with small line edits and never rewrites
  what it doesn't own. Core functions take `&str` and return the new `String`.
- Everything a user sees as ⚠ is a sentence saying what's wrong and what to do.
- Draw only when something changes; canvases cache their still layer and keep
  hover, drag and animation in an overlay. Idle CPU is measured at 0.
- Tests for anything in core; the app is checked with the `dev.rs` hooks and
  screenshots, and by hand (PLAN.md, "Checked by hand").
- Overlays go on a `stack!` that's always there; don't swap a widget for a
  `stack!` only when something shows, or the widgets under it lose their state.
