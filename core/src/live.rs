//! What Live Preview draws: a note split into blocks, and each line turned
//! into styled pieces with the markup hidden.
//!
//! Every piece remembers which raw bytes it came from, so a click on the
//! drawn text can put the caret on the matching raw character. Pieces whose
//! text is copied straight from the raw line are `verbatim`, and map
//! character for character; the others (a bullet, a status symbol, a link
//! alias) map to their start or end.

use std::ops::Range;

use crate::status::{Format, task_box};
use crate::syntax::{LineState, scan_line};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// One ordinary line: paragraph, heading, list item, task, quote, rule.
    Line,
    Frontmatter,
    /// A code fence, from its opening to its closing line.
    Fence,
    Table,
    /// A `%%` comment spanning several lines.
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub lines: Range<usize>,
}

/// Splits lines into blocks, using the same rules as the highlighter.
pub fn blocks<S: AsRef<str>>(lines: &[S]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut state = LineState::Start;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].as_ref();
        let (_, next) = scan_line(state, line);
        let kind = match (state, next) {
            (LineState::Start, LineState::Frontmatter) => Some(BlockKind::Frontmatter),
            (LineState::Start | LineState::Normal, LineState::Fence { .. }) => {
                Some(BlockKind::Fence)
            }
            (LineState::Start | LineState::Normal, LineState::Comment) => {
                Some(BlockKind::Comment)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            // Run until the state returns to normal.
            let start = i;
            let mut s = next;
            i += 1;
            while i < lines.len() && !matches!(s, LineState::Normal) {
                s = scan_line(s, lines[i].as_ref()).1;
                i += 1;
            }
            // An unclosed frontmatter isn't frontmatter (Obsidian agrees).
            if kind == BlockKind::Frontmatter && !matches!(s, LineState::Normal) {
                for j in start..i {
                    out.push(Block {
                        kind: BlockKind::Line,
                        lines: j..j + 1,
                    });
                }
            } else {
                out.push(Block {
                    kind,
                    lines: start..i,
                });
            }
            state = s;
            continue;
        }
        if is_table_row(line) && lines.get(i + 1).is_some_and(|l| is_table_separator(l.as_ref())) {
            let start = i;
            i += 2;
            while i < lines.len() && is_table_row(lines[i].as_ref()) {
                i += 1;
            }
            out.push(Block {
                kind: BlockKind::Table,
                lines: start..i,
            });
            state = LineState::Normal;
            continue;
        }
        out.push(Block {
            kind: BlockKind::Line,
            lines: i..i + 1,
        });
        state = next;
        i += 1;
    }
    out
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.len() > 1
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|')
        && t.contains('-')
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Style flags for a piece of drawn text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Style(pub u16);

impl Style {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const STRIKE: u16 = 1 << 2;
    pub const CODE: u16 = 1 << 3;
    pub const HIGHLIGHT: u16 = 1 << 4;
    pub const LINK: u16 = 1 << 5;
    pub const TAG: u16 = 1 << 6;
    pub const MENTION: u16 = 1 << 7;
    /// Bullets and numbers.
    pub const MARKER: u16 = 1 << 8;
    /// A status symbol.
    pub const CHIP: u16 = 1 << 9;
    /// Embeds, images, and other things shown as a placeholder.
    pub const FAINT: u16 = 1 << 10;
    /// A task whose status is the finished one.
    pub const DONE: u16 = 1 << 11;
    /// A status character the format doesn't know.
    pub const UNKNOWN: u16 = 1 << 12;
    /// A callout title (coloured by the callout type).
    pub const CALLOUT: u16 = 1 << 13;

    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    fn with(self, flag: u16) -> Self {
        Self(self.0 | flag)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `[[target]]`, as written (may include `#heading`).
    Wiki(String),
    Url(String),
    /// The task's status chip.
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    /// The raw bytes this piece stands for.
    pub raw: Range<usize>,
    /// `text` is exactly `line[raw]`.
    pub verbatim: bool,
    pub style: Style,
    pub action: Option<Action>,
}

/// One line, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    /// Heading level 1–6, or 0.
    pub heading: u8,
    /// `>` depth.
    pub quote: u8,
    /// List nesting as columns of leading whitespace (a space is 1, a tab
    /// 4), so the drawn list lines up with the raw text in Source mode.
    pub indent: u8,
    /// A `---`, `***` or `___` rule.
    pub rule: bool,
    /// `> [!note]` style callout type, lowercased.
    pub callout: Option<String>,
    pub segs: Vec<Seg>,
}

