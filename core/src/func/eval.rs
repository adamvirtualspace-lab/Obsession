//! Runs a function in memory against the files it's wired to (Syntax.md,
//! "Types" and "Node library"). Gives every node's value, which the editor
//! shows on hover (live values), and the patches the actions would make.
//! Nothing is written here; milestone 6 applies the patches.

use super::check::{Checked, Source};
use super::library::{self, Spec};
use super::{Function, Literal, PortKind, Value as ArgValue, number};
use crate::nodes::heading;
use crate::status::{self, Format};
use crate::syntax::{LineState, scan_line};

/// The file a port is wired to.
#[derive(Debug, Clone)]
pub struct Bind {
    pub rel: String,
    pub text: String,
    /// The heading (Section ports) or `^id` (Task ports) after `#`.
    pub sub: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskVal {
    pub status: char,
    pub text: String,
    pub people: Vec<String>,
    pub id: Option<String>,
    /// Where it is when it was read from a wired file: (port, line, byte
    /// offset of `[`).
    pub at: Option<(usize, usize, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Text(String),
    Number(f64),
    Bool(bool),
    Task(TaskVal),
    TextList(Vec<String>),
    TaskList(Vec<TaskVal>),
    /// A wired file (by port: the inputs, then the outputs).
    File(usize),
    /// A wired heading's section (by port).
    Section(usize),
}

/// A change an action would make: replace `old` lines at `line` with
/// `new` (an insert has no `old`).
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    pub port: usize,
    pub rel: String,
    pub line: usize,
    pub old: Vec<String>,
    pub new: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Run {
    /// Each step's value; for an action, what it would do. `Err` is "no
    /// value" with the reason.
    pub values: Vec<Result<Val, String>>,
    pub patches: Vec<Patch>,
}

struct Cx<'a> {
    f: &'a Function,
    binds: &'a [Option<Bind>],
    team: Option<&'a Format>,
}

impl Cx<'_> {
    fn port_name(&self, port: usize) -> &str {
        let ins = self.f.inputs.len();
        if port < ins {
            &self.f.inputs[port].name
        } else {
            &self.f.outputs[port - ins].name
        }
    }

    fn bind(&self, port: usize) -> Result<&Bind, String> {
        self.binds
            .get(port)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("`{}` isn't wired to anything", self.port_name(port)))
    }

    /// Text, and where its first line is in a wired file (if it's from one).
    fn text(&self, v: &Val) -> Result<(String, Option<(usize, usize)>), String> {
        match v {
            Val::Text(t) => Ok((t.clone(), None)),
            Val::Number(n) => Ok((number(*n), None)),
            Val::Bool(b) => Ok((b.to_string(), None)),
            Val::File(p) => {
                let b = self.bind(*p)?;
                let lines: Vec<&str> = b.text.lines().collect();
                let start = body_start(&lines);
                Ok((lines[start..].join("\n"), Some((*p, start))))
            }
            Val::Section(p) => {
                let b = self.bind(*p)?;
                let lines: Vec<&str> = b.text.lines().collect();
                let heading = b.sub.as_deref().unwrap_or("");
                let (start, end) = section_range(&lines, heading)
                    .ok_or_else(|| format!("{} has no heading `{heading}`", b.rel))?;
                Ok((lines[start..end].join("\n"), Some((*p, start))))
            }
            Val::Task(t) => Ok((t.text.clone(), None)),
            Val::TextList(_) | Val::TaskList(_) => Err("a list isn't text".into()),
        }
    }

    fn plain(&self, v: &Val) -> Result<String, String> {
        self.text(v).map(|(t, _)| t)
    }

    fn tasks(&self, v: &Val) -> Result<Vec<TaskVal>, String> {
        if let Val::TaskList(list) = v {
            return Ok(list.clone());
        }
        let (text, origin) = self.text(v)?;
        Ok(read_tasks(&text, origin))
    }
}

/// The first line after the frontmatter.
fn body_start(lines: &[&str]) -> usize {
    if lines.first().is_some_and(|l| l.trim_start_matches('\u{feff}').trim_end() == "---") {
        for (i, l) in lines.iter().enumerate().skip(1) {
            if matches!(l.trim_end(), "---" | "...") {
                return i + 1;
            }
        }
    }
    0
}

