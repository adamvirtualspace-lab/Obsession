//! Graph files: `_nodes.md` (Syntax.md, section 7).
//!
//! A graph file is an ordinary note with three sections the app reads:
//! `## Nodes` (one link or `fn Name` per list item), `## Wires`
//! (`from → to`, optionally `: Function(...)`) and `## Functions`, plus a
//! `%% obsession-layout %%` comment at the end with node positions.
//!
//! Edits here touch only the lines they must: a list item added or removed,
//! or the layout comment rewritten. Everything else in the file, line
//! endings included, stays as it was.

use std::ops::Range;

use crate::syntax::{LineState, scan_line};

/// The graph file's name; one per folder.
pub const FILE: &str = "_nodes.md";

/// Whether `rel` is a graph file.
pub fn is_graph(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.eq_ignore_ascii_case(FILE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// `- [[anim/sh040]]`: a note (`target` as written, no alias).
    File { target: String },
    /// `- fn Check Lighting`
    Function { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// How the layout names it: the link without `[[ ]]` and alias, or
    /// `fn Name`.
    pub key: String,
    pub kind: NodeKind,
    /// 0-based line in the file.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// `[[target#sub]]`
    Note { target: String, sub: Option<String> },
    /// `Check Lighting.anim`: a function node's pin.
    Pin { function: String, pin: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire {
    pub from: End,
    pub to: End,
    /// What follows ` : `, e.g. `Wait For Done(task: "x")`.
    pub function: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
    /// (key, x, y) from the layout comment.
    pub layout: Vec<(String, f32, f32)>,
}

impl Graph {
    pub fn position(&self, key: &str) -> Option<(f32, f32)> {
        self.layout
            .iter()
            .find(|(k, _, _)| k.eq_ignore_ascii_case(key))
            .map(|&(_, x, y)| (x, y))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Nodes,
    Wires,
    Functions,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Nodes => "Nodes",
            Section::Wires => "Wires",
            Section::Functions => "Functions",
        }
    }
}

/// The file as lines, remembering its line ending.
struct Lines {
    lines: Vec<String>,
    ending: &'static str,
}

impl Lines {
    fn new(text: &str) -> Self {
        let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let lines = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
            .collect();
        Self { lines, ending }
    }

    fn join(&self) -> String {
        self.lines.join(self.ending)
    }

    /// Which section each line is in (headings and fenced code respected).
    fn sections(&self) -> Vec<Option<Section>> {
        let mut out = Vec::with_capacity(self.lines.len());
        let mut state = LineState::Start;
        let mut current = None;
        for line in &self.lines {
            let code = matches!(state, LineState::Fence { .. } | LineState::Frontmatter);
            state = scan_line(state, line).1;
            if !code && let Some(level_title) = heading(line) {
                current = match level_title {
                    (2, title) if title.eq_ignore_ascii_case("nodes") => Some(Section::Nodes),
                    (2, title) if title.eq_ignore_ascii_case("wires") => Some(Section::Wires),
                    (2, title) if title.eq_ignore_ascii_case("functions") => Some(Section::Functions),
                    (level, _) if level <= 2 => None,
                    _ => current,
                };
                out.push(None);
                continue;
            }
            out.push(current);
        }
        out
    }

    /// The layout comment: (line of the opening `%%`, line of the closing
    /// `%%`).
    fn layout(&self) -> Option<(usize, usize)> {
        let open = (0..self.lines.len()).rev().find(|&i| {
            self.lines[i].trim() == "%%"
                && self.lines.get(i + 1).is_some_and(|l| l.trim() == "obsession-layout")
        })?;
        let close = (open + 2..self.lines.len()).find(|&i| self.lines[i].trim() == "%%")?;
        Some((open, close))
    }
}

/// `## Title` → (2, "Title").
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let level = t.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

/// The text of a `- ` / `* ` / `+ ` list item.
fn list_item(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix(['-', '*', '+'])?;
    rest.strip_prefix([' ', '\t']).map(str::trim)
}

/// `[[target#sub|alias]]` at the start of `s`: (target, sub, length).
fn wiki(s: &str) -> Option<(String, Option<String>, usize)> {
    let inner_start = s.strip_prefix("[[")?;
    let close = inner_start.find("]]")?;
    let inner = &inner_start[..close];
    let dest = inner.split('|').next().unwrap_or(inner);
    let (target, sub) = match dest.split_once('#') {
        Some((t, s)) => (t.trim().to_owned(), Some(s.trim().to_owned())),
        None => (dest.trim().to_owned(), None),
    };
    Some((target, sub, close + 4))
}

fn end(s: &str) -> Option<End> {
    let s = s.trim();
    if s.starts_with("[[") {
        let (target, sub, _) = wiki(s)?;
        return Some(End::Note { target, sub });
    }
    let (function, pin) = s.rsplit_once('.')?;
    Some(End::Pin {
        function: function.trim().to_owned(),
        pin: pin.trim().to_owned(),
    })
}

/// The arrow (`→` or `->`) outside any `[[ ]]`: (start, length).
fn arrow(s: &str) -> Option<(usize, usize)> {
    let mut depth = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if s[i..].starts_with("[[") {
            depth += 1;
            i += 2;
            continue;
        }
        if s[i..].starts_with("]]") {
            depth -= 1;
            i += 2;
            continue;
        }
        if depth == 0 {
            if s[i..].starts_with('→') {
                return Some((i, '→'.len_utf8()));
            }
            if s[i..].starts_with("->") {
                return Some((i, 2));
            }
        }
        i += s[i..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn parse_wire(item: &str, line: usize) -> Option<Wire> {
    let (at, len) = arrow(item)?;
    let from = end(&item[..at])?;
    let rest = item[at + len..].trim_start();
    // The far end runs to `]]` for a note, else to ` : ` or the end.
    let (to_text, after) = if rest.starts_with("[[") {
        let close = rest.find("]]")? + 2;
        (&rest[..close], &rest[close..])
    } else {
        match rest.find(':') {
            Some(colon) => (&rest[..colon], &rest[colon..]),
            None => (rest, ""),
        }
    };
    let to = end(to_text)?;
    let function = after
        .trim()
        .strip_prefix(':')
        .map(|f| f.trim().to_owned())
        .filter(|f| !f.is_empty());
    Some(Wire {
        from,
        to,
        function,
        line,
    })
}

/// Reads a graph file.
pub fn parse(text: &str) -> Graph {
    let lines = Lines::new(text);
    let sections = lines.sections();
    let mut graph = Graph::default();
    let layout = lines.layout();
    for (i, line) in lines.lines.iter().enumerate() {
        if layout.is_some_and(|(open, close)| (open..=close).contains(&i)) {
            continue;
        }
        let Some(item) = list_item(line) else {
            continue;
        };
        match sections[i] {
            Some(Section::Nodes) => {
                if let Some((target, _, _)) = wiki(item) {
                    if !target.is_empty() {
                        graph.nodes.push(Node {
                            key: target.clone(),
                            kind: NodeKind::File { target },
                            line: i,
                        });
                    }
                } else if let Some(name) = item.strip_prefix("fn ") {
                    let name = name.trim().to_owned();
                    graph.nodes.push(Node {
                        key: format!("fn {name}"),
                        kind: NodeKind::Function { name },
                        line: i,
                    });
                }
            }
            Some(Section::Wires) => {
                if let Some(wire) = parse_wire(item, i) {
                    graph.wires.push(wire);
                }
            }
            _ => {}
        }
    }
    if let Some((open, close)) = layout {
        for line in &lines.lines[open + 2..close] {
            let Some((key, xy)) = line.trim().rsplit_once(' ') else {
                continue;
            };
            let Some((x, y)) = xy.split_once(',') else {
                continue;
            };
            if let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse()) {
                graph.layout.push((key.trim().to_owned(), x, y));
            }
        }
    }
    graph
}

/// Writes node positions into the layout comment (replacing it, or adding
/// it at the end). Positions are rounded to whole pixels.
pub fn set_layout(text: &str, positions: &[(&str, f32, f32)]) -> String {
    let mut lines = Lines::new(text);
    let entries: Vec<String> = positions
        .iter()
        .map(|(key, x, y)| format!("{key} {},{}", x.round() as i64, y.round() as i64))
        .collect();
    match lines.layout() {
        Some((open, close)) => {
            lines.lines.splice(open + 2..close, entries);
        }
        None => {
            // Keep a final line ending where there was one.
            let trailing = lines.lines.last().is_some_and(String::is_empty);
            if trailing {
                lines.lines.pop();
            }
            if lines.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.lines.push(String::new());
            }
            lines.lines.push("%%".into());
            lines.lines.push("obsession-layout".into());
            lines.lines.extend(entries);
            lines.lines.push("%%".into());
            if trailing {
                lines.lines.push(String::new());
            }
        }
    }
    lines.join()
}

/// Adds `- {item}` at the end of a section's list, creating the section
/// (before the layout comment) if the file doesn't have it.
pub fn add_item(text: &str, section: Section, item: &str) -> String {
    let mut lines = Lines::new(text);
    let sections = lines.sections();
    let new = format!("- {item}");
    let in_section: Vec<usize> = (0..lines.lines.len()).filter(|&i| sections[i] == Some(section)).collect();
    if let Some(&first) = in_section.first() {
        // After the last list item, or right under the heading.
        let at = in_section
            .iter()
            .rev()
            .find(|&&i| list_item(&lines.lines[i]).is_some())
            .map_or(first, |&i| i + 1);
        lines.lines.insert(at, new);
        return lines.join();
    }
    // No such section: add it before the layout comment, or at the end.
    let at = match lines.layout() {
        Some((open, _)) => open,
        None => {
            let mut at = lines.lines.len();
            while at > 0 && lines.lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            at
        }
    };
    let mut block = Vec::new();
    if at > 0 && !lines.lines[at - 1].trim().is_empty() {
        block.push(String::new());
    }
    block.push(format!("## {}", section.title()));
    block.push(new);
    let after_blank = lines.lines.get(at).is_some_and(|l| !l.trim().is_empty());
    if after_blank || lines.layout().is_some() {
        block.push(String::new());
    }
    if at == lines.lines.len() {
        // At the very end: keep the file ending in a line ending.
        block.push(String::new());
        if lines.lines.last().is_some_and(String::is_empty) {
            lines.lines.pop();
        }
    }
    lines.lines.splice(at..at, block);
    lines.join()
}

/// Removes whole lines (0-based), e.g. a node and its wires.
pub fn remove_lines(text: &str, remove: &[usize]) -> String {
    let lines = Lines::new(text);
    let kept: Vec<String> = lines
        .lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !remove.contains(i))
        .map(|(_, l)| l.clone())
        .collect();
    Lines {
        lines: kept,
        ending: lines.ending,
    }
    .join()
}

/// The lines of the `## Functions` section's code, for later milestones.
pub fn section_lines(text: &str, section: Section) -> Range<usize> {
    let lines = Lines::new(text);
    let sections = lines.sections();
    let first = sections.iter().position(|s| *s == Some(section));
    let last = sections.iter().rposition(|s| *s == Some(section));
    match (first, last) {
        (Some(a), Some(b)) => a..b + 1,
        _ => 0..0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# sh040\n\nBrief.\n\n## Nodes\n- [[anim/sh040]]\n- [[fx/sh040-water|Water]]\n- fn Check Lighting\n\n## Wires\n- [[lighting/sh040]] → [[comp/sh040]] : Wait For Done(task: \"Render: pass\", target: \"Comp\")\n- [[comp/sh040]] -> [[delivery/ep04#^a1]]\n- [[anim/sh040]] → Check Lighting.anim\n- Check Lighting.light → [[lighting/sh040]]\n\n## Functions\n```obsession\nfn Check Lighting\n## not a heading\n```\n\n%%\nobsession-layout\nanim/sh040 120,80\nfn Check Lighting 300,200\n%%\n";

    #[test]
    fn reads_nodes_wires_layout() {
        let g = parse(SAMPLE);
        let keys: Vec<&str> = g.nodes.iter().map(|n| n.key.as_str()).collect();
        assert_eq!(keys, ["anim/sh040", "fx/sh040-water", "fn Check Lighting"]);
        assert_eq!(g.nodes[0].line, 5);
        assert_eq!(g.wires.len(), 4);
        assert_eq!(
            g.wires[0].function.as_deref(),
            Some("Wait For Done(task: \"Render: pass\", target: \"Comp\")")
        );
        assert_eq!(
            g.wires[1].to,
            End::Note { target: "delivery/ep04".into(), sub: Some("^a1".into()) }
        );
        assert_eq!(g.wires[2].to, End::Pin { function: "Check Lighting".into(), pin: "anim".into() });
        assert_eq!(g.wires[3].from, End::Pin { function: "Check Lighting".into(), pin: "light".into() });
        assert_eq!(g.position("anim/sh040"), Some((120.0, 80.0)));
        assert_eq!(g.position("fn check lighting"), Some((300.0, 200.0)));
    }

    #[test]
    fn layout_is_replaced_or_added() {
        let moved = set_layout(SAMPLE, &[("anim/sh040", 10.4, -20.6), ("fx/sh040-water", 5.0, 5.0)]);
        assert!(moved.ends_with("%%\nobsession-layout\nanim/sh040 10,-21\nfx/sh040-water 5,5\n%%\n"));
        assert_eq!(moved.lines().count(), SAMPLE.lines().count());
        // Everything before the comment is untouched.
        let cut = SAMPLE.find("%%\nobsession").unwrap();
        assert_eq!(&moved[..cut], &SAMPLE[..cut]);

        let crlf = "# G\r\n\r\n## Nodes\r\n- [[a]]\r\n";
        assert_eq!(
            set_layout(crlf, &[("a", 1.0, 2.0)]),
            "# G\r\n\r\n## Nodes\r\n- [[a]]\r\n\r\n%%\r\nobsession-layout\r\na 1,2\r\n%%\r\n"
        );
    }

    #[test]
    fn adds_and_removes_items() {
        let added = add_item(SAMPLE, Section::Nodes, "[[comp/sh040]]");
        let g = parse(&added);
        // At the end of the list, before the blank line and `## Wires`.
        assert_eq!(g.nodes[3].key, "comp/sh040");
        assert_eq!(g.nodes[3].line, 8);
        assert_eq!(g.nodes.len(), 4);

        let wired = add_item(SAMPLE, Section::Wires, "[[anim/sh040]] → [[comp/sh040]]");
        assert_eq!(parse(&wired).wires.len(), 5);

        // A file without the sections gets them, before the layout comment.
        let bare = "# Graph\n\nNotes.\n";
        let one = add_item(bare, Section::Nodes, "[[a]]");
        assert_eq!(one, "# Graph\n\nNotes.\n\n## Nodes\n- [[a]]\n");
        let laid = set_layout(&one, &[("a", 0.0, 0.0)]);
        let two = add_item(&laid, Section::Wires, "[[a]] → [[b]]");
        assert_eq!(
            two,
            "# Graph\n\nNotes.\n\n## Nodes\n- [[a]]\n\n## Wires\n- [[a]] → [[b]]\n\n%%\nobsession-layout\na 0,0\n%%\n"
        );
        assert_eq!(parse(&two).wires.len(), 1);

        let removed = remove_lines(SAMPLE, &[5, 11]);
        let g = parse(&removed);
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.wires.len(), 3);
    }

    #[test]
    fn graph_files() {
        assert!(is_graph("shots/_nodes.md"));
        assert!(is_graph("_Nodes.md"));
        assert!(!is_graph("shots/nodes.md"));
    }
}
