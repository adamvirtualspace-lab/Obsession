//! Changes to a function's text from the editor. Each touches only the
//! lines it must (a step's line, or one line added or removed); comments,
//! blank lines and every other line stay as they were.

use super::check::bind;
use super::library::{self, Default};
use super::{Arg, Block, Function, Literal, PortKind, Step, Value, write_call};

/// The file as lines, remembering its line ending.
struct Lines {
    lines: Vec<String>,
    ending: &'static str,
}

impl Lines {
    fn new(text: &str) -> Self {
        let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
        Self {
            lines: text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect(),
            ending,
        }
    }

    fn join(&self) -> String {
        self.lines.join(self.ending)
    }
}

/// Splits a line into (indent, code, trailing comment with its spaces).
fn parts(raw: &str) -> (&str, &str, &str) {
    let indent_len = raw.len() - raw.trim_start().len();
    let rest = &raw[indent_len..];
    let code = super::strip_comment(rest).trim_end();
    let comment_start = indent_len + code.len();
    (&raw[..indent_len], code, &raw[comment_start..])
}

/// Rewrites a step's line from `step`, keeping its indent and comment.
fn rewrite(text: &str, step: &Step) -> String {
    let mut lines = Lines::new(text);
    let Some(raw) = lines.lines.get(step.line) else {
        return text.to_owned();
    };
    let (indent, _, comment) = parts(raw);
    let comment = if comment.trim().is_empty() { String::new() } else { comment.to_owned() };
    lines.lines[step.line] = format!("{indent}{}{comment}", super::write_step(step));
    lines.join()
}

/// Moves a step in the editor: rewrites only its `@x,y`.
pub fn set_pos(text: &str, step: &Step, x: f32, y: f32) -> String {
    let mut lines = Lines::new(text);
    let Some(raw) = lines.lines.get(step.line).cloned() else {
        return text.to_owned();
    };
    let (indent, code, comment) = parts(&raw);
    let pos = format!("@{},{}", x.round() as i64, y.round() as i64);
    let code = match code.rfind('@') {
        Some(at) if !code[at..].contains(')') && !code[at..].contains('"') => format!("{}{pos}", &code[..at]),
        _ => format!("{code}  {pos}"),
    };
    lines.lines[step.line] = format!("{indent}{code}{comment}");
    lines.join()
}

/// Moves the inputs node in the editor: `@x,y` at the end of the block's
/// `fn` line (added, or replaced).
pub fn set_entry_pos(text: &str, block: &Block, x: f32, y: f32) -> String {
    let mut lines = Lines::new(text);
    let end = block.close.min(lines.lines.len());
    let Some(i) = (block.open + 1..end).find(|&i| lines.lines[i].trim_start().starts_with("fn ")) else {
        return text.to_owned();
    };
    let raw = lines.lines[i].clone();
    let (indent, code, comment) = parts(&raw);
    let (name, _) = super::split_pos(code);
    lines.lines[i] = format!("{indent}{name}  @{},{}{comment}", x.round() as i64, y.round() as i64);
    lines.join()
}

/// The arguments for a node's inputs, written the usual way: required
/// inputs in order without names while none is missing, the rest by name.
pub fn args_for(op: &str, values: &[Option<Value>]) -> Vec<Arg> {
    let Some(spec) = library::find(op) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut positional = true;
    for (k, input) in spec.inputs.iter().enumerate() {
        let Some(v) = values.get(k).cloned().flatten() else {
            positional = false;
            continue;
        };
        // A value equal to the default is left out.
        if let (Value::Lit(l), Some(d)) = (&v, input.default.literal())
            && *l == d
        {
            positional = false;
            continue;
        }
        positional &= input.default == Default::Required;
        out.push(Arg {
            name: (!positional).then(|| input.name.to_owned()),
            value: v,
        });
    }
    out
}

/// Sets (or clears, with `None`) one input of a step.
pub fn set_arg(text: &str, step: &Step, input: usize, value: Option<Value>) -> String {
    let Some(spec) = library::find(&step.op) else {
        return text.to_owned();
    };
    let (mut values, _) = bind(spec, &step.args);
    if input >= values.len() {
        return text.to_owned();
    }
    values[input] = value;
    let mut changed = step.clone();
    changed.args = args_for(&step.op, &values);
    rewrite(text, &changed)
}

