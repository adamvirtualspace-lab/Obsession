//! Functions (Syntax.md, sections 8 and 9): small graphs of nodes written as
//! text in ```` ```obsession ```` code blocks, one node per line.
//!
//! - this module: reading the text format, and finding the blocks in a file.
//! - [`types`]: the value types and what each argument accepts.
//! - [`library`]: every node in the node library.
//! - [`check`]: type checking; decides whether a wire is solid or dashed.
//! - [`eval`]: runs a function in memory against real files, for live
//!   values and (in milestone 6) for runs. It never writes; actions give
//!   patches.
//! - [`edit`]: changes to a function's text that touch only the lines they
//!   must.

pub mod check;
pub mod edit;
pub mod eval;
pub mod library;
pub mod types;

use crate::nodes::Section;

/// The saved functions file at the vault root.
pub const SAVED: &str = "_funct.md";

/// What `_funct.md` holds when the app creates it.
pub const SAVED_DEFAULT: &str = "# Functions

## Wait For Done
Sets the target task to ready when the source task is finished, and back to
\"cannot start\" if it isn't (retakes). `from: \"- \"` means it only touches the
target while it's in `[-]` or `[ ]`, so a task someone already started or
finished is never reset.

```obsession
fn Wait For Done
in src: File
out dst: File
param task = \"\"
param target = \"\"
param done = \"x\"

t  = Find Task(src, task)          @40,60
s  = Status(t)                     @320,60
ok = Equal(s, done)                @600,60
st = Select(ok, \" \", \"-\")          @880,60
Set Status(dst, st, find: target, from: \"- \")  @1160,60
```

## Copy Status
Gives the target task the same status as the source task.

```obsession
fn Copy Status
in src: File
out dst: File
param task = \"\"
param target = \"\"

t = Find Task(src, task)           @40,60
s = Status(t)                      @320,60
Set Status(dst, s, find: target)   @600,60
```
";

/// What an `in` or `out` is wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    File,
    Task,
    Section,
}

impl PortKind {
    pub fn name(self) -> &'static str {
        match self {
            PortKind::File => "File",
            PortKind::Task => "Task",
            PortKind::Section => "Section",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "file" => Some(PortKind::File),
            "task" => Some(PortKind::Task),
            "section" => Some(PortKind::Section),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Text(String),
    Number(f64),
    Bool(bool),
}

impl Literal {
    /// As written in the text format.
    pub fn write(&self) -> String {
        match self {
            Literal::Text(t) => {
                let mut out = String::with_capacity(t.len() + 2);
                out.push('"');
                for c in t.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        _ => out.push(c),
                    }
                }
                out.push('"');
                out
            }
            Literal::Number(n) => number(*n),
            Literal::Bool(b) => b.to_string(),
        }
    }
}

/// A number without a pointless `.0`.
pub fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A node `id`, or an `in`, `out` or `param` name.
    Ref(String),
    Lit(Literal),
}

impl Value {
    pub fn write(&self) -> String {
        match self {
            Value::Ref(r) => r.clone(),
            Value::Lit(l) => l.write(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    /// `ignore case` in `ignore case: true`; `None` when positional.
    pub name: Option<String>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub name: String,
    pub kind: PortKind,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub default: Literal,
    pub line: usize,
}

/// One node line: `id = Node(args) @x,y`.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: Option<String>,
    pub op: String,
    pub args: Vec<Arg>,
    pub pos: Option<(f32, f32)>,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Function {
    pub name: String,
    /// The inputs node's place in the editor: `@x,y` at the end of the
    /// `fn` line.
    pub entry: Option<(f32, f32)>,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub params: Vec<Param>,
    pub steps: Vec<Step>,
    /// (line, message) for lines that couldn't be read.
    pub errors: Vec<(usize, String)>,
}

impl Function {
    /// `(in, out)` when the function can sit on a wire: exactly one of each.
    pub fn wire_shape(&self) -> Option<(PortKind, PortKind)> {
        match (self.inputs.as_slice(), self.outputs.as_slice()) {
            ([i], [o]) => Some((i.kind, o.kind)),
            _ => None,
        }
    }

    pub fn port(&self, name: &str) -> Option<&Port> {
        self.inputs.iter().chain(&self.outputs).find(|p| p.name == name)
    }

    pub fn step_by_id(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id.as_deref() == Some(id))
    }
}

/// A function's code block in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The opening fence's line.
    pub open: usize,
    /// The closing fence's line (the file's line count if it's never closed).
    pub close: usize,
    pub function: Function,
}

/// Whether `s` is a usable `id` or port name: letters, digits, `-`, `_`.
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        && !matches!(s, "true" | "false")
        && !s.starts_with(|c: char| c.is_ascii_digit() || c == '-')
}

