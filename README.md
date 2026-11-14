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
| Ctrl+E | Cycle Edit / Split / Preview |
| Ctrl+S | Save now (autosave runs 1 s after you stop typing) |

**Tasks:** click a task's box (in the editor or the preview) to move it to the
next status. Right-click a task line in the editor to pick any status. Status
formats come from `statuses:` in the note's frontmatter, then `_settings.md`
at the vault root, then the default `[ ]` / `[x]`. See Syntax.md, sections 1–3.

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