/// An unused id for a new step of node `op` (its initials: `ft` for Find
/// Task), numbered if taken.
pub fn fresh_id(f: &Function, op: &str) -> String {
    let base: String = op
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_lowercase)
        .collect();
    let base = if base.is_empty() { "n".to_owned() } else { base };
    let taken = |id: &str| {
        f.steps.iter().any(|s| s.id.as_deref() == Some(id)) || f.port(id).is_some() || f.params.iter().any(|p| p.name == id)
    };
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}{n}")).find(|id| !taken(id)).unwrap_or(base)
}

/// Adds a step at the end of the block. Returns the text and the new
/// step's line.
pub fn add_step(text: &str, block: &Block, op: &str, x: f32, y: f32) -> (String, usize) {
    let spec = library::find(op);
    let id = match spec {
        Some(s) if s.is_action() => None,
        _ => Some(fresh_id(&block.function, op)),
    };
    let step = Step {
        id,
        op: spec.map_or(op, |s| s.name).to_owned(),
        args: Vec::new(),
        pos: Some((x, y)),
        line: 0,
    };
    let mut lines = Lines::new(text);
    let at = block.close.min(lines.lines.len());
    // Before trailing blank lines, after the last step.
    let mut at_line = at;
    while at_line > block.open + 1 && lines.lines[at_line - 1].trim().is_empty() {
        at_line -= 1;
    }
    lines.lines.insert(at_line, super::write_step(&step));
    (lines.join(), at_line)
}

/// Removes a step, and clears the inputs that read it.
pub fn remove_step(text: &str, block: &Block, index: usize) -> String {
    let f = &block.function;
    let Some(step) = f.steps.get(index) else {
        return text.to_owned();
    };
    let mut out = text.to_owned();
    if let Some(id) = &step.id {
        for other in &f.steps {
            let Some(spec) = library::find(&other.op) else { continue };
            let (values, _) = bind(spec, &other.args);
            if !values.iter().any(|v| matches!(v, Some(Value::Ref(r)) if r == id)) {
                continue;
            }
            let cleared: Vec<Option<Value>> = values
                .into_iter()
                .map(|v| v.filter(|v| !matches!(v, Value::Ref(r) if r == id)))
                .collect();
            let mut changed = other.clone();
            changed.args = args_for(&other.op, &cleared);
            out = rewrite(&out, &changed);
        }
    }
    let mut lines = Lines::new(&out);
    if step.line < lines.lines.len() {
        lines.lines.remove(step.line);
    }
    lines.join()
}

/// Renames step `index`'s id to `new`: its own `id =`, and every argument
/// of another step that reads it. Only those names change on each line
/// (spacing, `@x,y`, quoted text and comments stay as written). Refused
/// with a sentence when `new` isn't a name or is taken by another step, an
/// `in`, an `out` or a param.
pub fn rename_step(text: &str, f: &Function, index: usize, new: &str) -> Result<String, String> {
    check_step_rename(f, index, new)?;
    let step = &f.steps[index];
    let old = step.id.as_deref().unwrap_or_default();
    let new = new.trim();
    if new == old {
        return Ok(text.to_owned());
    }
    let mut lines = Lines::new(text);
    // The step's own `old =`.
    if let Some(raw) = lines.lines.get(step.line).cloned() {
        let (indent, code, comment) = parts(&raw);
        if let Some(rest) = code.strip_prefix(old)
            && rest.trim_start().starts_with('=')
        {
            lines.lines[step.line] = format!("{indent}{new}{rest}{comment}");
        }
    }
    // Every argument that reads it.
    for other in &f.steps {
        if !other.args.iter().any(|a| matches!(&a.value, Value::Ref(r) if r == old)) {
            continue;
        }
        let Some(raw) = lines.lines.get(other.line).cloned() else { continue };
        let (indent, code, comment) = parts(&raw);
        lines.lines[other.line] = format!("{indent}{}{comment}", rename_refs(code, old, new));
    }
    Ok(lines.join())
}