/// Every ```` ```obsession ```` block in a file. Blocks inside other code
/// fences are skipped.
pub fn blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((fence, info)) = fence_open(lines[i]) else {
            i += 1;
            continue;
        };
        let close = (i + 1..lines.len())
            .find(|&j| fence_closes(lines[j], fence))
            .unwrap_or(lines.len());
        if info.eq_ignore_ascii_case("obsession") {
            out.push(Block {
                open: i,
                close,
                function: parse_lines(&lines[i + 1..close], i + 1),
            });
        }
        i = close + 1;
    }
    out
}

/// The blocks a graph file defines in `## Functions`.
pub fn local_blocks(text: &str) -> Vec<Block> {
    let range = crate::nodes::section_lines(text, Section::Functions);
    blocks(text)
        .into_iter()
        .filter(|b| range.contains(&b.open))
        .collect()
}

/// An opening fence: (fence chars, e.g. "```", and the info string).
fn fence_open(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let ch = t.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = t.chars().take_while(|&c| c == ch).count();
    if run < 3 {
        return None;
    }
    let info = t[run..].trim();
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some((&t[..run], info))
}

fn fence_closes(line: &str, fence: &str) -> bool {
    let t = line.trim();
    let ch = fence.chars().next().unwrap_or('`');
    t.len() >= fence.len() && t.chars().all(|c| c == ch)
}

/// Reads one function's text (without the fences). `first` is the line
/// number of the first line, so steps know where they are in the file.
pub fn parse(text: &str) -> Function {
    let lines: Vec<&str> = text.lines().collect();
    parse_lines(&lines, 0)
}

pub fn parse_lines(lines: &[&str], first: usize) -> Function {
    let mut f = Function::default();
    let mut named = false;
    for (offset, raw) in lines.iter().enumerate() {
        let line = first + offset;
        let text = strip_comment(raw).trim();
        if text.is_empty() {
            continue;
        }
        if !named {
            match text.strip_prefix("fn ") {
                Some(rest) if !rest.trim().is_empty() => {
                    let (name, entry) = split_pos(rest);
                    f.name = name.trim().to_owned();
                    f.entry = entry;
                }
                _ => f.errors.push((line, "The first line must be `fn Name`".into())),
            }
            named = true;
            continue;
        }
        if let Some(rest) = text.strip_prefix("in ").or_else(|| text.strip_prefix("out ")) {
            let input = text.starts_with("in ");
            match port(rest, line) {
                Ok(p) if input => f.inputs.push(p),
                Ok(p) => f.outputs.push(p),
                Err(e) => f.errors.push((line, e)),
            }
            continue;
        }
        if let Some(rest) = text.strip_prefix("param ") {
            match param(rest, line) {
                Ok(p) => f.params.push(p),
                Err(e) => f.errors.push((line, e)),
            }
            continue;
        }
        match step(text, line) {
            Ok(s) => f.steps.push(s),
            Err(e) => f.errors.push((line, e)),
        }
    }
    if !named {
        f.errors.push((first, "Empty function: the first line must be `fn Name`".into()));
    }
    f
}