/// The lines under a heading (case-insensitive, any level): up to the next
/// heading of the same or a higher level. `(start, end)`, end exclusive.
pub fn section_range(lines: &[&str], wanted: &str) -> Option<(usize, usize)> {
    let wanted = wanted.trim().trim_start_matches('#').trim();
    let mut state = LineState::Start;
    let mut found: Option<(usize, usize)> = None; // (level, start)
    for (i, line) in lines.iter().enumerate() {
        let code = matches!(state, LineState::Fence { .. } | LineState::Frontmatter | LineState::Comment);
        state = scan_line(state, line).1;
        if code {
            continue;
        }
        let Some((level, title)) = heading(line) else { continue };
        match found {
            Some((open, start)) if level <= open => return Some((start, i)),
            None if title.eq_ignore_ascii_case(wanted) => found = Some((level, i + 1)),
            _ => {}
        }
    }
    found.map(|(_, start)| (start, lines.len()))
}

/// Text after a task's box: (text, people, id).
fn task_parts(after: &str) -> (String, Vec<String>, Option<String>) {
    let (body, id) = match crate::live::trailing_block_id(after) {
        Some(cut) => (&after[..cut], Some(after[cut..].trim().trim_start_matches('^').to_owned())),
        None => (after, None),
    };
    let mut people = Vec::new();
    let mut words = Vec::new();
    for word in body.split_whitespace() {
        match word.strip_prefix('@') {
            Some(name) if !name.is_empty() => {
                people.push(name.trim_end_matches([',', '.', ';', ':']).to_owned());
            }
            _ => words.push(word),
        }
    }
    (words.join(" "), people, id)
}

/// The tasks in `text`. `origin` is (port, line of text's first line).
fn read_tasks(text: &str, origin: Option<(usize, usize)>) -> Vec<TaskVal> {
    let lines: Vec<&str> = text.lines().collect();
    status::tasks(text)
        .into_iter()
        .map(|(line, at, ch)| {
            let raw = lines.get(line).copied().unwrap_or("");
            let after = raw.get(at + 2 + ch.len_utf8()..).unwrap_or("").trim();
            let (text, people, id) = task_parts(after);
            TaskVal {
                status: ch,
                text,
                people,
                id,
                at: origin.map(|(p, first)| (p, first + line, at)),
            }
        })
        .collect()
}

/// The first task whose text contains `find` (ignoring case), or whose ID
/// is `find` when it starts with `^`. An empty `find` matches the first.
fn find_task<'t>(tasks: &'t [TaskVal], find: &str) -> Option<&'t TaskVal> {
    let find = find.trim();
    if let Some(id) = find.strip_prefix('^') {
        return tasks.iter().find(|t| t.id.as_deref().is_some_and(|i| i.eq_ignore_ascii_case(id)));
    }
    let lower = find.to_lowercase();
    tasks.iter().find(|t| t.text.to_lowercase().contains(&lower))
}

fn fold(s: &str, ignore: bool) -> String {
    if ignore { s.to_lowercase() } else { s.to_owned() }
}

fn wildcard(text: &[char], pattern: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| wildcard(&text[i..], rest)),
        Some(('?', rest)) => !text.is_empty() && wildcard(&text[1..], rest),
        Some((c, rest)) => text.first() == Some(c) && wildcard(&text[1..], rest),
    }
}