impl Line {
    pub fn is_blank(&self) -> bool {
        self.segs.is_empty() && !self.rule
    }

    /// The drawn text, for measuring and tests.
    pub fn text(&self) -> String {
        self.segs.iter().map(|s| s.text.as_str()).collect()
    }

    /// Maps a byte offset in the drawn text to a byte offset in the raw line.
    pub fn raw_offset(&self, drawn: usize, raw_len: usize) -> usize {
        let mut at = 0;
        let count = self.segs.len();
        for (index, seg) in self.segs.iter().enumerate() {
            let end = at + seg.text.len();
            // On a boundary, prefer the start of the next piece (inside any
            // formatting), except after the last one.
            let last = index + 1 == count;
            if drawn < end || (drawn == end && last && seg.verbatim) {
                if seg.verbatim {
                    return seg.raw.start + (drawn - at);
                }
                // Not character-for-character: the nearer end of the piece.
                let half = seg.text.len() / 2;
                return if drawn - at <= half {
                    seg.raw.start
                } else {
                    seg.raw.end
                };
            }
            at = end;
        }
        self.segs.last().map_or(raw_len, |s| s.raw.end)
    }
}

/// Renders one ordinary line (a `BlockKind::Line` block).
pub fn line(raw: &str, format: &Format) -> Line {
    let mut out = Line::default();
    let bytes = raw.as_bytes();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return out;
    }

    // Rules: three or more of the same `-`, `*` or `_`, spaces allowed.
    if let Some(first) = trimmed.chars().next()
        && matches!(first, '-' | '*' | '_')
        && trimmed.chars().all(|c| c == first || c == ' ')
        && trimmed.chars().filter(|&c| c == first).count() >= 3
    {
        out.rule = true;
        return out;
    }

    // Headings (up to three spaces may come first, as in Obsidian).
    let lead = bytes.iter().take(4).take_while(|&&b| b == b' ').count();
    let hashes = bytes[lead.min(3)..].iter().take_while(|&&b| b == b'#').count();
    let after = lead.min(3) + hashes;
    if lead <= 3 && (1..=6).contains(&hashes) && matches!(bytes.get(after), None | Some(b' ' | b'\t')) {
        out.heading = hashes as u8;
        let gap = bytes[after..].iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
        let start = (after + gap).min(raw.len());
        let end = trailing_block_id(raw).unwrap_or(raw.len());
        inline(raw, start, end, Style::default(), &mut out.segs);
        merge(&mut out.segs);
        return out;
    }

    // Quote markers.
    let leading = bytes.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let mut i = if bytes.get(leading) == Some(&b'>') { leading } else { 0 };
    while bytes.get(i) == Some(&b'>') {
        out.quote += 1;
        i += 1;
        if bytes.get(i) == Some(&b' ') {
            i += 1;
        }
    }
    if out.quote > 0 {
        // `> [!note] Title`
        if let Some(rest) = raw[i..].strip_prefix("[!")
            && let Some(close) = rest.find(']')
        {
            let kind = rest[..close].to_ascii_lowercase();
            let title_start = i + 2 + close + 1;
            let title_start = title_start
                + raw[title_start..].len()
                - raw[title_start..].trim_start_matches(['+', '-']).trim_start().len();
            let style = Style(Style::BOLD | Style::CALLOUT);
            out.segs.push(Seg {
                text: format!("{}  ", callout_icon(&kind)),
                raw: i..title_start,
                verbatim: false,
                style,
                action: None,
            });
            if raw[title_start..].trim().is_empty() {
                out.segs.push(Seg {
                    text: capitalize(&kind),
                    raw: title_start..raw.len(),
                    verbatim: false,
                    style,
                    action: None,
                });
            } else {
                inline(raw, title_start, raw.len(), style, &mut out.segs);
            }
            out.callout = Some(kind);
            merge(&mut out.segs);
            return out;
        }
    }

    // List items: indentation, marker, then maybe a task box.
    let indent_ws = &raw[i..];
    let lead = indent_ws.len() - indent_ws.trim_start_matches([' ', '\t']).len();
    let marker_at = i + lead;
    let block_id = trailing_block_id(raw);
    let end = block_id.unwrap_or(raw.len());

    let bullet = matches!(bytes.get(marker_at), Some(b'-' | b'*' | b'+'))
        && matches!(bytes.get(marker_at + 1), None | Some(b' ' | b'\t'));
    let digits = bytes[marker_at..].iter().take_while(|b| b.is_ascii_digit()).count();
    let numbered = digits > 0
        && matches!(bytes.get(marker_at + digits), Some(b'.' | b')'))
        && matches!(bytes.get(marker_at + digits + 1), None | Some(b' ' | b'\t'));

    if bullet || numbered {
        let ws = &raw[i..marker_at];
        let tabs = ws.matches('\t').count();
        let spaces = ws.matches(' ').count();
        out.indent = (tabs * 4 + spaces).min(48) as u8;

        if bullet && let Some((at, ch)) = task_box(raw) {
            let close = at + 2 + ch.len_utf8();
            let (text, mut style) = match format.get(ch) {
                Some(status) => (status.symbol.to_string(), Style(Style::CHIP)),
                None => (format!("⚠ [{ch}]"), Style(Style::CHIP | Style::UNKNOWN)),
            };
            if format.is_finished(ch) {
                style = style.with(Style::DONE);
            }
            out.segs.push(Seg {
                text,
                raw: at..close,
                verbatim: false,
                style,
                action: Some(Action::Status),
            });
            out.segs.push(Seg {
                text: " ".into(),
                raw: close..close,
                verbatim: false,
                style: Style::default(),
                action: None,
            });
            let base = if format.is_finished(ch) {
                Style(Style::DONE)
            } else {
                Style::default()
            };
            let text_start = (close + 1).min(end.max(close));
            if text_start < end {
                inline(raw, text_start, end, base, &mut out.segs);
            }
        } else {
            let marker_end = if bullet {
                marker_at + 1
            } else {
                marker_at + digits + 1
            };
            let gap_end = (marker_end + 1).min(raw.len());
            let text = if bullet {
                "•  ".to_string()
            } else {
                format!("{} ", &raw[marker_at..marker_end])
            };
            out.segs.push(Seg {
                text,
                raw: marker_at..gap_end,
                verbatim: false,
                style: Style(Style::MARKER),
                action: None,
            });
            if gap_end < end {
                inline(raw, gap_end, end, Style::default(), &mut out.segs);
            }
        }
        merge(&mut out.segs);
        return out;
    }

    // A plain paragraph line (after any quote markers). Leading spaces of a
    // paragraph are not shown, as in a rendered note.
    let start = marker_at;
    if start < end {
        inline(raw, start, end, Style::default(), &mut out.segs);
    }
    merge(&mut out.segs);
    out
}