/// Cuts a `// comment` that starts a line or follows a space, outside
/// quotes.
pub(crate) fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        if c == '"' {
            quoted = true;
        } else if line[i..].starts_with("//") && (i == 0 || line[..i].ends_with([' ', '\t'])) {
            return &line[..i];
        }
    }
    line
}

fn port(rest: &str, line: usize) -> Result<Port, String> {
    let (name, kind) = rest
        .split_once(':')
        .ok_or_else(|| format!("Expected `name: Kind` after `in`/`out`, found `{}`", rest.trim()))?;
    let name = name.trim();
    if !is_name(name) {
        return Err(format!("`{name}` isn't a usable name (letters, digits, - and _)"));
    }
    let kind = PortKind::parse(kind)
        .ok_or_else(|| format!("`{}` isn't a kind: use File, Task or Section", kind.trim()))?;
    Ok(Port {
        name: name.to_owned(),
        kind,
        line,
    })
}

fn param(rest: &str, line: usize) -> Result<Param, String> {
    let (name, value) = rest
        .split_once('=')
        .ok_or_else(|| "Expected `param name = value`".to_owned())?;
    let name = name.trim();
    if !is_name(name) {
        return Err(format!("`{name}` isn't a usable name (letters, digits, - and _)"));
    }
    match parse_value(value)? {
        Value::Lit(default) => Ok(Param {
            name: name.to_owned(),
            default,
            line,
        }),
        Value::Ref(r) => Err(format!("A param's default must be text, a number or true/false, not `{r}`")),
    }
}

/// Splits `s` at `sep` characters outside quotes and brackets.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Where the `)` matching the `(` at `open` is, outside quotes.
fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn parse_value(s: &str) -> Result<Value, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Missing value".into());
    }
    if let Some(inner) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        loop {
            match chars.next() {
                None => return Err(format!("Text without a closing quote: {s}")),
                Some('\\') => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(c) => out.push(c),
                    None => return Err(format!("Text without a closing quote: {s}")),
                },
                Some('"') => break,
                Some(c) => out.push(c),
            }
        }
        let rest: String = chars.collect();
        if !rest.trim().is_empty() {
            return Err(format!("Unexpected `{}` after text", rest.trim()));
        }
        return Ok(Value::Lit(Literal::Text(out)));
    }
    match s {
        "true" => return Ok(Value::Lit(Literal::Bool(true))),
        "false" => return Ok(Value::Lit(Literal::Bool(false))),
        _ => {}
    }
    if let Ok(n) = s.parse::<f64>()
        && n.is_finite()
    {
        return Ok(Value::Lit(Literal::Number(n)));
    }
    if is_name(s) {
        return Ok(Value::Ref(s.to_owned()));
    }
    Err(format!("`{s}` isn't a value: use a node id, a name, \"text\", a number or true/false"))
}

/// `Name(args)` or just `Name`: a node call, or a function on a wire.
pub fn parse_call(s: &str) -> Result<(String, Vec<Arg>), String> {
    let s = s.trim();
    let Some(open) = s.find('(') else {
        return Ok((s.to_owned(), Vec::new()));
    };
    let name = s[..open].trim().to_owned();
    let close = matching_paren(s, open).ok_or_else(|| format!("`{name}(` has no closing `)`"))?;
    if !s[close + 1..].trim().is_empty() {
        return Err(format!("Unexpected `{}` after `)`", s[close + 1..].trim()));
    }
    let inner = &s[open + 1..close];
    let mut args = Vec::new();
    if !inner.trim().is_empty() {
        for part in split_top(inner, ',') {
            let pieces = split_top(part, ':');
            let arg = match pieces.as_slice() {
                [value] => Arg {
                    name: None,
                    value: parse_value(value)?,
                },
                [name, value] => {
                    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
                    if name.is_empty() {
                        return Err("An argument name is missing before `:`".into());
                    }
                    Arg {
                        name: Some(name),
                        value: parse_value(value)?,
                    }
                }
                _ => return Err(format!("Too many `:` in `{}`", part.trim())),
            };
            args.push(arg);
        }
    }
    Ok((name, args))
}

