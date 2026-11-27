# Obsession
Markdown Note App made with Rust. It's Node-based so everyone understood the flow of the project

- [PLAN.md](PLAN.md): how the app is built, milestones, performance budgets
- [Syntax.md](Syntax.md): what the app reads and writes in your notes
- [CODEMAP.md](CODEMAP.md): every file, its main types and functions, and how they connect

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
| F2 | Rename or move the open note (links to it are updated); right-click a note in the tree does the same, and right-click a folder renames or moves the folder |

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

**Pictures:** `![[shot.png]]`, `![[shot.png|300]]` (300 pixels wide) and
`![alt](refs/shot.png)` draw the picture in Live, Split and Reading, found from
any folder like a link, fitted to the text column and turned the way the
camera's EXIF says. PNG, JPEG, GIF (first frame), WebP and BMP. Clicking one
in Live edits its line; in Reading it opens the file. A picture that isn't in
the vault shows a ⚠ box. Pictures load in the background at the size they're
drawn, and at most 64 MB of them are kept in memory.

**Tasks:** click a task's status (in any view) to move it to the next status.
In a note with the default `[ ]` / `[x]`, a click checks and unchecks it; with
a status format of your own, the last status stays put (no un-finishing by
accident). Right-click a task (in any view) to pick any status. Status
formats come from `statuses:` in the note's frontmatter, then `_settings.md`
at the vault root, then the default `[ ]` / `[x]`. See Syntax.md, sections 1–3.

**Graphs:** any note named `…_nodes.md` (`_nodes.md`, `lighting_nodes.md`;
several per folder are fine) opens as a graph (the **Graph** button
switches between the graph and its text). Each node is a card with the note's
name, folder, tasks and progress; amber when several notes share the name,
dashed when the note doesn't exist yet. Drag the background (or hold the middle
button) to pan, scroll to zoom (each graph, and each function in the editor,
opens where you left it), drag a card to move it, double-click a card to
open its note. Drag from the dot on a card's right edge to another card to add
a wire; select a card or wire and press Delete to remove it from the graph
(the note itself stays). Deleting a link wire asks first, then turns the
links back into plain words in the note (`[[b|the shot]]` becomes `the shot`).
Ctrl+Z in the graph also undoes that, and a crew card dropped on a task. The graph's own buttons sit in a small bar in its
top-left corner: **+ Node** adds a note, **+ Function** a function node,
**Crew** shows or hides the crew, and `[[links]]` between cards show
as fainter wires (**Link wires** hides them). A wire to a task or a heading
(`[[a#^t2]]`, `[[a#Notes]]`) ends at that task's row on the card, or at the
first task under the heading. Everything is written back into
`_nodes.md` as Syntax.md section 7 describes, and Ctrl+Z undoes it.

**Functions:** hover a wire and click its **ƒ** to put a function on it: pick
one (the first time, the app creates `_funct.md` with *Wait For Done* and *Copy
Status*) or type a name to create a new one in the graph's `## Functions`. A
wire turns solid once its function checks out, and stays dashed with ⚠ and the
reason when it doesn't. Clicking a wire's ƒ (or double-clicking a `fn` card)
opens the **function editor**: the function's nodes, Blueprint style. Drag a
node to move it; drag from a pin on a node's right to a pin on another's left
to connect them (pins of the wrong type refuse); drag a connection off a pin to
remove it. **+ Node** (or double-click the canvas) adds a node from the
library. Click a node to set its values in the panel; hover a pin to see its
live value, read from the real notes the wire connects. The panel also holds
the wire's own values (`task`, `target`…). **+ Function** adds a function with
several ins or outs as a node; wire notes to its pins. To rename a function,
click its name at the top of the panel, type the new one and press Enter: every
wire, `fn` card and pin that uses it follows (in every graph, for a saved
function), and one Ctrl+Z puts it all back. A name that's already taken is
refused. Every edit in the function editor undoes with Ctrl+Z, saved functions
in `_funct.md` included, in turn with the graph's own edits.

**Search:** Ctrl+Shift+F (or **Search** in the toolbar) finds every line that
has all the words you type, in every note, ignoring case (accented and
non-Latin letters too: `équipe` finds `ÉQUIPE`). Matches are
highlighted; Up and Down move through them, Enter or a click opens the note at
that line, and Esc closes the search. The first search
reads every note once (about a second for 10,000 notes); after that it's
instant and kept up to date as notes change.

**Settings** (toolbar): the crew folder, shared through `_settings.md`, and
**compact layout** for this computer only (a tighter toolbar, file tree and
panels).

**Crew:** one note per person (or bot) in the crew folder (`_crew/ika.md`,
with `name`, `role`, `color` in its frontmatter; Syntax.md section 4), or one
folder per person (`Members/Adam/`, card `Members/Adam/Adam.md`). **Settings**
in the toolbar picks the crew folder (type it, pick it, or **Browse…**); it's saved in `_settings.md` for the
whole team. Type `@` in a
note for suggestions (Tab or click picks one). A click on `@ika` opens Ika's
card; an `@name` without a card shows in amber, and clicking it creates the
card. `portrait: ika.png` on a card gives them a face: on their card in the graph's
hand, on the card being dragged, on a node for their card, and at the top of
their card note (the file is found next to the card, or by name anywhere).
In a graph the crew is a hand of cards along the bottom (**Crew** hides
it): hover a card to light up the nodes and tasks that mention them, drag it
onto a task to add their `@name`, or onto a node to pick the task. Cards show
**OVERLOADED** or **POWERHOUSE** when someone has 2× or 1.5× the open tasks of
the rest of the crew (the median); on a crew card note, the status bar shows
their open tasks across the vault. With many people or a narrow graph the
cards overlap, each still showing its face, name and badge (shortened to
fit); hover one to see it whole.

**Runs:** a function runs when a note it reads changes, whether you edit it
here or in Obsidian, VS Code or a `git pull`. If its changes change a note
another function reads, that one runs too (each line changes at most once per
run). Runs happen at most once a second, and every changed note is saved once.
Opening a vault never changes anything: functions that would change something
show **⚠ out of date** on their wire, and **Run N out of date** in the graph's bar
runs them. **Runs** opens the log: every run with the lines it changed, notes
changed in other apps, and **Undo** for the newest run (it puts back only lines
that still hold what the run wrote). The log lives in `.obsessed/log/` at the
vault root, which has its own `.gitignore`.

**Links** resolve as in Obsidian: `[[sh040]]` finds the note by name wherever
it is, `[[anim/sh040]]` narrows it by folder, `./` and `../` are relative, and
`#Heading`, `#^block` and `|alias` work. A link is blue when it's fine, dim
when the note doesn't exist yet (clicking creates it, at the vault root or the
path the link names), and amber when it's ambiguous (several notes share the
name; the click opens the one Obsidian would and says so) or its `#heading` or
`^block` isn't there. The **Backlinks** panel on the right lists every line
that links to the open note, drawn formatted; click one to go there. Under
them, **Find unlinked mentions** lists lines that name the note without
linking to it. Renaming a note or a folder rewrites every `[[link]]` and
markdown link that no longer leads where it did, keeping headings, aliases
and `!` embeds, using the shortest form that stays unique; relative links
inside moved notes stay relative. Links to pictures and other
files (`![[shot.png]]`, `[[ref.pdf]]`) are found the same way, from any folder;
clicking one opens it in the app Windows uses for that file, and one that isn't
in the vault shows dim.

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
