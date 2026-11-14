//! Turns Obsidian syntax into plain markdown for the preview.
//!
//! Read-only: the result is only ever shown, never written back. Code fences,
//! inline code and frontmatter are passed through untouched.
//!
//! - `[[target|alias]]` becomes a link with the `wiki:` scheme, which the app
//!   resolves with [`crate::Vault::resolve`] when clicked.
//! - `%% comments %%` are removed, also across lines.
//! - `![[embeds]]` are left as plain text for now.
//! - A task's `[c]` becomes its status symbol, as a link with the `status:`
//!   scheme and the 0-based source line (`status:12`), so clicking it in the
//!   preview can change that one character. A character the format doesn't
//!   know shows as `⚠ [c]`.

use crate::status::{Format, task_box};

/// The URI scheme used for wiki links in the preview.
pub const WIKI_SCHEME: &str = "wiki:";
/// The URI scheme used for status chips: `status:<line>`.
pub const STATUS_SCHEME: &str = "status:";

pub fn to_markdown(source: &str, format: &Format) -> String {
    let mut out = String::with_capacity(source.len() + source.len() / 8);
    let mut fence: Option<(u8, usize)> = None;
    let mut in_comment = false;
    let mut in_frontmatter = false;

    for (index, line) in source.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_end_matches(['\r', '\n']);

        if index == 0 && trimmed == "---" {
            in_frontmatter = true;
            out.push_str(line);
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" || trimmed == "..." {
                in_frontmatter = false;
            }
            out.push_str(line);
            continue;
        }

        if !in_comment {
            if let Some(open) = fence {
                if closes_fence(trimmed, open) {
                    fence = None;
                }
                out.push_str(line);
                continue;
            }
            if let Some(open) = opens_fence(trimmed) {
                fence = Some(open);
                out.push_str(line);
                continue;
            }
        }

        if !in_comment && let Some((at, ch)) = task_box(trimmed) {
            let chip = match format.get(ch) {
                Some(status) => escape(&status.symbol),
                None => format!("⚠ \\[{}\\]", escape(&ch.to_string())),
            };
            let after = at + 2 + ch.len_utf8();
            let line = format!(
                "{}[{chip}](<{STATUS_SCHEME}{index}>){}",
                &line[..at],
                &line[after..]
            );
            convert_line(&line, &mut in_comment, &mut out);
            continue;
        }

        convert_line(line, &mut in_comment, &mut out);
    }
    out
}

/// Escapes characters that would change how a link label renders.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '>' | '%' | '~' | '=') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let line = line.trim_start_matches(' ');
    let first = *line.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let run = line.bytes().take_while(|&b| b == first).count();
    (run >= 3).then_some((first, run))
}

fn closes_fence(line: &str, (ch, len): (u8, usize)) -> bool {
    let line = line.trim();
    !line.is_empty() && line.bytes().all(|b| b == ch) && line.len() >= len
}

fn convert_line(line: &str, in_comment: &mut bool, out: &mut String) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut copied = 0;

    while i < bytes.len() {
        if *in_comment {
            if line[i..].starts_with("%%") {
                *in_comment = false;
                i += 2;
                copied = i;
            } else {
                i += 1;
                copied = i;
            }
            continue;
        }

        match bytes[i] {
            b'`' => {
                // Skip an inline code span: a run of N backticks up to the
                // next run of exactly N.
                let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let after = i + run;
                match find_backtick_run(&bytes[after..], run) {
                    Some(end) => i = after + end + run,
                    None => i = after,
                }
            }
            b'%' if line[i..].starts_with("%%") => {
                out.push_str(&line[copied..i]);
                *in_comment = true;
                i += 2;
                copied = i;
            }
            b'!' if line[i..].starts_with("![[") => {
                // Embeds stay as text: jump past them.
                i = line[i..].find("]]").map_or(bytes.len(), |end| i + end + 2);
            }
            b'[' if line[i..].starts_with("[[") => {
                let Some(end) = line[i + 2..].find("]]") else {
                    i += 2;
                    continue;
                };
                let inner = &line[i + 2..i + 2 + end];
                if let Some(link) = wiki_link(inner) {
                    out.push_str(&line[copied..i]);
                    out.push_str(&link);
                    i += 2 + end + 2;
                    copied = i;
                } else {
                    i += 2;
                }
            }
            _ => i += 1,
        }
    }

    if !*in_comment {
        out.push_str(&line[copied..]);
    } else if line.ends_with('\n') {
        // Keep line structure even inside a multi-line comment.
        out.push('\n');
    }
}

fn find_backtick_run(bytes: &[u8], len: usize) -> Option<usize> {
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

fn wiki_link(inner: &str) -> Option<String> {
    if inner.is_empty() || inner.contains(['<', '>', '[', ']', '\n']) {
        return None;
    }
    let (target, alias) = match inner.split_once('|') {
        Some((target, alias)) => (target.trim(), alias.trim()),
        None => (inner.trim(), ""),
    };
    let label = if alias.is_empty() {
        // Obsidian shows `file > heading` for heading links.
        target.replace('#', " > ")
    } else {
        alias.to_owned()
    };
    let label = label.replace('*', "\\*").replace('_', "\\_");
    Some(format!("[{label}](<{WIKI_SCHEME}{target}>)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_markdown_default(src: &str) -> String {
        to_markdown(src, &Format::builtin())
    }

    #[test]
    fn status_chips() {
        assert_eq!(
            to_markdown_default("# T\n- [ ] a [[b]]\n  - [x] c\n- [?] d\n"),
            "# T\n- [🔳](<status:1>) a [b](<wiki:b>)\n  - [✅](<status:2>) c\n- [⚠ \\[?\\]](<status:3>) d\n"
        );
        // Not in code, and not numbered items.
        let src = "```\n- [ ] code\n```\n1. [ ] step\n";
        assert_eq!(to_markdown_default(src), src);
    }

    #[test]
    fn wiki_links() {
        assert_eq!(to_markdown_default("see [[sh040]]"), "see [sh040](<wiki:sh040>)");
        assert_eq!(
            to_markdown_default("[[anim/sh040|Shot 40]]!"),
            "[Shot 40](<wiki:anim/sh040>)!"
        );
        assert_eq!(
            to_markdown_default("[[sh040#Notes]]"),
            "[sh040 > Notes](<wiki:sh040#Notes>)"
        );
    }

    #[test]
    fn leaves_code_alone() {
        let src = "```\n[[not a link]]\n```\n`[[inline]]` and [[real]]\n";
        assert_eq!(
            to_markdown(src, &Format::builtin()),
            "```\n[[not a link]]\n```\n`[[inline]]` and [real](<wiki:real>)\n"
        );
    }

    #[test]
    fn removes_comments_across_lines() {
        let src = "a %%hidden%% b\nstart %%\nlayout 1,2\n%% end\n";
        assert_eq!(to_markdown(src, &Format::builtin()), "a  b\nstart \n\n end\n");
    }

    #[test]
    fn keeps_embeds_and_frontmatter() {
        let src = "---\ntags: [[x]]\n---\n![[image.png]]\n";
        assert_eq!(to_markdown(src, &Format::builtin()), src);
    }

    #[test]
    fn keeps_crlf() {
        assert_eq!(to_markdown_default("a\r\n[[b]]\r\n"), "a\r\n[b](<wiki:b>)\r\n");
    }
}
