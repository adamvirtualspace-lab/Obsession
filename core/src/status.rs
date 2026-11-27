//! Tasks and status formats (Syntax.md sections 1–3).
//!
//! A status format is a `statuses:` list in frontmatter:
//!
//! ```yaml
//! statuses:
//!   - "[ ] 🔳 to do"
//!   - "[x] ✅ done"
//! ```
//!
//! Which one applies: the note's own, then `_settings.md`, then the built-in
//! default. Only a small, forgiving slice of YAML is read: a block list or a
//! flow list (`[...]`) of quoted or plain strings under a top-level
//! `statuses:` key. Nothing here ever writes YAML.

/// One entry of a format: `"[?] ❓ ready to check"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The character typed between the brackets.
    pub ch: char,
    /// What the app draws instead of the brackets, usually an emoji.
    pub symbol: Box<str>,
    /// Shown in menus and tooltips. May be empty.
    pub label: Box<str>,
}

/// A status format: at least one status, in click order. The last one
/// counts as finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    statuses: Vec<Status>,
}

/// Where the format in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Note,
    Settings,
    Default,
}

/// What clicking a task does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Move to this character.
    To(char),
    /// Already at the last status: stays put.
    Finished,
    /// The character isn't in the format: the line is left alone.
    Unknown,
}

impl Format {
    /// A format from its statuses (`None` if there are none).
    pub fn from_statuses(statuses: Vec<Status>) -> Option<Self> {
        (!statuses.is_empty()).then_some(Self { statuses })
    }

    pub fn builtin() -> Self {
        Self {
            statuses: vec![
                Status {
                    ch: ' ',
                    symbol: "🔳".into(),
                    label: "to do".into(),
                },
                Status {
                    ch: 'x',
                    symbol: "✅".into(),
                    label: "done".into(),
                },
            ],
        }
    }

    pub fn statuses(&self) -> &[Status] {
        &self.statuses
    }

    /// The position of `ch` in the format. As in Obsidian, `X` counts as
    /// `x` unless the format lists `X` itself.
    pub fn index_of(&self, ch: char) -> Option<usize> {
        let find = |c: char| self.statuses.iter().position(|s| s.ch == c);
        find(ch).or_else(|| if ch == 'X' { find('x') } else { None })
    }

    pub fn get(&self, ch: char) -> Option<&Status> {
        self.index_of(ch).map(|i| &self.statuses[i])
    }

    pub fn next(&self, ch: char) -> Next {
        match self.index_of(ch) {
            None => Next::Unknown,
            Some(i) if i + 1 == self.statuses.len() => Next::Finished,
            Some(i) => Next::To(self.statuses[i + 1].ch),
        }
    }

    /// What a click on a task does. With a format of the file's or the
    /// team's own, `next`: the last status stays put, so a finished shot
    /// isn't un-finished by accident. With the built-in one (a plain note's
    /// `[ ]` / `[x]`) it's a checkbox: the last goes back to the first.
    pub fn click(&self, ch: char, source: Source) -> Next {
        match self.next(ch) {
            Next::Finished if source == Source::Default => Next::To(self.statuses[0].ch),
            next => next,
        }
    }

    /// The finished status: the last one.
    pub fn finished(&self) -> char {
        self.statuses.last().map_or('x', |s| s.ch)
    }

    pub fn is_finished(&self, ch: char) -> bool {
        self.index_of(ch) == Some(self.statuses.len() - 1)
    }
}

/// The result of reading `statuses:` from one file's frontmatter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lookup {
    /// `None` if the key was there but no entry was usable.
    pub format: Option<Format>,
    /// Problems worth showing: skipped entries, duplicates.
    pub warnings: Vec<String>,
}

/// Reads `statuses:` from the frontmatter at the top of `lines`. Returns
/// `None` when there is no frontmatter or no `statuses:` key, so the next
/// source applies. Stops reading at the end of the frontmatter.
pub fn from_frontmatter<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> Option<Lookup> {
    let mut lines = lines.into_iter();
    let first = lines.next()?;
    if first.as_ref().trim_start_matches('\u{feff}').trim_end() != "---" {
        return None;
    }

    let mut items: Option<Vec<String>> = None;
    let mut in_list = false;
    for line in lines {
        let line = line.as_ref().trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            break;
        }
        if in_list {
            let content = line.trim_start();
            if let Some(item) = content.strip_prefix('-').filter(|rest| {
                rest.is_empty() || rest.starts_with([' ', '\t'])
            }) {
                if let Some(list) = &mut items {
                    list.push(unquote(item.trim()));
                }
                continue;
            }
            if content.is_empty() || content.starts_with('#') {
                continue;
            }
            if !line.starts_with([' ', '\t']) {
                // The next top-level key: the list is over.
                in_list = false;
            } else {
                continue;
            }
        }
        if let Some(value) = line.strip_prefix("statuses:") {
            let value = value.trim();
            if value.is_empty() {
                items = Some(Vec::new());
                in_list = true;
            } else {
                items = Some(flow_list(value));
            }
        }
    }

    let items = items?;
    let mut lookup = Lookup::default();
    let mut statuses: Vec<Status> = Vec::new();
    for item in items {
        match parse_entry(&item) {
            Ok(status) => {
                if statuses.iter().any(|s| s.ch == status.ch) {
                    lookup.warnings.push(format!(
                        "statuses: [{}] is listed twice; the first one wins",
                        status.ch
                    ));
                } else {
                    statuses.push(status);
                }
            }
            Err(why) => lookup
                .warnings
                .push(format!("statuses: \"{item}\" skipped: {why}")),
        }
    }
    if statuses.is_empty() {
        lookup
            .warnings
            .push("statuses: no usable entries, so the next format applies".into());
    } else {
        lookup.format = Some(Format { statuses });
    }
    Some(lookup)
}