fn replace_all(text: &str, from: &str, to: &str, ignore: bool) -> String {
    if !ignore {
        return text.replace(from, to);
    }
    let lower = text.to_lowercase();
    let needle = from.to_lowercase();
    if lower.len() != text.len() {
        // Lower-casing changed byte lengths; fall back to an exact match.
        return text.replace(from, to);
    }
    let mut out = String::new();
    let mut last = 0;
    for (i, _) in lower.match_indices(&needle) {
        out.push_str(&text[last..i]);
        out.push_str(to);
        last = i + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

/// Runs `f`. `binds` has one entry per port (inputs, then outputs);
/// `params` one per param (`None` for its default).
pub fn eval(f: &Function, c: &Checked, binds: &[Option<Bind>], params: &[Option<Literal>], team: Option<&Format>) -> Run {
    let cx = Cx { f, binds, team };
    let mut run = Run {
        values: vec![Err("not checked".into()); f.steps.len()],
        patches: Vec::new(),
    };
    let broken: Vec<bool> = (0..f.steps.len()).map(|i| c.step_problems(i).next().is_some()).collect();
    for &i in &c.order {
        let step = &f.steps[i];
        let Some(spec) = library::find(&step.op) else { continue };
        if broken[i] {
            run.values[i] = Err(c.step_problems(i).next().map_or_else(String::new, |p| p.message.clone()));
            continue;
        }
        let mut args = Vec::with_capacity(spec.inputs.len());
        let mut missing = None;
        for (k, input) in spec.inputs.iter().enumerate() {
            let given = c.bound[i].get(k).cloned().flatten();
            let v = match given {
                None => Ok(lit(&input.default.literal().unwrap_or(Literal::Text(String::new())))),
                Some(ArgValue::Lit(l)) => Ok(lit(&l)),
                Some(ArgValue::Ref(name)) => resolve(&cx, c, &run, params, &name),
            };
            match v {
                Ok(v) => args.push(v),
                Err(e) => {
                    missing.get_or_insert(e);
                    args.push(Val::Text(String::new()));
                }
            }
        }
        run.values[i] = match missing {
            Some(e) => Err(e),
            None => apply(&cx, spec, &args, &mut run.patches),
        };
    }
    run
}

fn lit(l: &Literal) -> Val {
    match l {
        Literal::Text(t) => Val::Text(t.clone()),
        Literal::Number(n) => Val::Number(*n),
        Literal::Bool(b) => Val::Bool(*b),
    }
}

fn resolve(cx: &Cx, c: &Checked, run: &Run, params: &[Option<Literal>], name: &str) -> Result<Val, String> {
    match c.source(name) {
        Some(Source::In(p)) => port_value(cx, p),
        Some(Source::Out(p)) => port_value(cx, cx.f.inputs.len() + p),
        Some(Source::Param(p)) => Ok(lit(params
            .get(p)
            .cloned()
            .flatten()
            .as_ref()
            .unwrap_or(&cx.f.params[p].default))),
        Some(Source::Step(j)) => run.values[j].clone(),
        None => Err(format!("Nothing is called `{name}`")),
    }
}

fn port_value(cx: &Cx, port: usize) -> Result<Val, String> {
    let ins = cx.f.inputs.len();
    let kind = if port < ins { cx.f.inputs[port].kind } else { cx.f.outputs[port - ins].kind };
    let b = cx.bind(port)?;
    match kind {
        PortKind::File => Ok(Val::File(port)),
        PortKind::Section => {
            let lines: Vec<&str> = b.text.lines().collect();
            let heading = b.sub.as_deref().unwrap_or("");
            section_range(&lines, heading)
                .map(|_| Val::Section(port))
                .ok_or_else(|| format!("{} has no heading `{heading}`", b.rel))
        }
        PortKind::Task => {
            let id = b.sub.as_deref().unwrap_or("");
            let tasks = read_tasks(&b.text, Some((port, 0)));
            find_task(&tasks, &format!("^{}", id.trim_start_matches('^')))
                .cloned()
                .map(Val::Task)
                .ok_or_else(|| format!("{} has no task {id}", b.rel))
        }
    }
}

fn apply(cx: &Cx, spec: &Spec, a: &[Val], patches: &mut Vec<Patch>) -> Result<Val, String> {
    let text = |i: usize| cx.plain(&a[i]);
    let num = |i: usize| match &a[i] {
        Val::Number(n) => Ok(*n),
        _ => Err("not a number".to_owned()),
    };
    let flag = |i: usize| matches!(a.get(i), Some(Val::Bool(true)));
    let items = |i: usize| -> Result<Vec<String>, String> {
        match &a[i] {
            Val::TextList(l) => Ok(l.clone()),
            Val::TaskList(l) => Ok(l.iter().map(|t| t.text.clone()).collect()),
            _ => Err("not a list".into()),
        }
    };
    let len = |v: &Val| match v {
        Val::TextList(l) => l.len(),
        Val::TaskList(l) => l.len(),
        _ => 0,
    };
    let item = |v: &Val, i: usize| match v {
        Val::TextList(l) => l.get(i).cloned().map(Val::Text),
        Val::TaskList(l) => l.get(i).cloned().map(Val::Task),
        _ => None,
    };
    let chars = |s: &str| s.chars().collect::<Vec<char>>();

    Ok(match spec.name {
        "Contains" => Val::Bool(fold(&text(0)?, flag(2)).contains(&fold(&text(1)?, flag(2)))),
        "Starts With" => Val::Bool(fold(&text(0)?, flag(2)).starts_with(&fold(&text(1)?, flag(2)))),
        "Ends With" => Val::Bool(fold(&text(0)?, flag(2)).ends_with(&fold(&text(1)?, flag(2)))),
        "Equal" => Val::Bool(fold(&text(0)?, flag(2)) == fold(&text(1)?, flag(2))),
        "Is Empty" => Val::Bool(text(0)?.trim().is_empty()),
        "Len" => Val::Number(text(0)?.chars().count() as f64),
        "Left" => Val::Text(text(0)?.chars().take(num(1)?.max(0.0) as usize).collect()),
        "Right" => {
            let c = chars(&text(0)?);
            let n = (num(1)?.max(0.0) as usize).min(c.len());
            Val::Text(c[c.len() - n..].iter().collect())
        }
        "Mid" => Val::Text(
            text(0)?
                .chars()
                .skip(num(1)?.max(0.0) as usize)
                .take(num(2)?.max(0.0) as usize)
                .collect(),
        ),
        "Find" => {
            let hay = fold(&text(0)?, flag(2));
            let needle = fold(&text(1)?, flag(2));
            Val::Number(hay.find(&needle).map_or(-1.0, |b| hay[..b].chars().count() as f64))
        }
        "Replace" => {
            let from = text(1)?;
            if from.is_empty() {
                return Err("Replace's `from` is empty".into());
            }
            Val::Text(replace_all(&text(0)?, &from, &text(2)?, flag(3)))
        }
        "Trim" => Val::Text(text(0)?.trim().to_owned()),
        "Lower" => Val::Text(text(0)?.to_lowercase()),
        "Upper" => Val::Text(text(0)?.to_uppercase()),
        "Matches Wildcard" => Val::Bool(wildcard(&chars(&text(0)?), &chars(&text(1)?))),
        "Join Text" => Val::Text(format!("{}{}{}", text(0)?, text(2)?, text(1)?)),
        "Split" => {
            let by = text(1)?;
            if by.is_empty() {
                return Err("Split's `by` is empty".into());
            }
            Val::TextList(text(0)?.split(by.as_str()).map(str::to_owned).collect())
        }
        "Lines" => Val::TextList(text(0)?.lines().map(str::to_owned).collect()),

        "Count" => Val::Number(len(&a[0]) as f64),
        "Get" => {
            let i = num(1)?;
            let n = len(&a[0]);
            if i < 0.0 || i.fract() != 0.0 {
                return Err(format!("Get: {} isn't a position (0, 1, 2…)", number(i)));
            }
            item(&a[0], i as usize).ok_or_else(|| format!("Get: position {} is past the end ({n} items)", number(i)))?
        }
        "First" => item(&a[0], 0).ok_or("First: the list is empty")?,
        "Last" => {
            let n = len(&a[0]);
            n.checked_sub(1)
                .and_then(|i| item(&a[0], i))
                .ok_or("Last: the list is empty")?
        }
        "Filter Contains" => {
            let find = fold(&text(1)?, flag(2));
            match &a[0] {
                Val::TextList(l) => Val::TextList(l.iter().filter(|t| fold(t, flag(2)).contains(&find)).cloned().collect()),
                Val::TaskList(l) => Val::TaskList(l.iter().filter(|t| fold(&t.text, flag(2)).contains(&find)).cloned().collect()),
                _ => return Err("not a list".into()),
            }
        }
        "Any Contains" => {
            let find = fold(&text(1)?, flag(2));
            Val::Bool(items(0)?.iter().any(|t| fold(t, flag(2)).contains(&find)))
        }
        "All Contains" => {
            let find = fold(&text(1)?, flag(2));
            Val::Bool(items(0)?.iter().all(|t| fold(t, flag(2)).contains(&find)))
        }
        "Join" => Val::Text(items(0)?.join(&text(1)?)),

        "Not" => Val::Bool(!flag(0)),
        "And" => Val::Bool(flag(0) && flag(1)),
        "Or" => Val::Bool(flag(0) || flag(1)),
        "Select" => {
            if flag(0) {
                a[1].clone()
            } else {
                a[2].clone()
            }
        }
        "Compare" => {
            let (x, op, y) = (text(0)?, text(1)?, text(2)?);
            let order = match (x.trim().parse::<f64>(), y.trim().parse::<f64>()) {
                (Ok(p), Ok(q)) => p.partial_cmp(&q).unwrap_or(std::cmp::Ordering::Equal),
                _ => x.cmp(&y),
            };
            use std::cmp::Ordering::*;
            Val::Bool(match op.trim() {
                "=" | "==" => order == Equal,
                "<" => order == Less,
                ">" => order == Greater,
                "<=" => order != Greater,
                ">=" => order != Less,
                other => return Err(format!("Compare: `{other}` isn't =, <, >, <= or >=")),
            })
        }

        "Tasks" => Val::TaskList(cx.tasks(&a[0])?),
        "Find Task" => {
            let find = text(1)?;
            let tasks = cx.tasks(&a[0])?;
            match find_task(&tasks, &find) {
                Some(t) => Val::Task(t.clone()),
                None if find.trim().is_empty() => return Err("Find Task: there are no tasks".into()),
                None => return Err(format!("Find Task: no task matches \"{}\"", find.trim())),
            }
        }
        "Status" => match &a[0] {
            Val::Task(t) => Val::Text(t.status.to_string()),
            _ => return Err("not a task".into()),
        },
        "Task Text" => match &a[0] {
            Val::Task(t) => Val::Text(t.text.clone()),
            _ => return Err("not a task".into()),
        },
        "People" => match &a[0] {
            Val::Task(t) => Val::TextList(t.people.clone()),
            _ => return Err("not a task".into()),
        },
        "Count Status" | "All Status" => {
            let tasks = cx.tasks(&a[0])?;
            let want = text(1)?;
            let n = tasks.iter().filter(|t| t.status.to_string() == want).count();
            if spec.name == "Count Status" {
                Val::Number(n as f64)
            } else {
                Val::Bool(n == tasks.len())
            }
        }
        "Section" => {
            let (body, _) = cx.text(&a[0])?;
            let heading = text(1)?;
            let lines: Vec<&str> = body.lines().collect();
            let (s, e) = section_range(&lines, &heading).ok_or_else(|| format!("Section: no heading `{}`", heading.trim()))?;
            Val::Text(lines[s..e].join("\n"))
        }
        "Frontmatter" => {
            let Val::File(p) = &a[0] else {
                return Err("not a file".into());
            };
            let b = cx.bind(*p)?;
            let key = text(1)?;
            frontmatter(&b.text, key.trim()).ok_or_else(|| format!("{} has no `{}` property", b.rel, key.trim()))?
        }

        "Set Status" => return set_status(cx, a, patches),
        "Replace Text" => return replace_text(cx, a, patches),
        "Ensure Line" => return ensure_line(cx, a, patches),
        "Write Section" => return write_section(cx, a, patches),
        other => return Err(format!("{other} isn't implemented")),
    })
}

fn frontmatter(text: &str, key: &str) -> Option<Val> {
    let lines: Vec<&str> = text.lines().collect();
    let end = body_start(&lines);
    for line in lines.get(1..end.saturating_sub(1))? {
        let Some((k, v)) = line.split_once(':') else { continue };
        if k.trim().eq_ignore_ascii_case(key) && !line.starts_with([' ', '\t']) {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v);
            return Some(Val::Text(v.to_owned()));
        }
    }
    None
}

const SKIPPED: &str = "Doesn't fire: `when` is false";

/// The lines an action may change: (port, first, end exclusive).
fn target_range(cx: &Cx, v: &Val) -> Result<(usize, usize, usize), String> {
    match v {
        Val::File(p) => {
            let b = cx.bind(*p)?;
            let lines: Vec<&str> = b.text.lines().collect();
            Ok((*p, body_start(&lines), lines.len()))
        }
        Val::Section(p) => {
            let b = cx.bind(*p)?;
            let lines: Vec<&str> = b.text.lines().collect();
            let heading = b.sub.as_deref().unwrap_or("");
            let (s, e) = section_range(&lines, heading).ok_or_else(|| format!("{} has no heading `{heading}`", b.rel))?;
            Ok((*p, s, e))
        }
        _ => Err("the target must be a file or a section".into()),
    }
}

fn set_status(cx: &Cx, a: &[Val], patches: &mut Vec<Patch>) -> Result<Val, String> {
    if !matches!(a[4], Val::Bool(true)) {
        return Ok(Val::Text(SKIPPED.into()));
    }
    let new = cx.plain(&a[1])?;
    let mut chars = new.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return Err(format!("Set Status: a status is one character, not \"{new}\""));
    };
    let find = cx.plain(&a[2])?;
    let from = cx.plain(&a[3])?;
    let task = match &a[0] {
        Val::File(p) => {
            let b = cx.bind(*p)?;
            let tasks = read_tasks(&b.text, Some((*p, 0)));
            match find_task(&tasks, &find) {
                Some(t) => t.clone(),
                None if find.trim().is_empty() => return Err(format!("Set Status: {} has no tasks", b.rel)),
                None => return Err(format!("Set Status: no task in {} matches \"{}\"", b.rel, find.trim())),
            }
        }
        Val::Task(t) => t.clone(),
        _ => return Err("Set Status: the target must be a file or a task".into()),
    };
    let Some((port, line, at)) = task.at else {
        return Err("Set Status: that task isn't in a file this function can change".into());
    };
    let b = cx.bind(port)?;
    let lookup = status::from_frontmatter(b.text.lines());
    let (format, _) = status::resolve(lookup.as_ref(), cx.team);
    if format.index_of(ch).is_none() {
        return Err(format!("Set Status: [{ch}] isn't a status in {}", b.rel));
    }
    let name = if task.text.is_empty() { "the task".to_owned() } else { format!("\"{}\"", task.text) };
    if task.status == ch {
        return Ok(Val::Text(format!("No change: {name} is already [{ch}]")));
    }
    if !from.is_empty() && !from.contains(task.status) {
        return Ok(Val::Text(format!("Leaves {name} alone: it's [{}], not one of [{from}]", task.status)));
    }
    let raw = b.text.lines().nth(line).unwrap_or("");
    let start = at + 1;
    let end = start + task.status.len_utf8();
    let changed = format!("{}{ch}{}", &raw[..start], &raw[end..]);
    patches.push(Patch {
        port,
        rel: b.rel.clone(),
        line,
        old: vec![raw.to_owned()],
        new: vec![changed],
    });
    Ok(Val::Text(format!("Sets {name} in {} from [{}] to [{ch}]", b.rel, task.status)))
}