/// `text` without a trailing `@x,y`, and the point, when it ends with one
/// (two numbers). Anything else after an `@` stays part of the text.
pub fn split_pos(text: &str) -> (&str, Option<(f32, f32)>) {
    let Some(at) = text.rfind('@') else { return (text, None) };
    let Some((x, y)) = text[at + 1..].split_once(',') else { return (text, None) };
    match (x.trim().parse::<f32>(), y.trim().parse::<f32>()) {
        (Ok(x), Ok(y)) if x.is_finite() && y.is_finite() => (text[..at].trim_end(), Some((x, y))),
        _ => (text, None),
    }
}

fn step(text: &str, line: usize) -> Result<Step, String> {
    // `@x,y` at the end, after the call.
    let (body, pos) = match text.rfind('@') {
        Some(at) if text[at..].find(')').is_none() && !text[at..].contains('"') => {
            let xy = text[at + 1..].trim();
            let (x, y) = xy
                .split_once(',')
                .ok_or_else(|| format!("Expected `@x,y`, found `@{xy}`"))?;
            let x: f32 = x.trim().parse().map_err(|_| format!("`{}` isn't a number", x.trim()))?;
            let y: f32 = y.trim().parse().map_err(|_| format!("`{}` isn't a number", y.trim()))?;
            (text[..at].trim_end(), Some((x, y)))
        }
        _ => (text, None),
    };
    let open = body.find('(').ok_or_else(|| format!("Expected `Node(...)`, found `{body}`"))?;
    let (id, call) = match body[..open].split_once('=') {
        Some((id, _)) => {
            let id = id.trim();
            if !is_name(id) {
                return Err(format!("`{id}` isn't a usable id (letters, digits, - and _)"));
            }
            let eq = body.find('=').unwrap_or(0);
            (Some(id.to_owned()), &body[eq + 1..])
        }
        None => (None, body),
    };
    let (op, args) = parse_call(call)?;
    if op.is_empty() {
        return Err("A node name is missing before `(`".into());
    }
    Ok(Step {
        id,
        op,
        args,
        pos,
        line,
    })
}

/// A step as one line of text, without its position.
pub fn write_call(op: &str, args: &[Arg]) -> String {
    let args: Vec<String> = args
        .iter()
        .map(|a| match &a.name {
            Some(n) => format!("{n}: {}", a.value.write()),
            None => a.value.write(),
        })
        .collect();
    format!("{op}({})", args.join(", "))
}

pub fn write_step(step: &Step) -> String {
    let call = write_call(&step.op, &step.args);
    let mut out = match &step.id {
        Some(id) => format!("{id} = {call}"),
        None => call,
    };
    if let Some((x, y)) = step.pos {
        out.push_str(&format!("  @{},{}", x.round() as i64, y.round() as i64));
    }
    out
}

/// Functions by name: the graph's own first, then the saved ones.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub local: Vec<Block>,
    pub saved: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// In the open `_nodes.md`'s `## Functions`.
    Local,
    /// In `_funct.md`.
    Saved,
}

impl Library {
    /// The function called `name` (case-insensitive), where it lives, and
    /// whether a saved function of the same name is hidden by it.
    pub fn find(&self, name: &str) -> Option<(&Block, Origin, bool)> {
        let matches = |b: &&Block| b.function.name.eq_ignore_ascii_case(name.trim());
        let saved = self.saved.iter().find(matches);
        match self.local.iter().find(matches) {
            Some(b) => Some((b, Origin::Local, saved.is_some())),
            None => saved.map(|b| (b, Origin::Saved, false)),
        }
    }