/// Picks the format for a note: its own, then the team's, then the default.
pub fn resolve(note: Option<&Lookup>, settings: Option<&Format>) -> (Format, Source) {
    if let Some(format) = note.and_then(|l| l.format.as_ref()) {
        return (format.clone(), Source::Note);
    }
    match settings {
        Some(format) => (format.clone(), Source::Settings),
        None => (Format::builtin(), Source::Default),
    }
}

/// Parses `[c] symbol label`.
fn parse_entry(entry: &str) -> Result<Status, &'static str> {
    let rest = entry
        .trim()
        .strip_prefix('[')
        .ok_or("it must start with [ and one character, like [x]")?;
    let mut chars = rest.chars();
    let ch = chars.next().ok_or("nothing between the brackets")?;
    if matches!(ch, '[' | ']' | '\n' | '\r') {
        return Err("needs exactly one character between the brackets");
    }
    let rest = chars
        .as_str()
        .strip_prefix(']')
        .ok_or("needs exactly one character between the brackets")?;
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return Err("needs a space after ]");
    }
    let rest = rest.trim();
    let (symbol, label) = match rest.split_once([' ', '\t']) {
        Some((symbol, label)) => (symbol, label.trim()),
        None => (rest, ""),
    };
    if symbol.is_empty() {
        return Err("needs a symbol after ], like ✅");
    }
    Ok(Status {
        ch,
        symbol: symbol.into(),
        label: label.into(),
    })
}

/// Strips YAML quotes: `"..."` (with `\"` and `\\`) or `'...'` (with `''`).
fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let inner = &value[1..value.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\'
                && let Some(next) = chars.next()
            {
                out.push(next);
            } else {
                out.push(c);
            }
        }
        out
    } else if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_owned()
    }
}

/// The quoted strings of a flow list: `["[ ] 🔳 to do", '[x] ✅ done']`.
fn flow_list(value: &str) -> Vec<String> {
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return vec![unquote(value)];
    };
    let mut items = Vec::new();
    let mut rest = inner.trim_start();
    while let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        let body = &rest[1..];
        // Find the closing quote, skipping escapes.
        let mut end = None;
        let mut iter = body.char_indices();
        while let Some((i, c)) = iter.next() {
            if quote == '"' && c == '\\' {
                iter.next();
            } else if c == quote {
                if quote == '\'' && body[i + 1..].starts_with('\'') {
                    iter.next();
                    continue;
                }
                end = Some(i);
                break;
            }
        }
        let Some(end) = end else { break };
        items.push(unquote(&rest[..end + 2]));
        rest = body[end + 1..].trim_start().trim_start_matches(',').trim_start();
    }
    items
}

/// Finds a task's status box: the byte offset of `[` and the character
/// inside. A task is a `-`, `*` or `+` list item (optionally inside `>`
/// quotes) whose text starts with `[c]` followed by a space or the line end.
/// Code fences aren't known here; callers skip them.
pub fn task_box(line: &str) -> Option<(usize, char)> {
    let bytes = line.as_bytes();
    let mut i = bytes.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
    while bytes.get(i) == Some(&b'>') {
        i += 1;
        i += bytes[i..].iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
    }
    if !matches!(bytes.get(i), Some(b'-' | b'*' | b'+')) {
        return None;
    }
    i += 1;
    let gap = bytes[i..].iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
    if gap == 0 || gap > 4 {
        return None;
    }
    i += gap;
    if bytes.get(i) != Some(&b'[') {
        return None;
    }
    let ch = line[i + 1..].chars().next()?;
    if matches!(ch, '[' | ']' | '\r' | '\n') {
        return None;
    }
    let close = i + 1 + ch.len_utf8();
    if bytes.get(close) != Some(&b']') {
        return None;
    }
    match bytes.get(close + 1) {
        None | Some(b' ' | b'\t' | b'\r' | b'\n') => Some((i, ch)),
        _ => None,
    }
}