fn replace_text(cx: &Cx, a: &[Val], patches: &mut Vec<Patch>) -> Result<Val, String> {
    if !matches!(a[3], Val::Bool(true)) {
        return Ok(Val::Text(SKIPPED.into()));
    }
    let (from, to) = (cx.plain(&a[1])?, cx.plain(&a[2])?);
    if from.is_empty() {
        return Err("Replace Text: `from` is empty".into());
    }
    if from.contains('\n') || to.contains('\n') {
        return Err("Replace Text: `from` and `to` must be one line".into());
    }
    let (port, start, end) = target_range(cx, &a[0])?;
    let b = cx.bind(port)?;
    let mut count = 0;
    for (i, line) in b.text.lines().enumerate().take(end).skip(start) {
        if line.contains(&from) {
            if to.contains(&from) {
                return Err("Replace Text: `to` contains `from`, so every run would change it again".into());
            }
            count += 1;
            patches.push(Patch {
                port,
                rel: b.rel.clone(),
                line: i,
                old: vec![line.to_owned()],
                new: vec![line.replace(&from, &to)],
            });
        }
    }
    Ok(Val::Text(match count {
        0 => format!("No change: \"{from}\" isn't in {}", b.rel),
        1 => format!("Changes 1 line in {}", b.rel),
        n => format!("Changes {n} lines in {}", b.rel),
    }))
}

