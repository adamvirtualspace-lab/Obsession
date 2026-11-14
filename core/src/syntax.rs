//! A small line-by-line markdown tokenizer for editor highlighting.
//!
//! It knows exactly what the app cares about (headings, lists, task boxes,
//! links, `@people`, `^ids`, `#tags`, code, comments, frontmatter) and nothing
//! else. It replaces a general syntax-highlighting engine that cost ~30 MB of
//! RAM just to colour markdown.
//!
//! State that spans lines (code fences, frontmatter, `%%` comments) is carried
//! in [`LineState`], so an editor can re-highlight from any line onward.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineState {
    /// The first line of the file, where frontmatter may start.
    #[default]
    Start,
    Normal,
    Frontmatter,
    Fence {
        ch: u8,
        len: u8,
    },
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Heading,
    ListMarker,
    TaskBox,
    Link,
    Mention,
    BlockId,
    Tag,
    Code,
    Quote,
    Frontmatter,
    Comment,
}

pub type Span = (Range<usize>, Kind);

/// Tokenizes one line (without its line ending). Returns the spans, sorted
/// and non-overlapping, and the state for the next line.
pub fn scan_line(state: LineState, line: &str) -> (Vec<Span>, LineState) {
    let mut spans = Vec::new();
    let whole = 0..line.len();

    match state {
        LineState::Start if line.trim_end() == "---" => {
            spans.push((whole, Kind::Frontmatter));
            return (spans, LineState::Frontmatter);
        }
        LineState::Frontmatter => {
            spans.push((whole, Kind::Frontmatter));
            let trimmed = line.trim_end();
            let next = if trimmed == "---" || trimmed == "..." {
                LineState::Normal
            } else {
                LineState::Frontmatter
            };
            return (spans, next);
        }
        LineState::Fence { ch, len } => {
            spans.push((whole, Kind::Code));
            let trimmed = line.trim();
            let closes = !trimmed.is_empty()
                && trimmed.bytes().all(|b| b == ch)
                && trimmed.len() >= usize::from(len);
            return (spans, if closes { LineState::Normal } else { state });
        }
        _ => {}
    }

    let mut in_comment = state == LineState::Comment;
    if !in_comment {
        if let Some((ch, len)) = opens_fence(line) {
            spans.push((whole, Kind::Code));
            return (spans, LineState::Fence { ch, len });
        }
        if is_heading(line) {
            spans.push((whole, Kind::Heading));
            return (spans, LineState::Normal);
        }
    }

    let mut i = 0;
    if !in_comment {
        i = line_prefix(line, &mut spans);
    }
    let block_id = block_id_start(line);

    let bytes = line.as_bytes();
    while i < bytes.len() {
        if in_comment {
            let end = line[i..].find("%%").map_or(bytes.len(), |e| i + e + 2);
            spans.push((i..end, Kind::Comment));
            in_comment = end == bytes.len() && !line[i..].contains("%%");
            i = end;
            continue;
        }
        if Some(i) == block_id {
            spans.push((i..line.trim_end().len(), Kind::BlockId));
            break;
        }
        match bytes[i] {
            b'`' => {
                let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let end = find_run(&bytes[i + run..], run).map_or(i + run, |e| i + run + e + run);
                spans.push((i..end, Kind::Code));
                i = end;
            }
            b'%' if line[i..].starts_with("%%") => {
                in_comment = true;
                let start = i;
                i += 2;
                let end = line[i..].find("%%").map(|e| i + e + 2);
                if let Some(end) = end {
                    in_comment = false;
                    spans.push((start..end, Kind::Comment));
                    i = end;
                } else {
                    spans.push((start..bytes.len(), Kind::Comment));
                    i = bytes.len();
                }
            }
            b'[' | b'!' => {
                let start = i;
                let open = if bytes[i] == b'!' { i + 1 } else { i };
                if line[open..].starts_with("[[") {
                    if let Some(end) = line[open + 2..].find("]]") {
                        let end = open + 2 + end + 2;
                        spans.push((start..end, Kind::Link));
                        i = end;
                        continue;
                    }
                } else if bytes.get(open) == Some(&b'[')
                    && let Some(end) = markdown_link_end(line, open) {
                        spans.push((start..end, Kind::Link));
                        i = end;
                        continue;
                    }
                i += 1;
            }
            b'@' if word_start(bytes, i) => {
                let end = i + 1 + name_len(&bytes[i + 1..]);
                if end > i + 1 {
                    spans.push((i..end, Kind::Mention));
                }
                i = end.max(i + 1);
            }
            b'#' if word_start(bytes, i) => {
                let len = bytes[i + 1..]
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/'))
                    .count();
                let tag = &bytes[i + 1..i + 1 + len];
                if len > 0 && !tag.iter().all(u8::is_ascii_digit) {
                    spans.push((i..i + 1 + len, Kind::Tag));
                }
                i += 1 + len;
            }
            _ => i += 1,
        }
    }

    let next = if in_comment {
        LineState::Comment
    } else {
        LineState::Normal
    };
    (spans, next)
}

/// The state at the start of line `index`, found by scanning from the top.
/// Used for one-off questions ("is this line inside a code fence?"), not
/// per frame.
pub fn state_at<S: AsRef<str>>(lines: impl IntoIterator<Item = S>, index: usize) -> LineState {
    let mut state = LineState::Start;
    for line in lines.into_iter().take(index) {
        state = scan_line(state, line.as_ref().trim_end_matches(['\r', '\n'])).1;
    }
    state
}

