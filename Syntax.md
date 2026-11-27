# Obsession – Syntax (v0.3)

Everything the app reads and writes is plain markdown. This file describes the
few extra conventions it understands. How the app is built is in
[PLAN.md](PLAN.md).

## 0. The files at a glance

| File | What it is | Shared with the team? |
|---|---|---|
| any `.md` | A note. May contain tasks, `@people` and links | Yes |
| `_nodes.md`, `anything_nodes.md` | A node graph (section 7) | Yes |
| `_funct.md` | Saved functions for the whole vault, at the vault root (section 9) | Yes |
| `_crew/<name>.md` | One crew member's card (section 4) | Yes |
| `_settings.md` | Optional vault settings, at the vault root (section 3) | Yes |
| `.obsessed/` | Undo log, window state, caches (section 11) | No, it ignores itself in git |

Rule of thumb: **a name starting with `_` means something to the app.**

## What changed from v0.2

- `needs::` and `sync::` are gone. Functions on wires do their job, and the
  common cases ship as saved functions (`Wait For Done`, `Copy Status`).
- Status roles (`open`, `done`, `blocked`) and automatic blocking are gone.
- `::statusformat::` is replaced by a `statuses:` list in frontmatter.
- `.obsessed/flows/*.json` is replaced by `_nodes.md`, which is markdown.
- Links use Obsidian's shortest unique name, not always the full path.

## 1. Tasks

A task is a list item with one character in square brackets:

```md
- [ ] Polish pass @ika
- [x] Blocking pass @ika ^a1
```

| Part | Meaning |
|---|---|
| `- ` | List marker. `-`, `*` and `+` all work. |
| `[ ]` | Status. Exactly one character between the brackets (a space counts). How it's drawn comes from the status format (section 3). |
| `Polish pass` | Task text. Anything goes, including emoji and links. |
| `@ika` | Optional. A person on the task (section 4). |
| `^a1` | Optional. Obsidian's block ID (section 5). Rarely needed. |

Rules:

- Tasks can be indented under other tasks (subtasks).
- Lines inside code fences (` ``` ` or `~~~`) are never tasks.
- The brackets must come right after the list marker and a space.
  `Text with [ ] in the middle` is not a task, and neither is `-[ ] no space`.
- Numbered lists (`1. [ ]`) are not tasks in v0.3.

## 2. Clicking a status

- **Click** moves a task to the next status in its file's format. The last
  status stays where it is, so a finished shot can't be un-finished by accident.
  A note without a format of its own (no `statuses:` in it or in
  `_settings.md`) uses the default `[ ]` / `[x]`, and there a click is a
  checkbox: `[x]` goes back to `[ ]`.
- **Right-click** opens a menu of every status in the format, each with its
  symbol, label and character, the current one highlighted. Pick any of them.
- This works wherever a status shows: the editor, the preview and the graph.
- The app only changes the one character between the brackets.

## 3. Status format

A file chooses its statuses in frontmatter. The order is the click order.

```yaml
---
statuses:
  - "[-] ⚪ cannot start"
  - "[ ] 🔳 ready to start"
  - "[?] ❓ ready to check"
  - "[x] ✅ finished"
---
```

Each entry is `"[character] symbol label"`:

| Part | Example | Meaning |
|---|---|---|
| character | `[?]` | What you type in the task's brackets. Exactly one character. |
| symbol | `❓` | What the app draws instead of the brackets: the first word after `]`. Usually an emoji. |
| label | `ready to check` | Everything after the symbol. Shown in menus and tooltips. |

- Keep the quotes. Without them YAML reads `[x]` as a list.
- Obsidian shows `statuses` as a normal list property.

**Which format applies**, first match wins:

1. The file's own `statuses:`.
2. `statuses:` in `_settings.md` at the vault root (the team default).
3. The built-in default:

   ```yaml
   statuses:
     - "[ ] 🔳 to do"
     - "[x] ✅ done"
   ```

**The last status counts as finished.** Only the crew badges use this
(section 4). Nothing else in the app treats any status as special.

**Mistakes:**

- A task uses a character that isn't in its format: the app shows ⚠ and leaves
  the line alone. As in Obsidian, `[X]` counts as `[x]` unless the format lists
  `X` itself.
- A broken entry (no brackets, more than one character, no symbol) is skipped
  with a warning. The rest of the format still works.
- The same character listed twice: the first one wins, with a warning.

Tip: keeping `[ ]` and `[x]` in your format means GitHub and Obsidian draw real
checkboxes for them.

## 4. People and crew

### Mentions

`@name` on a task's line assigns someone. Several people per task are fine.

- Names use letters, digits, `-` and `_`. Matching ignores case.
- The `@` has to start a word, so `bob@example.com` is not a mention.
- `@ika` means the card `_crew/ika.md`. An `@name` with no card shows ⚠ and a
  "create card" button.

### Crew cards

One markdown file per person (or bot) in the **crew folder**: `_crew/` at the
vault root, or the folder `crew:` names in `_settings.md` (the app's
**Settings** page sets it: type the path, pick from the list, or **Browse…**):

```yaml
---
crew: 01_Project_Planning_and_Assignments/Members
---
```

In the crew folder:

- Each note is a person: `Members/ika.md` is `@ika`.
- Each folder is a person too: `Members/Adam/` is `@adam`, and their card is
  the note named like the folder, `Members/Adam/Adam.md`. The folder's other
  notes are left alone. If the card doesn't exist yet, the app creates it the
  first time it's needed (clicking `@adam`).
- A name with spaces becomes `@` with dashes: `Members/Ika Putri/` is
  `@ika-putri`.

A card looks like this:

```md
---
name: Ika Putri
role: Animator
portrait: ika.png
color: "#e8826b"
skills: [anim, rig]
kind: human
---
Anything else about Ika: notes, links, contacts. It's a normal note.
```

| Field | Meaning |
|---|---|
| `name` | Shown on the card. Defaults to the file name. |
| `role` | Shown under the portrait. |
| `portrait` | Optional picture (PNG, JPEG, GIF, WebP, BMP), relative to the card file, or found by its name anywhere in the vault like a link. Drawn round, from its middle square, on the hand, the dragged card, a node for the card and at the top of the card note. Without one (or if the file isn't there), the app draws initials in `color`. |
| `color` | Card accent color. |
| `skills` | Optional tags shown as icons on the card. |
| `kind` | `human` (default) or `bot`. Bots get a rounded square instead of a circle, with or without a portrait. |

All fields are optional.

### The card hand

The crew appears as a hand of cards in the graph view.

- **Hover** a card to highlight the nodes and tasks where that person appears.
- **Drag** a card onto a task to add their `@name` to the end of the task text.
  Drop it onto a node card to choose which tasks.
- Each card shows the portrait, name, role and how many nodes mention them.
  When the cards overlap, each keeps its portrait, name and badge in the part
  that shows; the hovered card rises whole.
- Double-click a card to open its note.

### Badges

Badges are calculated live and never saved.

- **Mentions** = `@name` on task lines that are *not* in their file's last
  status. In the graph view this counts the graph's nodes. Elsewhere it counts
  the whole vault.
- Each person's mentions are compared with the **median** of everyone else's:

| Mentions vs. the others' median | Badge |
|---|---|
| 2× or more | **OVERLOADED** (red, a warning) |
| 1.5× up to 2× | **POWERHOUSE** (purple, a compliment) |
| less than 1.5× | none |

- No badges if the crew has fewer than 3 people, or if the others' median is 0.

## 5. Task IDs

```md
- [ ] Polish pass @ika ^a2
```

- This is Obsidian's block ID, unchanged: letters, digits and `-`, at the very
  end of the line, after a space. Obsidian links to it and hides it in reading
  view.
- Each ID must be unique within its file.
- You rarely need one: functions usually find tasks by their text
  (`Find Task`, section 8).
- When you drag a wire from a **task's pin** in the graph, the app adds a random
  ID in Obsidian's style (`^3f9a2c`) if the task has none, the same thing
  Obsidian does when you copy a block link. It's the only automatic edit to a
  task's text.
- A `^id` on a line that isn't a task is left alone.

## 6. Links

The app reads the same links Obsidian does:

| Form | Points at |
|---|---|
| `[[sh040]]` | A file |
| `[[sh040\|Shot 40]]` | A file, shown as "Shot 40" |
| `[[sh040#Lighting notes]]` | A heading in the file |
| `[[sh040#^a2]]` | A block, usually a task |
| `[[#^a2]]` | A block in the same file |
| `[text](anim/sh040.md)` | A file, with a normal markdown link |

**Names resolve like Obsidian's default (shortest unique name):**

- `[[sh040]]` works if only one file in the vault is called `sh040.md`.
- If several are (`anim/sh040.md`, `lighting/sh040.md`), write enough of the
  path to tell them apart: `[[anim/sh040]]`. Paths start at the vault root and
  leave out `.md`.
- If a link still matches more than one file, the app shows ⚠ instead of
  guessing.
- Whenever the app writes a link, it writes the shortest unique form.
- **Renaming or moving a file** in the app updates every link to it, in both
  link styles.
- Links inside code fences and inline code are ignored.
- **Pictures and other files** (`![[shot.png]]`, `[[ref.pdf]]`, videos,
  `.blend`…) resolve the same way, with their extension: by name from any
  folder, a path to narrow it, `./` and `../` relative. When two share a
  name, the one in the linking note's folder wins, then the shortest path,
  as Obsidian picks. A click opens the file in its usual app; a link to a
  file that isn't in the vault shows as missing and never creates a note.

## 7. Graphs: `_nodes.md`

Any note whose name ends in `_nodes.md` is a graph: `_nodes.md`,
`lighting_nodes.md`, `ep04 delivery_nodes.md` (case doesn't matter). A folder can
hold as many as you like. It's a normal note with three sections the app
understands. Everything else in it is yours.

~~~md
# sh040

Brief, references, anything you like.

## Nodes
- [[anim/sh040]]
- [[fx/sh040-water]]
- [[lighting/sh040]]
- [[comp/sh040]]
- fn Check Lighting

## Wires
- [[lighting/sh040]] → [[comp/sh040]] : Wait For Done(task: "Render lighting pass", target: "Comp sh040")
- [[comp/sh040]] → [[delivery/ep04]]
- [[anim/sh040]] → Check Lighting.anim
- [[fx/sh040-water]] → Check Lighting.fx
- Check Lighting.light → [[lighting/sh040]]

## Functions
```obsession
fn Check Lighting
in anim: File
in fx: File
out light: File

at = Tasks(anim)
ad = All Status(at, "x")
fl = Lines(fx)
fa = Any Contains(fl, "approved", ignore case: true)
ok = And(ad, fa)
st = Select(ok, " ", "-")
Set Status(light, st, find: "Render lighting pass", from: "- ")
```

%%
obsession-layout
anim/sh040 120,80
fx/sh040-water 120,260
lighting/sh040 420,80
comp/sh040 720,80
fn Check Lighting 300,200
%%
~~~

### `## Nodes`

- Each list item is one node: a single link to a file, or `fn Name` for a
  function shown as a node (section 8).
- Any file can be a node, including files from other folders.
- Links to a graph named just `_nodes.md` must include the path
  (`[[fx/_nodes]]`), because many folders have one. A graph with its own name
  (`[[lighting_nodes]]`) links like any note.

### `## Wires`

Each list item is one wire: `from → to`, optionally followed by
`: Function Name(...)`. `->` works as well as `→`.

| Wire | Drawn | Does |
|---|---|---|
| `[[a]] → [[b]]` | Dashed | Nothing. It shows a relation. |
| `[[a]] → [[b]] : Wait For Done(...)` | Solid once the function checks out, dashed with ⚠ if not | Runs the function |
| `[[a]] → Check Lighting.anim` | Solid or dashed, as above | Feeds a function node's input |
| `Check Lighting.light → [[b]]` | Solid or dashed, as above | Sends a function node's output |

Wire ends:

| End | Kind | Example |
|---|---|---|
| A file | `File` | `[[anim/sh040]]` |
| A task | `Task` | `[[anim/sh040#^a2]]` |
| A heading's section | `Section` | `[[lighting/sh040#Notes]]` |
| A function node's pin | the pin's kind | `Check Lighting.anim` |

The values in `( )` after the function name fill its `param`s (section 8).
Leave them out to use the defaults.

### Links as wires

A `[[link]]` written inside a note also shows as a **dashed** wire, as long as
both files are nodes in the open graph.

- It runs from the **linked** file to the file **that contains the link**: if
  `lighting/sh040` mentions `[[anim/sh040]]`, the wire runs anim → lighting.
- Adding a function to it writes a wire line into `## Wires`. The link in the
  note stays where it is.
- Deleting it in the graph asks whether to also remove the link from the note.
  Removing keeps the link's words: `[[b|the shot]]` becomes `the shot`,
  `[[b]]` becomes `b`, `[[b#Notes]]` becomes `b > Notes`. Every link in that
  note leading to the same note goes, so the wire does too.
- A toggle in the graph hides link wires.

### Layout

Node positions live in an Obsidian comment (`%% ... %%`) at the end of the file,
so Obsidian hides them in reading view. The first line is `obsession-layout`,
then one line per node: the node as written in `## Nodes` (without `[[ ]]`)
and its `x,y`. Nodes without a position are placed automatically, in columns
along the `## Wires`. Moving a card rewrites this comment with every node's
position, in whole pixels; nothing else in the file changes.

### Housekeeping

- The app finds the three sections by their `##` headings. If one is missing,
  the app adds it at the end when it needs it.
- The app never changes anything else in the file.

## 8. Functions

A function is a small graph of nodes, written as text in an `obsession` code
block, one node per line. The app shows it as Blueprint-style nodes. You never
have to type it, but you can.

````md
```obsession
fn Wait For Done
in src: File
out dst: File
param task = ""
param target = ""
param done = "x"

t  = Find Task(src, task)          @40,60
s  = Status(t)                     @200,60
ok = Equal(s, done)                @360,60
st = Select(ok, " ", "-")          @520,60
Set Status(dst, st, find: target, from: "- ")  @680,60
```
````

| Line | Meaning |
|---|---|
| `fn Name` | First line. The name can contain spaces. |
| `in name: Kind` | An input. `Kind` is `File`, `Task` or `Section`. |
| `out name: Kind` | An output: something an action can change. |
| `param name = value` | A value the wire can set, with a default. |
| `id = Node(args)` | A node with a result. `id` uses letters, digits, `-` and `_`. |
| `Node(args)` | An action node (no result). |
| `@x,y` | Optional, at the end of a node line: its position in the function editor. At the end of the `fn` line: where the inputs node sits. |
| `// text` | A comment. |

- Arguments are positional or named (`ignore case: true`). An argument is a node
  `id`, an `in`, `out` or `param` name, `"text"` (with `\"` for a quote, `\\`
  for a backslash, `\n` for a new line), a number, or `true` / `false`.
- One node per line, no nesting. Line order doesn't matter. Loops are an error.

### On a wire or as a node

- A function with **exactly one `in` and one `out`** can sit on a wire:
  `[[a]] → [[b]] : Name`. It's drawn as a small `ƒ` dot on a thin wire.
- A function with **more inputs or outputs** is listed in `## Nodes` as
  `fn Name` and wired by its pins (`Name.pin`). It's drawn as a visible node.
- Adding an input or output in the editor moves it from one form to the other.
  The app rewrites the lines for you.

### Where functions live

- **Local:** in `## Functions` of a `_nodes.md`. Only that graph can use them.
- **Saved:** in `_funct.md` (section 9). Every graph can use them.
- If both have the same name, the local one wins, with a warning.
- **Renaming:** edit the name at the top of the function editor and press
  Enter. The app rewrites the `fn` line and every use: `- fn Name` in
  `## Nodes`, `: Name(...)` on wires, `Name.pin` wire ends and the layout
  line. A local function is renamed in its own graph only; a saved one in
  `_funct.md` (its `## Name` heading too) and in every graph that uses it,
  except a graph with a local function of the same name. A name another
  function already has is refused. One Ctrl+Z undoes the whole rename.

### Types

| Type | What it holds |
|---|---|
| `File` | A file. Wherever `Text` is expected it gives the file's body (without frontmatter). |
| `Text` | Text. A status is the one-character text between the brackets: `" "`, `"x"`. |
| `Number` | A number. |
| `Bool` | `true` or `false`. |
| `Task` | One task: its status, text, people and ID. |
| `List` | A list of `Text` or a list of `Task`. |

A `Section` input reads as `Text`: the lines under that heading.

When a node can't produce a value (`Find Task` matched nothing, `Get` was out of
range), everything after it gets **no value**. Actions with no value don't fire,
and the wire shows ⚠ with the reason.

### Solid or dashed

A wire is **solid** when the function exists, every `in` and `out` is
connected, every node's required arguments are filled, every type matches, and
there's at least one action. Otherwise it's **dashed with ⚠** that says what's
missing. A wire with no function is plain dashed, without ⚠.

### Node library

**Text**

| Node | Gives |
|---|---|
| `Contains(text, find, ignore case: false)` | Bool |
| `Starts With(text, find, ignore case: false)` | Bool |
| `Ends With(text, find, ignore case: false)` | Bool |
| `Equal(a, b, ignore case: false)` | Bool |
| `Is Empty(text)` | Bool |
| `Len(text)` | Number |
| `Left(text, count)` / `Right(text, count)` | Text |
| `Mid(text, start, count)` | Text |
| `Find(text, find, ignore case: false)` | Number (position, or -1) |
| `Replace(text, from, to, ignore case: false)` | Text |
| `Trim(text)` / `Lower(text)` / `Upper(text)` | Text |
| `Matches Wildcard(text, pattern)` | Bool (`*` and `?`, e.g. `"sh0*"`) |
| `Join Text(a, b, separator: "")` | Text |
| `Split(text, by)` | List of Text |
| `Lines(text)` | List of Text |

**Lists**

| Node | Gives |
|---|---|
| `Count(list)` | Number |
| `Get(list, index)` | One item (counting from 0) |
| `First(list)` / `Last(list)` | One item |
| `Filter Contains(list, find, ignore case: false)` | List of the items whose text contains `find` |
| `Any Contains(list, find, ignore case: false)` | Bool |
| `All Contains(list, find, ignore case: false)` | Bool |
| `Join(list, separator)` | Text |

**Logic**

| Node | Gives |
|---|---|
| `Not(bool)` / `And(a, b)` / `Or(a, b)` | Bool |
| `Select(bool, if true, if false)` | Whichever value is picked |
| `Compare(a, op, b)` | Bool. `op` is `"="`, `"<"`, `">"`, `"<="` or `">="` |

**Markdown and tasks**

| Node | Gives |
|---|---|
| `Tasks(text)` | List of Task |
| `Find Task(text or list, find)` | The first task whose text contains `find` (ignoring case; empty `find` takes the first task). If `find` starts with `^`, matches the ID instead |
| `Status(task)` | Text |
| `Task Text(task)` | Text, without status, people or ID |
| `People(task)` | List of Text |
| `Count Status(tasks, status)` | Number |
| `All Status(tasks, status)` | Bool |
| `Section(text, heading)` | Text under that heading |
| `Frontmatter(file, key)` | Text |

**Actions** (they change the `out` they're given)

| Node | Changes |
|---|---|
| `Set Status(target, status, find: "", from: "", when: true)` | The status character of a `Task` target, or of the first task in a `File` target whose text contains `find`. If `from` is set, only changes a task whose current status is one of those characters, so work that has already moved on is left alone |
| `Replace Text(target, from, to, when: true)` | Text in a `File` or `Section` |
| `Ensure Line(target, line, when: true)` | Adds `line` at the end of a `File` or `Section`, **only if it isn't already there** |
| `Write Section(target, text, when: true)` | Replaces everything under a heading (a `Section` target) |

- There are no exec wires. An action fires when its `when` is `true`.
- Every action is safe to run twice: running it again on the same files changes
  nothing. This keeps teammates who all run the app from duplicating lines.
- `Set Status` refuses a character that isn't in the target file's format, and
  shows ⚠ instead of writing it.

### Live values

Hovering a pin in the function editor shows its current value, read from the
real files. That's the debugger.

## 9. Saved functions: `_funct.md`

`_funct.md` at the vault root holds the functions every graph can use. It's a
normal note: write descriptions between them if you like.

````md
# Functions

## Wait For Done
Sets the target task to ready when the source task is finished, and back to
"cannot start" if it isn't (retakes). `from: "- "` means it only touches the
target while it's in `[-]` or `[ ]`, so a task someone already started or
finished is never reset.

```obsession
fn Wait For Done
in src: File
out dst: File
param task = ""
param target = ""
param done = "x"

t  = Find Task(src, task)
s  = Status(t)
ok = Equal(s, done)
st = Select(ok, " ", "-")
Set Status(dst, st, find: target, from: "- ")
```

## Copy Status
Gives the target task the same status as the source task.

```obsession
fn Copy Status
in src: File
out dst: File
param task = ""
param target = ""

t = Find Task(src, task)
s = Status(t)
Set Status(dst, s, find: target)
```
````

- Names must be unique across `_funct.md`.
- Renaming one from the function editor (section 8) renames its `## Name`
  heading too, and every graph's wires that use it.
- **Editing a saved function updates every wire that uses it.** To make a
  variant, use **Duplicate** in the function library and edit the copy.
  Ctrl+Z in the function editor undoes such an edit (unless `_funct.md`
  was changed outside since).
- The app creates `_funct.md` with these two functions the first time you open
  the function library.

## 10. When things happen

- **Opening a vault never changes files.** The app checks functions in memory
  in the background: the open graph first, then the rest of the vault a few at a
  time. If one would change something, its wire shows ⚠ "out of date" with a
  **Run** button.
- **Functions run after a file changes**: from the app, Obsidian, VS Code or a
  `git pull`. Only functions with an input in a changed file run.
- **At most one run per second.** If files change while a run is going, they
  join **one** pending run that starts when the current one ends. Changes never
  pile up.
- **Chains:** if a run changes a file that another function reads, that function
  runs too, in the same run. Each task, line or section can change **at most
  once per run**. A second change is skipped and reported as a loop.
- **One write per file per run.** All changes happen in memory first, then each
  changed file is written once.
- **Safe writes:** the app writes a temporary file and swaps it in, so nobody
  ever reads a half-written file. If Windows has the file locked, it retries
  briefly.
- **Autosave** in the editor: 1 second after you stop typing, and when you
  switch files or close the app. Nothing is written if nothing changed.
- **Undo:** every change a run makes is logged with the file, line, old text and
  new text. A whole run undoes as one step, newest first. Undo only reverts a
  line that still holds what the run wrote. If someone changed it since, that
  line is skipped with a warning.
- **Outside edits** show in the log (the **Runs** panel) but can't be undone
  from the app.

## 11. The `.obsessed/` folder

At the vault root, for this machine only:

- `log/` – the undo log (the last 500 runs)
- `views.txt` – where you last looked in each graph and each function (the
  point in the middle and the zoom), so they open where you left them
- `cache/links.txt` – the link index, so opening a vault only reads notes
  changed since last time

The app puts a `.gitignore` containing `*` inside it, so it never reaches git.
Deleting the folder is safe: you only lose undo history and window positions.

## 12. Markdown support

The app never reformats a file or drops anything it doesn't understand. The
raw editor always shows the exact text.

| | Drawn in preview |
|---|---|
| **Standard markdown** | Headings, emphasis, lists, quotes, code (highlighted), links, rules, pictures (`![alt](shot.png)`, `![alt\|300](shot.png)`) |
| **GitHub extras** | Tables, task lists, strikethrough, footnotes |
| **Obsidian extras** | `[[links]]` in every form from section 6, `#tags`, frontmatter properties, `%% comments %%` (hidden), `==highlights==`, `> [!note]` callouts, picture embeds (`![[shot.png]]`, `![[shot.png\|300]]`) |
| **Not yet** (shown as a 📎 placeholder or plain text) | Note embeds (`![[other note]]`), PDFs and videos, pictures from the web, math (`$...$`), Mermaid |

Pictures (PNG, JPEG, GIF, WebP, BMP) are found like any attachment (section
6), drawn under their line at their own size or the `|width` given, never
wider than the text column. A line holding only pictures shows just them.

Also: a backlinks panel, a quick switcher and vault search.

## 13. How it looks in other tools

| Syntax | Obsidian | GitHub |
|---|---|---|
| `- [ ]` and `- [x]` | Checkbox | Checkbox |
| `- [-]`, `- [?]` and other characters | Checked box, unless a theme styles it | Plain text |
| `statuses:` frontmatter | A list property | A table at the top |
| `[[file#^id]]` | Link that jumps to the task | Plain text |
| `^id` | Hidden in reading view | Plain text |
| `obsession` code blocks | A code block | A code block |
| `%% obsession-layout %%` | Hidden in reading view | Plain text |

## 14. Full example

```
vault/
  _settings.md
  _funct.md
  _crew/
    ika.md
    maya.md
    unit7.md
  anim/sh040.md
  lighting/sh040.md
  comp/sh040.md
  shots/_nodes.md
```

`_settings.md`:

```md
---
statuses:
  - "[-] ⚪ cannot start"
  - "[ ] 🔳 ready to start"
  - "[?] ❓ ready to check"
  - "[x] ✅ finished"
---
Team defaults for the vault.
```

`anim/sh040.md`:

```md
# Animation – sh040

- [x] Blocking pass @ika
- [?] Polish pass @ika
```

`lighting/sh040.md`:

```md
# Lighting – sh040

Late-afternoon sun, warm key from camera left.

- [x] Key and fill setup @maya @unit7
- [-] Render lighting pass @maya @unit7
```

`shots/_nodes.md`:

```md
# Shots

## Nodes
- [[anim/sh040]]
- [[lighting/sh040]]
- [[comp/sh040]]

## Wires
- [[anim/sh040]] → [[lighting/sh040]] : Wait For Done(task: "Polish pass", target: "Render lighting pass")
- [[lighting/sh040]] → [[comp/sh040]]
```

When the polish pass goes from `[?]` to `[x]`, the app rewrites one character in
`lighting/sh040.md`:

```md
- [ ] Render lighting pass @maya @unit7
```

If the polish pass is sent back to `[?]` for a retake, the lighting task goes
back to `[-]`, unless someone has already moved it past `[ ]`.
The wire from lighting to comp is dashed: it shows the relation and does
nothing yet.

## Ideas for later

- Functions that call other saved functions.
- A `_nodes.md` linking another folder's `_nodes.md`, drawn as a group card you
  can open.
- Note embeds, PDFs, math and Mermaid in the preview.
- Due dates and priorities on tasks.
- More badges for crew cards.