    /// Every function, local first, without the saved ones a local one
    /// hides.
    pub fn all(&self) -> Vec<(&Block, Origin)> {
        let mut out: Vec<(&Block, Origin)> = self.local.iter().map(|b| (b, Origin::Local)).collect();
        for b in &self.saved {
            if !self.local.iter().any(|l| l.function.name.eq_ignore_ascii_case(&b.function.name)) {
                out.push((b, Origin::Saved));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_function() {
        let f = parse(
            "fn Wait For Done\nin src: File\nout dst: File\nparam task = \"\"\nparam done = \"x\"\n\n// a comment\nt  = Find Task(src, task)          @40,60\nst = Select(ok, \" \", \"-\")\nSet Status(dst, st, find: target, from: \"- \")  @680,60 // the end\n",
        );
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert_eq!(f.name, "Wait For Done");
        assert_eq!(f.inputs[0].name, "src");
        assert_eq!(f.outputs[0].kind, PortKind::File);
        assert_eq!(f.params[1].default, Literal::Text("x".into()));
        assert_eq!(f.steps.len(), 3);
        assert_eq!(f.steps[0].id.as_deref(), Some("t"));
        assert_eq!(f.steps[0].op, "Find Task");
        assert_eq!(f.steps[0].pos, Some((40.0, 60.0)));
        assert_eq!(f.steps[0].line, 7);
        assert_eq!(f.steps[1].args[1].value, Value::Lit(Literal::Text(" ".into())));
        let set = &f.steps[2];
        assert_eq!(set.id, None);
        assert_eq!(set.args[2].name.as_deref(), Some("find"));
        assert_eq!(set.args[3].value, Value::Lit(Literal::Text("- ".into())));
        assert_eq!(f.wire_shape(), Some((PortKind::File, PortKind::File)));
    }

    #[test]
    fn values_and_errors() {
        let f = parse("fn X\nin a File\nparam p = other\nx = Contains(\"say \\\"hi\\\"\", \"a,b:c\", ignore   case: true)\ny = Len(\"x\"\nz = Count(list) @1\n");
        assert_eq!(f.errors.len(), 4, "{:?}", f.errors);
        let s = &f.steps[0];
        assert_eq!(s.args[0].value, Value::Lit(Literal::Text("say \"hi\"".into())));
        assert_eq!(s.args[1].value, Value::Lit(Literal::Text("a,b:c".into())));
        assert_eq!(s.args[2].name.as_deref(), Some("ignore case"));
        assert_eq!(parse_value("-2.5"), Ok(Value::Lit(Literal::Number(-2.5))));
        assert!(parse_value("a b").is_err());
        // Round trip.
        assert_eq!(
            write_step(s),
            "x = Contains(\"say \\\"hi\\\"\", \"a,b:c\", ignore case: true)"
        );
    }

    #[test]
    fn finds_blocks() {
        let text = "# G\n\n## Functions\n```obsession\nfn A\n```\n\n````md\n```obsession\nfn Hidden\n```\n````\n\n~~~ obsession\nfn B\n";
        let found = blocks(text);
        let names: Vec<&str> = found.iter().map(|b| b.function.name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!((found[0].open, found[0].close), (3, 5));
        assert_eq!(found[1].close, text.lines().count());
        assert_eq!(local_blocks(text).len(), 2);

        let saved = blocks(SAVED_DEFAULT);
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().all(|b| b.function.errors.is_empty()));
        let lib = Library {
            local: blocks("```obsession\nfn copy status\n```\n"),
            saved,
        };
        let (_, origin, shadowed) = lib.find("Copy Status").unwrap();
        assert_eq!((origin, shadowed), (Origin::Local, true));
        assert_eq!(lib.find("wait for done").unwrap().1, Origin::Saved);
        assert_eq!(lib.all().len(), 2);
    }
}
