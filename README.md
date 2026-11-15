# Obsession
Markdown Note App made with Rust. It's Node-based so everyone understood the flow of the project

- [PLAN.md](PLAN.md): how the app is built, milestones, performance budgets
- [Syntax.md](Syntax.md): what the app reads and writes in your notes

## Run it

```bash
cargo run --release
```

Or open a vault folder directly:

```bash
cargo run --release -- "C:\path\to\vault"
```

The app remembers the last vault in `%APPDATA%\Obsession\last_vault.txt`.

| Shortcut | Does |
|---|---|
| Ctrl+O | Open a vault folder |
| Ctrl+N | New note (starts in the open note's folder; `/` makes subfolders) |
| Ctrl+P | Quick switcher |
| Ctrl+E | Cycle Live / Source / Split / Reading |
| Ctrl+S | Save now (autosave runs 1 s after you stop typing) |
| Ctrl+Z / Ctrl+Y | Undo / redo (Ctrl+Shift+Z also redoes) |
| F2 | Rename or move the open note (links to it are updated); right-click a note in the tree does the same |

**Views:** **Live** (the default) shows the note formatted and turns the line
you click into raw markdown, like Obsidian's Live Preview. Arrow keys move
between lines, Enter continues lists, Backspace at the start of a line joins
it to the one above, Esc stops editing. Drag across lines to select: the
selection is highlighted as you drag, and when you let go those lines turn
raw with the text selected, so Ctrl+C, Ctrl+X, typing and Delete work. A code
block, table or the frontmatter is edited as a whole. **Source** is the raw markdown, for big
edits and selecting across many lines. **Split** is Source with a read-only
Live view beside it that follows your typing. The two scroll together: the
view keeps the line at the editor's middle level with it, or the caret's line
when you type or click; scrolling the view scrolls the editor back. **Reading** is Live with editing
off: links and status chips still click, nothing else does. All views centre
the text in the same column.

**Tasks:** click a task's status (in any view) to move it to the next status.
Right-click a task (in any view) to pick any status. Status
formats come from `statuses:` in the note's frontmatter, then `_settings.md`
at the vault root, then the default `[ ]` / `[x]`. See Syntax.md, sections 1–3.

**Links** resolve as in Obsidian: `[[sh040]]` finds the note by name wherever
it is, `[[anim/sh040]]` narrows it by folder, `./` and `../` are relative, and
`#Heading`, `#^block` and `|alias` work. A link is blue when it's fine, dim
when the note doesn't exist yet (clicking creates it, at the vault root or the
path the link names), and amber when it's ambiguous (several notes share the
name; the click opens the one Obsidian would and says so) or its `#heading` or
`^block` isn't there. The **Backlinks** panel on the right lists every line
that links to the open note; click one to go there. Renaming a note rewrites
every `[[link]]` and markdown link to it, keeping headings, aliases and `!`
embeds, using the shortest form that stays unique.

## Workspace

| Package | What |
|---|---|
| `core` (`obsession-core`) | Everything that isn't UI. `cargo test -p obsession-core` |
| `app` (`obsession`) | The iced app |
| `bench` (`obsession-bench`) | Test-vault generator and timings |

## Measuring

```bash
cargo run --release -p obsession-bench -- gen C:\temp\big-vault 10000
```

```bash
cargo run --release -p obsession-bench -- scan C:\temp\big-vault
```

```bash
cargo run --release -p obsession-bench -- read C:\temp\big-vault
```

For memory and frame checks, `cargo build -p obsession --profile measure`
builds a release-speed binary that keeps the debug-only hooks from
`app/src/dev.rs` (open a note, set the view, screenshot, quit), all driven by
`OBSESSION_DEV_*` environment variables. Results so far are in PLAN.md under
"Milestone 1 measurements".