/// Whether step `index`'s id may become `new` (see `rename_step`).
pub fn check_step_rename(f: &Function, index: usize, new: &str) -> Result<(), String> {
    let step = f.steps.get(index).ok_or("That node is gone.")?;
    let old = step.id.as_deref().ok_or("This node has no id to rename.")?;
    let new = new.trim();
    if new == old {
        return Ok(());
    }
    if !super::is_name(new) {
        return Err(format!("`{new}` can't be an id: use letters, digits, - and _, not starting with a digit"));
    }
    let taken = f.steps.iter().enumerate().any(|(i, s)| i != index && s.id.as_deref() == Some(new))
        || f.inputs.iter().chain(&f.outputs).any(|p| p.name == new)
        || f.params.iter().any(|p| p.name == new);
    if taken {
        return Err(format!("`{new}` is already used in this function"));
    }
    Ok(())
}

/// `code` (one step) with every argument that is the name `old` written
/// `new`: inside the call's parentheses, outside quotes, and not an
/// argument's own name (`find: x`).
fn rename_refs(code: &str, old: &str, new: &str) -> String {
    let Some(open) = code.find('(') else {
        return code.to_owned();
    };
    let is_name_char = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(code.len() + new.len());
    out.push_str(&code[..=open]);
    let rest = &code[open + 1..];
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '"' {
            // A quoted literal, escapes and all.
            out.push(c);
            let mut escaped = false;
            for (_, q) in chars.by_ref() {
                out.push(q);
                if escaped {
                    escaped = false;
                } else if q == '\\' {
                    escaped = true;
                } else if q == '"' {
                    break;
                }
            }
        } else if is_name_char(c) {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if !is_name_char(d) {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
            let word = &rest[i..end];
            let names_an_arg = rest[end..].trim_start().starts_with(':');
            out.push_str(if word == old && !names_an_arg { new } else { word });
        } else {
            out.push(c);
        }
    }
    out
}

/// Adds an `in`, `out` or `param` line after the block's other ports and
/// params (before the first step).
pub fn add_header(text: &str, block: &Block, line: &str) -> String {
    let f = &block.function;
    let last = f
        .inputs
        .iter()
        .chain(&f.outputs)
        .map(|p| p.line)
        .chain(f.params.iter().map(|p| p.line))
        .max();
    let at = match last {
        Some(l) => l + 1,
        // Right after `fn Name`.
        None => (block.open + 1..block.close)
            .find(|&i| text.lines().nth(i).is_some_and(|l| l.trim_start().starts_with("fn ")))
            .map_or(block.open + 1, |i| i + 1),
    };
    let mut lines = Lines::new(text);
    lines.lines.insert(at.min(lines.lines.len()), line.to_owned());
    lines.join()
}

pub fn port_line(input: bool, name: &str, kind: PortKind) -> String {
    format!("{} {name}: {}", if input { "in" } else { "out" }, kind.name())
}

pub fn param_line(name: &str, default: &Literal) -> String {
    format!("param {name} = {}", default.write())
}

/// Removes one whole line (a port or param).
pub fn remove_line(text: &str, line: usize) -> String {
    let mut lines = Lines::new(text);
    if line < lines.lines.len() {
        lines.lines.remove(line);
    }
    lines.join()
}

/// A new function's block (fences included).
pub fn new_block(name: &str) -> Vec<String> {
    vec![
        "```obsession".into(),
        format!("fn {name}"),
        "in src: File".into(),
        "out dst: File".into(),
        "```".into(),
    ]
}