/// A table row: one rendered cell per `|`-separated column.
pub fn table_cells(raw: &str, format: &Format) -> Vec<Line> {
    let _ = format;
    let bytes = raw.as_bytes();
    let mut cells = Vec::new();
    let mut start = raw.find('|').map_or(0, |p| p + 1);
    let mut i = start;
    let mut in_code = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'`' => in_code = !in_code,
            b'|' if !in_code => {
                cells.push(cell(raw, start, i));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if !raw[start..].trim().is_empty() {
        cells.push(cell(raw, start, raw.len()));
    }
    cells
}

fn cell(raw: &str, start: usize, end: usize) -> Line {
    let inner = &raw[start..end];
    let lead = inner.len() - inner.trim_start().len();
    let trail = inner.len() - inner.trim_end().len();
    let mut line = Line::default();
    if start + lead < end - trail {
        inline(raw, start + lead, end - trail, Style::default(), &mut line.segs);
    }
    // An empty cell still needs a place to click.
    if line.segs.is_empty() {
        line.segs.push(Seg {
            text: String::new(),
            raw: start + lead..start + lead,
            verbatim: true,
            style: Style::default(),
            action: None,
        });
    }
    merge(&mut line.segs);
    line
}

/// A raw line drawn as-is in one style (code, frontmatter, comments).
pub fn verbatim(raw: &str, style: Style) -> Line {
    let mut line = Line::default();
    if !raw.is_empty() {
        line.segs.push(Seg {
            text: raw.to_owned(),
            raw: 0..raw.len(),
            verbatim: true,
            style,
            action: None,
        });
    }
    line
}

/// Obsidian's callout types, as one symbol each.
pub(crate) fn callout_icon(kind: &str) -> &'static str {
    match kind {
        "warning" | "caution" | "attention" => "⚠",
        "danger" | "error" => "⛔",
        "bug" => "🐞",
        "failure" | "fail" | "missing" => "✖",
        "tip" | "hint" | "important" => "💡",
        "success" | "check" | "done" => "✔",
        "question" | "help" | "faq" => "❓",
        "example" => "📋",
        "quote" | "cite" => "❝",
        "todo" => "☑",
        "abstract" | "summary" | "tldr" => "📄",
        _ => "ℹ",
    }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Where a trailing ` ^id` starts (including the space before it).
pub(crate) fn trailing_block_id(line: &str) -> Option<usize> {
    let trimmed = line.trim_end();
    let caret = trimmed.rfind('^')?;
    let id = &trimmed[caret + 1..];
    let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let spaced = caret > 0 && matches!(trimmed.as_bytes()[caret - 1], b' ' | b'\t');
    (valid && spaced).then(|| line[..caret].trim_end().len())
}

pub(crate) fn word_start(bytes: &[u8], i: usize) -> bool {
    i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')
}

/// Parses inline markup in `raw[start..end]` into `out`.
fn inline(raw: &str, start: usize, end: usize, style: Style, out: &mut Vec<Seg>) {
    let bytes = raw.as_bytes();
    let mut i = start;
    let mut plain = start;

    let flush = |out: &mut Vec<Seg>, from: usize, to: usize| {
        if from < to {
            out.push(Seg {
                text: raw[from..to].to_owned(),
                raw: from..to,
                verbatim: true,
                style,
                action: None,
            });
        }
    };

    while i < end {
        let rest = &raw[i..end];
        let b = bytes[i];

        // Escapes: `\*` shows `*`.
        if b == b'\\' && i + 1 < end && bytes[i + 1].is_ascii_punctuation() {
            flush(out, plain, i);
            out.push(Seg {
                text: raw[i + 1..i + 2].to_owned(),
                raw: i..i + 2,
                verbatim: false,
                style,
                action: None,
            });
            i += 2;
            plain = i;
            continue;
        }

        // Inline code.
        if b == b'`' {
            let run = rest.bytes().take_while(|&c| c == b'`').count();
            if let Some(close) = find_run(&bytes[i + run..end], b'`', run) {
                flush(out, plain, i);
                let inner = i + run..i + run + close;
                out.push(Seg {
                    text: raw[inner.clone()].to_owned(),
                    raw: inner,
                    verbatim: true,
                    style: style.with(Style::CODE),
                    action: None,
                });
                i = i + run + close + run;
                plain = i;
                continue;
            }
            i += run;
            continue;
        }

        // `%% comments %%` are hidden.
        if let Some(after) = rest.strip_prefix("%%") {
            flush(out, plain, i);
            i = after.find("%%").map_or(end, |c| i + 2 + c + 2);
            plain = i;
            continue;
        }

        // Embeds and images: a placeholder.
        if rest.starts_with("![[")
            && let Some(close) = rest.find("]]")
        {
            flush(out, plain, i);
            let name = rest[3..close].split('|').next().unwrap_or("");
            out.push(Seg {
                text: format!("📎 {name}"),
                raw: i..i + close + 2,
                verbatim: false,
                style: style.with(Style::FAINT),
                action: Some(Action::Wiki(name.to_owned())),
            });
            i += close + 2;
            plain = i;
            continue;
        }
        if rest.starts_with("![")
            && let Some((label, url, len)) = markdown_link(&rest[1..])
        {
            flush(out, plain, i);
            out.push(Seg {
                text: format!("🖼 {}", &rest[1 + label.start..1 + label.end]),
                raw: i..i + 1 + len,
                verbatim: false,
                style: style.with(Style::FAINT),
                action: Some(Action::Url(url.to_owned())),
            });
            i += 1 + len;
            plain = i;
            continue;
        }

        // Wiki links.
        if rest.starts_with("[[")
            && let Some(close) = rest[2..].find("]]")
        {
            let inner = &rest[2..2 + close];
            if !inner.is_empty() && !inner.contains('[') {
                flush(out, plain, i);
                let (target, alias) = match inner.split_once('|') {
                    Some((t, a)) => (t.trim(), Some(a.trim())),
                    None => (inner.trim(), None),
                };
                let whole = i..i + 2 + close + 2;
                let link = Some(Action::Wiki(target.to_owned()));
                let style = style.with(Style::LINK);
                match alias {
                    None if !target.contains('#') => out.push(Seg {
                        text: inner.to_owned(),
                        raw: i + 2..i + 2 + close,
                        verbatim: true,
                        style,
                        action: link,
                    }),
                    _ => out.push(Seg {
                        text: alias.map_or_else(|| target.replace('#', " > "), str::to_owned),
                        raw: whole.clone(),
                        verbatim: false,
                        style,
                        action: link,
                    }),
                }
                i = whole.end;
                plain = i;
                continue;
            }
        }

        // Markdown links: the label stays, the URL hides.
        if b == b'['
            && let Some((label, url, len)) = markdown_link(rest)
        {
            flush(out, plain, i);
            out.push(Seg {
                text: rest[label.clone()].to_owned(),
                raw: i + label.start..i + label.end,
                verbatim: true,
                style: style.with(Style::LINK),
                action: Some(Action::Url(url.to_owned())),
            });
            i += len;
            plain = i;
            continue;
        }

        // Emphasis: ** __ ~~ == then * _.
        let pairs: [(&str, u16); 6] = [
            ("**", Style::BOLD),
            ("__", Style::BOLD),
            ("~~", Style::STRIKE),
            ("==", Style::HIGHLIGHT),
            ("*", Style::ITALIC),
            ("_", Style::ITALIC),
        ];
        let mut matched = false;
        for (delim, flag) in pairs {
            if !rest.starts_with(delim) {
                continue;
            }
            let n = delim.len();
            if delim.starts_with('_') && !word_start(bytes, i) {
                break;
            }
            let after = &raw[i + n..end];
            if after.starts_with([' ', '\t']) || after.is_empty() {
                break;
            }
            if let Some(close) = find_closing(after, delim) {
                let inner_end = i + n + close;
                let closing_end = inner_end + n;
                if delim.starts_with('_')
                    && closing_end < end
                    && (bytes[closing_end].is_ascii_alphanumeric())
                {
                    break;
                }
                flush(out, plain, i);
                inline(raw, i + n, inner_end, style.with(flag), out);
                i = closing_end;
                plain = i;
                matched = true;
            }
            break;
        }
        if matched {
            continue;
        }

        // Tags and mentions keep their text, in colour.
        if (b == b'#' || b == b'@') && word_start(bytes, i) {
            let len = raw[i + 1..end]
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'/'))
                .count();
            let all_digits = raw[i + 1..i + 1 + len].bytes().all(|c| c.is_ascii_digit());
            if len > 0 && !(b == b'#' && all_digits) {
                flush(out, plain, i);
                let flag = if b == b'#' { Style::TAG } else { Style::MENTION };
                out.push(Seg {
                    text: raw[i..i + 1 + len].to_owned(),
                    raw: i..i + 1 + len,
                    verbatim: true,
                    style: style.with(flag),
                    action: None,
                });
                i += 1 + len;
                plain = i;
                continue;
            }
        }

        i += raw[i..].chars().next().map_or(1, char::len_utf8);
    }
    flush(out, plain, end.min(raw.len()));
}