fn ensure_line(cx: &Cx, a: &[Val], patches: &mut Vec<Patch>) -> Result<Val, String> {
    if !matches!(a[2], Val::Bool(true)) {
        return Ok(Val::Text(SKIPPED.into()));
    }
    let line = cx.plain(&a[1])?;
    if line.trim().is_empty() || line.contains('\n') {
        return Err("Ensure Line: the line must be one non-empty line".into());
    }
    let (port, start, end) = target_range(cx, &a[0])?;
    let b = cx.bind(port)?;
    let lines: Vec<&str> = b.text.lines().collect();
    if lines[start..end].iter().any(|l| l.trim() == line.trim()) {
        return Ok(Val::Text(format!("No change: the line is already in {}", b.rel)));
    }
    let at = (start..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(start, |i| i + 1);
    patches.push(Patch {
        port,
        rel: b.rel.clone(),
        line: at,
        old: Vec::new(),
        new: vec![line.clone()],
    });
    Ok(Val::Text(format!("Adds \"{}\" to {}", line.trim(), b.rel)))
}

fn write_section(cx: &Cx, a: &[Val], patches: &mut Vec<Patch>) -> Result<Val, String> {
    if !matches!(a[2], Val::Bool(true)) {
        return Ok(Val::Text(SKIPPED.into()));
    }
    let text = cx.plain(&a[1])?;
    let (port, start, end) = target_range(cx, &a[0])?;
    let b = cx.bind(port)?;
    let lines: Vec<&str> = b.text.lines().collect();
    // Keep the blank lines before the next heading.
    let content_end = (start..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(start, |i| i + 1);
    let old: Vec<String> = lines[start..content_end].iter().map(|l| (*l).to_owned()).collect();
    let new: Vec<String> = text.lines().map(str::to_owned).collect();
    if old == new {
        return Ok(Val::Text(format!("No change: the section in {} already says that", b.rel)));
    }
    patches.push(Patch {
        port,
        rel: b.rel.clone(),
        line: start,
        old,
        new,
    });
    Ok(Val::Text(format!("Rewrites the section in {}", b.rel)))
}

/// Applies patches to `text` (all made against it), newest line first.
/// A patch whose old lines are no longer there is skipped. Line endings
/// are kept.
pub fn apply_patches(text: &str, patches: &[&Patch]) -> String {
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    let mut sorted: Vec<&&Patch> = patches.iter().collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.line));
    for p in sorted {
        let end = p.line + p.old.len();
        if end > lines.len() || lines[p.line..end] != p.old[..] {
            continue;
        }
        lines.splice(p.line..end, p.new.iter().cloned());
    }
    lines.join(ending)
}