/// Renames a function where it's defined: its `fn` line, and with
/// `heading`, the heading just above its block when it carries the old name
/// (`## Wait For Done` in `_funct.md`). Uses elsewhere are
/// `nodes::rename_function`'s.
pub fn rename(text: &str, block: &Block, new: &str, heading: bool) -> String {
    let mut lines = Lines::new(text);
    let old = block.function.name.clone();
    let end = block.close.min(lines.lines.len());
    if let Some(i) = (block.open + 1..end).find(|&i| lines.lines[i].trim_start().starts_with("fn ")) {
        let raw = lines.lines[i].clone();
        let (indent, code, comment) = parts(&raw);
        // The inputs node's `@x,y` stays.
        let pos = match super::split_pos(code) {
            (name, Some(_)) => code[name.len()..].to_owned(),
            (_, None) => String::new(),
        };
        lines.lines[i] = format!("{indent}fn {new}{pos}{comment}");
    }
    if heading {
        // The nearest heading above, unless a fence comes first.
        for i in (0..block.open.min(lines.lines.len())).rev() {
            let t = lines.lines[i].trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                break;
            }
            if let Some((level, title)) = crate::nodes::heading(&lines.lines[i]) {
                if title.eq_ignore_ascii_case(old.trim()) {
                    let indent = &lines.lines[i][..lines.lines[i].len() - t.len()];
                    lines.lines[i] = format!("{indent}{} {new}", "#".repeat(level));
                }
                break;
            }
        }
    }
    lines.join()
}

/// The text after ` : ` on a wire for this call.
pub fn wire_call(name: &str, args: &[Arg]) -> String {
    if args.is_empty() { name.to_owned() } else { write_call(name, args) }
}

#[cfg(test)]
mod tests {
    use super::super::{blocks, parse_call};
    use super::*;

    #[test]
    fn keeps_the_inputs_node_place_on_the_fn_line() {
        let text = "```obsession\nfn Wait For Done // waits\nin src: File\n```\n";
        let block = &blocks(text)[0];
        assert_eq!(block.function.entry, None);
        let moved = set_entry_pos(text, block, -220.4, 60.0);
        assert_eq!(moved, "```obsession\nfn Wait For Done  @-220,60 // waits\nin src: File\n```\n");
        let block = &blocks(&moved)[0];
        assert_eq!(block.function.name, "Wait For Done");
        assert_eq!(block.function.entry, Some((-220.0, 60.0)));
        // Moved again: replaced, not added.
        let again = set_entry_pos(&moved, block, 10.0, 20.0);
        assert!(again.contains("fn Wait For Done  @10,20 // waits"), "{again}");
        // A rename keeps it.
        let renamed = rename(&again, &blocks(&again)[0], "Hold", false);
        assert!(renamed.contains("fn Hold  @10,20 // waits"), "{renamed}");
        // An `@` that isn't a place stays in the name.
        assert_eq!(blocks("```obsession\nfn Ping @ika\n```\n")[0].function.name, "Ping @ika");
    }

    #[test]
    fn renames_a_step_id_and_what_reads_it() {
        let text = "```obsession\nfn Wait\nin src: File\nout dst: File\nparam task = \"\"\n\nt  = Find Task(src, task)   @40,60\ns  = Status(t)              @320,60 // the status\nok = Equal(s, \"t\")\nSet Status(dst, s, find: t)  @600,60\n```\n";
        let block = &blocks(text)[0];
        let f = &block.function;
        let out = rename_step(text, f, 0, "task_row").unwrap();
        assert!(out.contains("task_row  = Find Task(src, task)   @40,60\n"), "{out}");
        assert!(out.contains("s  = Status(task_row)              @320,60 // the status\n"), "{out}");
        // Quoted text stays; `find: t` names the argument `find`, and its
        // value `t` is the step.
        assert!(out.contains("ok = Equal(s, \"t\")\n"), "{out}");
        assert!(out.contains("Set Status(dst, s, find: task_row)  @600,60"), "{out}");
        // Still the same function, renamed.
        let again = &blocks(&out)[0].function;
        assert_eq!(again.steps[0].id.as_deref(), Some("task_row"));
        // The same problems as before (this sample has one on purpose:
        // `find` wants text), so every reference still resolves.
        let before = super::super::check::check(f).first_problem().map(str::to_owned);
        assert_eq!(super::super::check::check(again).first_problem().map(str::to_owned), before);

        assert_eq!(rename_step(text, f, 0, "t").unwrap(), text);
        assert!(rename_step(text, f, 0, "s").unwrap_err().contains("already used"));
        assert!(rename_step(text, f, 0, "src").unwrap_err().contains("already used"));
        assert!(rename_step(text, f, 0, "task").unwrap_err().contains("already used"));
        assert!(rename_step(text, f, 0, "2x").unwrap_err().contains("can't be an id"));
        assert!(rename_step(text, f, 3, "x").unwrap_err().contains("no id"));
    }