/// `[label](url)` at the start of `s`: (label range in `s`, url, total length).
fn markdown_link(s: &str) -> Option<(Range<usize>, &str, usize)> {
    if !s.starts_with('[') {
        return None;
    }
    let close = s.find("](")?;
    if s[1..close].contains('[') {
        return None;
    }
    let url_end = close + 2 + s[close + 2..].find(')')?;
    let url = s[close + 2..url_end].trim().trim_start_matches('<').trim_end_matches('>');
    Some((1..close, url, url_end + 1))
}

/// The next closing `delim` in `s` that isn't preceded by whitespace (and,
/// for single `*` / `_`, isn't part of a double).
fn find_closing(s: &str, delim: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut from = 0;
    while let Some(pos) = s[from..].find(delim) {
        let at = from + pos;
        let n = delim.len();
        let prev_space = at == 0 || matches!(bytes[at - 1], b' ' | b'\t');
        let doubled = n == 1
            && (bytes.get(at + 1) == Some(&delim.as_bytes()[0])
                || (at > 0 && bytes[at - 1] == delim.as_bytes()[0]));
        if at > 0 && !prev_space && !doubled {
            return Some(at);
        }
        from = at + n.max(if doubled { 2 } else { 1 });
        if from >= s.len() {
            break;
        }
    }
    None
}