impl Val {
    /// A short description for hovering a pin.
    pub fn show(&self, binds: &[Option<Bind>]) -> String {
        fn cut(s: &str) -> String {
            let one: String = s.replace('\n', " ⏎ ");
            if one.chars().count() > 60 {
                format!("{}…", one.chars().take(59).collect::<String>())
            } else {
                one
            }
        }
        let rel = |p: &usize| binds.get(*p).and_then(Option::as_ref).map_or("?", |b| b.rel.as_str());
        match self {
            Val::Text(t) => format!("\"{}\"", cut(t)),
            Val::Number(n) => number(*n),
            Val::Bool(b) => b.to_string(),
            Val::Task(t) => format!("[{}] {}", t.status, cut(&t.text)),
            Val::TextList(l) => {
                let shown: Vec<String> = l.iter().take(3).map(|t| format!("\"{}\"", cut(t))).collect();
                let more = if l.len() > 3 { ", …" } else { "" };
                format!("{} item{}: {}{more}", l.len(), if l.len() == 1 { "" } else { "s" }, shown.join(", "))
            }
            Val::TaskList(l) => {
                let shown: Vec<String> = l.iter().take(3).map(|t| format!("[{}] {}", t.status, cut(&t.text))).collect();
                let more = if l.len() > 3 { ", …" } else { "" };
                format!("{} task{}: {}{more}", l.len(), if l.len() == 1 { "" } else { "s" }, shown.join(", "))
            }
            Val::File(p) => format!("File {}", rel(p)),
            Val::Section(p) => {
                let sub = binds.get(*p).and_then(Option::as_ref).and_then(|b| b.sub.clone()).unwrap_or_default();
                format!("Section {}#{sub}", rel(p))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::check::check;
    use super::super::parse;
    use super::*;

    const WAIT: &str = "fn Wait For Done\nin src: File\nout dst: File\nparam task = \"\"\nparam target = \"\"\nparam done = \"x\"\n\nt  = Find Task(src, task)\ns  = Status(t)\nok = Equal(s, done)\nst = Select(ok, \" \", \"-\")\nSet Status(dst, st, find: target, from: \"- \")\n";

    fn bind(rel: &str, text: &str) -> Option<Bind> {
        Some(Bind {
            rel: rel.into(),
            text: text.into(),
            sub: None,
        })
    }

    fn run(src: &str, binds: &[Option<Bind>], params: &[Option<Literal>]) -> Run {
        let f = parse(src);
        let c = check(&f);
        assert!(c.ok(), "{:?}", c.problems);
        eval(&f, &c, binds, params, None)
    }

    #[test]
    fn wait_for_done() {
        let lighting = "---\nowner: sam\n---\n# Lighting\n- [x] Render lighting pass @sam ^l1\n- [ ] Notes\n";
        let comp = "# Comp\n- [-] Comp sh040 @ana\n- [ ] Other\n";
        let params = [Some(Literal::Text("render".into())), Some(Literal::Text("Comp sh040".into())), None];
        let binds = [bind("lighting/sh040.md", lighting), bind("comp/sh040.md", comp)];
        let r = run(WAIT, &binds, &params);
        let Ok(Val::Task(t)) = &r.values[0] else { panic!("{:?}", r.values[0]) };
        assert_eq!((t.status, t.text.as_str(), t.id.as_deref()), ('x', "Render lighting pass", Some("l1")));
        assert_eq!(t.people, ["sam"]);
        assert_eq!(t.at, Some((0, 4, 2)));
        assert_eq!(r.values[2], Ok(Val::Bool(true)));
        assert_eq!(r.patches.len(), 1);
        let p = &r.patches[0];
        assert_eq!((p.port, p.line), (1, 1));
        assert_eq!(p.new, ["- [ ] Comp sh040 @ana"]);

        // Run twice: no change.
        let after = apply_patches(comp, &[p]);
        assert_eq!(after, "# Comp\n- [ ] Comp sh040 @ana\n- [ ] Other\n");
        let binds2 = [bind("lighting/sh040.md", lighting), bind("comp/sh040.md", &after)];
        let r2 = run(WAIT, &binds2, &params);
        assert!(r2.patches.is_empty());
        assert_eq!(r2.values[4], Ok(Val::Text("No change: \"Comp sh040\" is already [ ]".into())));

        // Work that already moved on is left alone.
        let started = "# Comp\n- [/] Comp sh040\n";
        let binds3 = [bind("lighting/sh040.md", lighting), bind("comp/sh040.md", started)];
        assert!(run(WAIT, &binds3, &params).patches.is_empty());

        // No such task: no value, and the action doesn't fire.
        let none = [Some(Literal::Text("nothing".into())), None, None];
        let r4 = run(WAIT, &binds, &none);
        assert_eq!(r4.values[0], Err("Find Task: no task matches \"nothing\"".into()));
        assert_eq!(r4.values[4], Err("Find Task: no task matches \"nothing\"".into()));
        assert!(r4.patches.is_empty());

        // Not wired.
        let r5 = run(WAIT, &[None, None], &params);
        assert_eq!(r5.values[0], Err("`src` isn't wired to anything".into()));
    }

    #[test]
    fn text_and_lists() {
        let src = "fn T\nin a: File\nout b: File\nl = Lines(a)\nn = Count(l)\ng = Get(l, 5)\nf = Filter Contains(l, \"SH\", ignore case: true)\nj = Join(f, \"|\")\nw = Matches Wildcard(\"sh040\", \"sh0*\")\nm = Mid(\"abcdef\", 2, 3)\nr = Right(\"abcdef\", 2)\nfi = Find(\"héllo\", \"l\")\nc = Compare(\"10\", \"<\", \"9\")\ns = Section(a, \"Notes\")\nfm = Frontmatter(a, \"owner\")\nEnsure Line(b, \"- [ ] Follow up\")\n";
        let a = "---\nowner: \"sam\"\n---\nsh040 water\nsh050\n## Notes\nnote one\n## End\n";
        let r = run(src, &[bind("a.md", a), bind("b.md", "# B\n\n")], &[]);
        assert_eq!(r.values[1], Ok(Val::Number(5.0)));
        assert_eq!(r.values[2], Err("Get: position 5 is past the end (5 items)".into()));
        assert_eq!(r.values[4], Ok(Val::Text("sh040 water|sh050".into())));
        assert_eq!(r.values[5], Ok(Val::Bool(true)));
        assert_eq!(r.values[6], Ok(Val::Text("cde".into())));
        assert_eq!(r.values[7], Ok(Val::Text("ef".into())));
        assert_eq!(r.values[8], Ok(Val::Number(2.0)));
        assert_eq!(r.values[9], Ok(Val::Bool(false)));
        assert_eq!(r.values[10], Ok(Val::Text("note one".into())));
        assert_eq!(r.values[11], Ok(Val::Text("sam".into())));
        assert_eq!(r.patches[0].line, 1);
        assert_eq!(apply_patches("# B\n\n", &[&r.patches[0]]), "# B\n- [ ] Follow up\n\n");
    }

    #[test]
    fn section_actions() {
        let src = "fn S\nin a: File\nout notes: Section\nWrite Section(notes, \"one\\ntwo\")\nReplace Text(notes, \"old\", \"new\")\n";
        let text = "# Shot\r\n## Notes\r\nold text\r\n\r\n## Next\r\nold stays\r\n";
        let notes = Some(Bind {
            rel: "s.md".into(),
            text: text.into(),
            sub: Some("notes".into()),
        });
        let r = run(src, &[bind("a.md", ""), notes.clone()], &[]);
        assert_eq!(r.patches.len(), 2);
        let written = apply_patches(text, &[&r.patches[0]]);
        assert_eq!(written, "# Shot\r\n## Notes\r\none\r\ntwo\r\n\r\n## Next\r\nold stays\r\n");
        assert_eq!(r.patches[1].new, ["new text"]);

        let src = "fn S\nin a: File\nout b: File\nReplace Text(b, \"a\", \"aa\")\n";
        let r = run(src, &[bind("a.md", ""), bind("b.md", "a\n")], &[]);
        assert_eq!(r.values[0], Err("Replace Text: `to` contains `from`, so every run would change it again".into()));
    }

    #[test]
    fn statuses_must_be_in_the_format() {
        let src = "fn S\nin a: File\nout b: File\nSet Status(b, \"q\")\n";
        let r = run(src, &[bind("a.md", ""), bind("b.md", "- [ ] One\n")], &[]);
        assert_eq!(r.values[0], Err("Set Status: [q] isn't a status in b.md".into()));
        assert!(r.patches.is_empty());
        let src = "fn S\nin a: Task\nout b: File\nx = Task Text(a)\nSet Status(b, \"x\", when: false)\n";
        let task = Some(Bind {
            rel: "a.md".into(),
            text: "- [ ] Hello @bo ^t2\n".into(),
            sub: Some("^t2".into()),
        });
        let r = run(src, &[task, bind("b.md", "- [ ] One\n")], &[]);
        assert_eq!(r.values[0], Ok(Val::Text("Hello".into())));
        assert_eq!(r.values[1], Ok(Val::Text(SKIPPED.into())));
    }
}