    const FILE: &str = "## Functions\r\n```obsession\r\nfn Wait\r\nin src: File\r\nout dst: File\r\n\r\n// find it\r\n  t  = Find Task(src, \"a\")   @40,60   // first\r\ns = Status(t)\r\nSet Status(dst, s, find: \"b\")\r\n\r\n```\r\nafter\r\n";

    #[test]
    fn moves_and_rewires_one_line() {
        let b = &blocks(FILE)[0];
        let f = &b.function;
        let moved = set_pos(FILE, &f.steps[0], 100.4, 7.0);
        assert_eq!(moved, FILE.replace("@40,60", "@100,7"));
        let placed = set_pos(FILE, &f.steps[1], 1.0, 2.0);
        assert_eq!(placed, FILE.replace("s = Status(t)", "s = Status(t)  @1,2"));

        // Wiring the `from` input: written by name, the rest untouched.
        let wired = set_arg(FILE, &f.steps[2], 3, Some(Value::Lit(Literal::Text("- ".into()))));
        assert_eq!(wired, FILE.replace("find: \"b\")", "find: \"b\", from: \"- \")"));
        // Clearing a required input makes the rest named.
        let cleared = set_arg(FILE, &f.steps[2], 1, None);
        assert_eq!(cleared, FILE.replace("Set Status(dst, s, find: \"b\")", "Set Status(dst, find: \"b\")"));
        // A step with a comment keeps it.
        let rewired = set_arg(FILE, &f.steps[0], 1, Some(Value::Ref("src".into())));
        assert!(rewired.contains("  t = Find Task(src, src)  @40,60   // first\r\n"), "{rewired}");
        // Default values are left out.
        let (_, args) = parse_call("Contains(a, b, ignore case: false)").unwrap();
        let spec = library::find("Contains").unwrap();
        let (values, _) = bind(spec, &args);
        assert_eq!(write_call("Contains", &args_for("Contains", &values)), "Contains(a, b)");
    }

    #[test]
    fn adds_and_removes_steps() {
        let b = &blocks(FILE)[0];
        let (added, line) = add_step(FILE, b, "find task", 10.0, 20.0);
        assert_eq!(line, 10);
        assert!(added.contains("Set Status(dst, s, find: \"b\")\r\nft = Find Task()  @10,20\r\n\r\n```"), "{added}");
        let again = &blocks(&added)[0];
        assert_eq!(fresh_id(&again.function, "Find Task"), "ft2");
        assert_eq!(fresh_id(&again.function, "Status"), "s2");

        let removed = remove_step(FILE, b, 1);
        assert_eq!(removed, FILE.replace("s = Status(t)\r\n", "").replace("Set Status(dst, s, find: \"b\")", "Set Status(dst, find: \"b\")"));

        let with_param = add_header(FILE, b, &param_line("done", &Literal::Text("x".into())));
        assert!(with_param.contains("out dst: File\r\nparam done = \"x\"\r\n\r\n"));
    }

    #[test]
    fn renames_where_its_defined() {
        let b = &blocks(FILE)[0];
        // In a graph: only the `fn` line (the heading is `## Functions`).
        assert_eq!(rename(FILE, b, "Wait Longer", true), FILE.replace("fn Wait\r\n", "fn Wait Longer\r\n"));
        // In `_funct.md`: its own heading too, but not another one's.
        let saved = "# Functions\n\n## Wait\nWaits.\n\n```obsession\n  fn Wait   // the first\nin a: File\n```\n\n## Copy\n```obsession\nfn Copy\n```\n";
        let blocks = blocks(saved);
        assert_eq!(
            rename(saved, &blocks[0], "Hold", true),
            saved.replace("## Wait\n", "## Hold\n").replace("  fn Wait   // the first", "  fn Hold   // the first")
        );
        assert_eq!(rename(saved, &blocks[0], "Hold", false), saved.replace("fn Wait ", "fn Hold "));
        // A heading with another name stays.
        let other = saved.replace("## Wait\n", "## Waiting\n");
        assert_eq!(rename(&other, &blocks[0], "Hold", true), other.replace("fn Wait ", "fn Hold "));
    }
}