fn find_run(bytes: &[u8], ch: u8, len: usize) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ch {
            let run = bytes[i..].iter().take_while(|&&b| b == ch).count();
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Joins neighbouring verbatim pieces with the same style.
fn merge(segs: &mut Vec<Seg>) {
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for seg in segs.drain(..) {
        if let Some(last) = out.last_mut()
            && last.verbatim
            && seg.verbatim
            && last.style == seg.style
            && last.action == seg.action
            && last.raw.end == seg.raw.start
        {
            last.text.push_str(&seg.text);
            last.raw.end = seg.raw.end;
            continue;
        }
        out.push(seg);
    }
    *segs = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt() -> Format {
        Format::builtin()
    }

    fn drawn(raw: &str) -> String {
        line(raw, &fmt()).text()
    }

    #[test]
    fn hides_markup() {
        assert_eq!(drawn("## Notes **bold** and *it*"), "Notes bold and it");
        assert_eq!(drawn("see [[anim/sh040|Shot 40]] and [[sh041]]"), "see Shot 40 and sh041");
        assert_eq!(drawn("[site](http://a.b) `code` ~~old~~ ==hi=="), "site code old hi");
        assert_eq!(drawn("a %%hidden%% b"), "a  b");
        assert_eq!(drawn("task text ^a1"), "task text");
        assert_eq!(drawn(r"not \*em\*"), "not *em*");
        assert_eq!(drawn("snake_case_name stays"), "snake_case_name stays");
        assert_eq!(drawn("2 * 3 * 4"), "2 * 3 * 4");
    }

    #[test]
    fn indented_headings() {
        let l = line("  #### 003B Interact", &fmt());
        assert_eq!(l.heading, 4);
        assert_eq!(l.text(), "003B Interact");
        assert_eq!(line("####   003A", &fmt()).text(), "003A");
        // Four spaces is too many.
        assert_eq!(line("    # not", &fmt()).heading, 0);
    }

    #[test]
    fn lists_and_tasks() {
        let l = line("  - [ ] Polish @ika", &fmt());
        assert_eq!(l.indent, 2);
        assert_eq!(line("	- b", &fmt()).indent, 4);
        assert_eq!(l.text(), "🔳 Polish @ika");
        assert_eq!(l.segs[0].action, Some(Action::Status));
        assert_eq!(l.segs[0].raw, 4..7);

        let done = line("- [x] Done", &fmt());
        assert!(done.segs.iter().any(|s| s.style.has(Style::DONE)));
        assert_eq!(line("- [k] what", &fmt()).text(), "⚠ [k] what");
        assert_eq!(drawn("- item"), "•  item");
        assert_eq!(drawn("12. item"), "12. item");
        assert_eq!(drawn("> quoted"), "quoted");
        assert_eq!(line("> > deep", &fmt()).quote, 2);
        assert!(line("---", &fmt()).rule);
        assert!(line("   ", &fmt()).is_blank());
    }

    #[test]
    fn callouts() {
        let l = line("> [!warning] Careful", &fmt());
        assert_eq!(l.callout.as_deref(), Some("warning"));
        assert_eq!(l.text(), "⚠  Careful");
        assert_eq!(line("> [!note]", &fmt()).text(), "ℹ  Note");
    }

    #[test]
    fn maps_clicks_back_to_raw() {
        let raw = "a **bold** [[x|alias]] end";
        let l = line(raw, &fmt());
        assert_eq!(l.text(), "a bold alias end");
        // "b" of bold (drawn 2) is raw 4.
        assert_eq!(l.raw_offset(2, raw.len()), 4);
        // "o" (drawn 3) is raw 5.
        assert_eq!(l.raw_offset(3, raw.len()), 5);
        // Inside the alias: snaps to an end of the whole link.
        let alias_at = "a bold ".len();
        assert_eq!(l.raw_offset(alias_at, raw.len()), raw.find("[[").unwrap());
        // "end" maps exactly.
        let end_at = l.text().find("end").unwrap();
        assert_eq!(l.raw_offset(end_at + 1, raw.len()), raw.find("end").unwrap() + 1);
        // Past the end: the raw end.
        assert_eq!(l.raw_offset(999, raw.len()), raw.len());
    }

    #[test]
    fn splits_blocks() {
        let lines = [
            "---", "a: 1", "---", "# T", "```", "code", "```", "| a | b |", "|---|---|",
            "| 1 | 2 |", "text", "%%", "hidden", "%%", "after",
        ];
        let kinds: Vec<(BlockKind, Range<usize>)> =
            blocks(&lines).into_iter().map(|b| (b.kind, b.lines)).collect();
        assert_eq!(
            kinds,
            [
                (BlockKind::Frontmatter, 0..3),
                (BlockKind::Line, 3..4),
                (BlockKind::Fence, 4..7),
                (BlockKind::Table, 7..10),
                (BlockKind::Line, 10..11),
                (BlockKind::Comment, 11..14),
                (BlockKind::Line, 14..15),
            ]
        );
        // An unclosed frontmatter is just lines; an unclosed fence runs to the end.
        assert_eq!(blocks(&["---", "x"]).len(), 2);
        assert_eq!(blocks(&["```", "x"])[0].lines, 0..2);
    }

    #[test]
    fn table_cells_map_to_raw() {
        let raw = "| a **b** | `c|d` |  |";
        let cells = table_cells(raw, &fmt());
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].text(), "a b");
        assert_eq!(cells[1].text(), "c|d");
        // An empty cell is still clickable, at its end.
        assert_eq!(cells[2].segs[0].raw.start, raw.len() - 1);
    }
}