/// Every task in `source` as (0-based line, byte offset of `[`, character),
/// skipping code fences, frontmatter and `%%` comments.
pub fn tasks(source: &str) -> Vec<(usize, usize, char)> {
    use crate::syntax::{LineState, can_hold_task, scan_line};
    let mut state = LineState::Start;
    let mut found = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if can_hold_task(state)
            && let Some((at, ch)) = task_box(line)
        {
            found.push((index, at, ch));
        }
        state = scan_line(state, line).1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(format: &Format) -> String {
        format.statuses().iter().map(|s| s.ch).collect()
    }

    #[test]
    fn finds_task_boxes() {
        assert_eq!(task_box("- [ ] a"), Some((2, ' ')));
        assert_eq!(task_box("    * [x]"), Some((6, 'x')));
        assert_eq!(task_box("> - [?] quoted"), Some((4, '?')));
        assert_eq!(task_box("+ [✅] emoji"), Some((2, '✅')));
        assert_eq!(task_box("- [x]\r"), Some((2, 'x')));
        assert_eq!(task_box("-[ ] no space"), None);
        assert_eq!(task_box("1. [ ] numbered"), None);
        assert_eq!(task_box("text [ ] middle"), None);
        assert_eq!(task_box("- [ab] two"), None);
        assert_eq!(task_box("- [x]text"), None);
        assert_eq!(task_box("- [](x)"), None);
        assert_eq!(task_box("- [link](x)"), None);
    }

    #[test]
    fn reads_block_list() {
        let src = "---\ntitle: x\nstatuses:\n  - \"[-] ⚪ cannot start\"\n  - \"[ ] 🔳 ready to start\"\n  - '[?] ❓ ready to check'\n  - \"[x] ✅ finished\"\ntags: [a]\n---\n- [ ] body\n";
        let lookup = from_frontmatter(src.lines()).unwrap();
        let format = lookup.format.unwrap();
        assert_eq!(chars(&format), "- ?x");
        assert_eq!(&*format.statuses()[2].label, "ready to check");
        assert_eq!(&*format.statuses()[3].symbol, "✅");
        assert!(lookup.warnings.is_empty());
        assert_eq!(format.next('-'), Next::To(' '));
        assert_eq!(format.next('x'), Next::Finished);
        assert_eq!(format.next('X'), Next::Finished);
        assert_eq!(format.next('k'), Next::Unknown);
        assert!(format.is_finished('x'));
        // A format of its own keeps finished tasks finished on a click.
        assert_eq!(format.click('x', Source::Note), Next::Finished);
        assert_eq!(format.click('x', Source::Settings), Next::Finished);
        assert_eq!(format.click('-', Source::Note), Next::To(' '));
        // The built-in one is a checkbox: done goes back to to do.
        let plain = Format::builtin();
        assert_eq!(plain.click(' ', Source::Default), Next::To('x'));
        assert_eq!(plain.click('x', Source::Default), Next::To(' '));
        assert_eq!(plain.click('X', Source::Default), Next::To(' '));
        assert_eq!(plain.click('?', Source::Default), Next::Unknown);
    }

    #[test]
    fn reads_flow_list_and_crlf() {
        let src = "---\r\nstatuses: [\"[ ] 🔳 to do\", '[x] ✅ done']\r\n---\r\n";
        let format = from_frontmatter(src.split_inclusive('\n')).unwrap().format.unwrap();
        assert_eq!(chars(&format), " x");
    }

    #[test]
    fn warns_and_skips() {
        let src = "---\nstatuses:\n  - \"[ab] two\"\n  - \"[x]\"\n  - \"[ ] 🔳 todo\"\n  - \"[ ] 🔲 again\"\n  - \"no brackets\"\n---\n";
        let lookup = from_frontmatter(src.lines()).unwrap();
        assert_eq!(chars(lookup.format.as_ref().unwrap()), " ");
        assert_eq!(lookup.warnings.len(), 4, "{:?}", lookup.warnings);
    }

    #[test]
    fn falls_through() {
        assert_eq!(from_frontmatter("# no frontmatter".lines()), None);
        assert_eq!(from_frontmatter("---\ntitle: x\n---\n".lines()), None);
        // `statuses:` lower in the body isn't frontmatter.
        assert_eq!(from_frontmatter("---\n---\nstatuses:\n  - \"[x] ✅\"".lines()), None);

        let empty = from_frontmatter("---\nstatuses: []\n---".lines()).unwrap();
        assert_eq!(empty.format, None);

        let team = Format {
            statuses: vec![parse_entry("[-] ⚪ blocked").unwrap()],
        };
        assert_eq!(resolve(Some(&empty), Some(&team)).1, Source::Settings);
        assert_eq!(resolve(None, None).1, Source::Default);
        assert_eq!(chars(&resolve(None, None).0), " x");
    }

    #[test]
    fn lists_tasks_outside_code() {
        let src = "---
x: 1
---
- [ ] a
```
- [ ] code
```
%%
- [ ] hidden
%%
  - [?] b
";
        assert_eq!(tasks(src), [(3, 2, ' '), (10, 4, '?')]);
    }

    #[test]
    fn capital_x_unless_listed() {
        let format = Format::builtin();
        assert_eq!(format.index_of('X'), Some(1));
        let src = "---\nstatuses:\n  - \"[x] ✅ done\"\n  - \"[X] ❌ cut\"\n---";
        let both = from_frontmatter(src.lines()).unwrap().format.unwrap();
        assert_eq!(both.index_of('X'), Some(1));
    }
}