/// Whether a line in this state can hold a task (not code, frontmatter or a
/// `%%` comment).
pub fn can_hold_task(state: LineState) -> bool {
    matches!(state, LineState::Start | LineState::Normal)
}

fn opens_fence(line: &str) -> Option<(u8, u8)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|&b| b == ch).count();
    (run >= 3).then(|| (ch, run.min(255) as u8))
}

fn is_heading(line: &str) -> bool {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && matches!(line.as_bytes().get(hashes), None | Some(b' ' | b'\t'))
}

/// Colours `>` quote markers and list markers (plus a task box), and returns
/// where the rest of the line starts.
fn line_prefix(line: &str, spans: &mut Vec<Span>) -> usize {
    let bytes = line.as_bytes();
    let mut i = bytes.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();

    while bytes.get(i) == Some(&b'>') {
        spans.push((i..i + 1, Kind::Quote));
        i += 1;
        while bytes.get(i) == Some(&b' ') {
            i += 1;
        }
    }

    let marker_end = match bytes.get(i) {
        Some(b'-' | b'*' | b'+') => Some(i + 1),
        Some(b) if b.is_ascii_digit() => {
            let digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
            matches!(bytes.get(i + digits), Some(b'.' | b')')).then_some(i + digits + 1)
        }
        _ => None,
    };
    let Some(end) = marker_end else { return i };
    if !matches!(bytes.get(end), None | Some(b' ' | b'\t')) {
        return i;
    }
    spans.push((i..end, Kind::ListMarker));

    // A task box, by the same rules the rest of the app uses.
    if let Some((at, ch)) = crate::status::task_box(line)
        && at > end
    {
        let close = at + 2 + ch.len_utf8();
        spans.push((at..close, Kind::TaskBox));
        return close;
    }
    (end + 1).min(bytes.len())
}

/// Where a trailing ` ^id` starts, if the line ends with one.
fn block_id_start(line: &str) -> Option<usize> {
    let trimmed = line.trim_end();
    let caret = trimmed.rfind('^')?;
    let id = &trimmed[caret + 1..];
    let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let spaced = caret > 0 && matches!(trimmed.as_bytes()[caret - 1], b' ' | b'\t');
    (valid && spaced).then_some(caret)
}

fn markdown_link_end(line: &str, open: usize) -> Option<usize> {
    let close = open + line[open..].find("](")?;
    if line[open + 1..close].contains('[') {
        return None;
    }
    let end = close + 2 + line[close + 2..].find(')')?;
    Some(end + 1)
}

fn word_start(bytes: &[u8], i: usize) -> bool {
    i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')
}

fn name_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        .count()
}

fn find_run(bytes: &[u8], len: usize) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(&str, Kind)> {
        scan_line(LineState::Normal, line)
            .0
            .into_iter()
            .map(|(r, k)| (&line[r], k))
            .collect()
    }

    #[test]
    fn task_line() {
        assert_eq!(
            kinds("- [?] Polish pass @ika see [[anim/sh040]] ^a2"),
            [
                ("-", Kind::ListMarker),
                ("[?]", Kind::TaskBox),
                ("@ika", Kind::Mention),
                ("[[anim/sh040]]", Kind::Link),
                ("^a2", Kind::BlockId),
            ]
        );
    }

    #[test]
    fn emoji_task_box_and_email() {
        assert_eq!(
            kinds("* [✅] mail bob@example.com"),
            [("*", Kind::ListMarker), ("[✅]", Kind::TaskBox)]
        );
    }

    #[test]
    fn headings_tags_code() {
        assert_eq!(kinds("## Notes"), [("## Notes", Kind::Heading)]);
        assert_eq!(kinds("#notheading"), [("#notheading", Kind::Tag)]);
        assert_eq!(kinds("issue #42 only"), []);
        assert_eq!(kinds("`[[x]] @y`"), [("`[[x]] @y`", Kind::Code)]);
        assert_eq!(kinds("[site](http://a.b)"), [("[site](http://a.b)", Kind::Link)]);
    }

    #[test]
    fn numbered_items_are_not_tasks() {
        assert_eq!(
            kinds("1. [ ] step"),
            [("1.", Kind::ListMarker)]
        );
        assert_eq!(kinds("- [x]done"), [("-", Kind::ListMarker)]);
    }

    #[test]
    fn state_at_lines() {
        let lines = ["# a", "```", "- [ ] code", "```", "- [ ] real"];
        assert!(!can_hold_task(state_at(lines, 2)));
        assert!(can_hold_task(state_at(lines, 4)));
        assert!(can_hold_task(state_at(lines, 0)));
    }

    #[test]
    fn multi_line_state() {
        let (_, s) = scan_line(LineState::Start, "---");
        assert_eq!(s, LineState::Frontmatter);
        let (_, s) = scan_line(s, "---");
        assert_eq!(s, LineState::Normal);

        let (_, s) = scan_line(LineState::Normal, "```rust");
        assert!(matches!(s, LineState::Fence { .. }));
        let (spans, s2) = scan_line(s, "@not_a_mention");
        assert_eq!(spans, [(0..14, Kind::Code)]);
        let (_, s3) = scan_line(s2, "```");
        assert_eq!(s3, LineState::Normal);

        let (_, s) = scan_line(LineState::Normal, "text %% start");
        assert_eq!(s, LineState::Comment);
        let (spans, s) = scan_line(s, "hidden %% @shown");
        assert_eq!(spans, [(0..9, Kind::Comment), (10..16, Kind::Mention)]);
        assert_eq!(s, LineState::Normal);
    }
}
