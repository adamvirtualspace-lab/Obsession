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
        let (indent, _, comment) = parts(&lines.lines[i]);
        lines.lines[i] = format!("{indent}fn {new}{comment}");
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
