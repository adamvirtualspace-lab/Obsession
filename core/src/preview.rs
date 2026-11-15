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
//! - Single line breaks are kept, as Obsidian does by default ("strict line
//!   breaks" off): each text line ends in a markdown hard break.
//! - `> [!type] Title` callouts get their icon and a bold title.
//! - `==highlights==`, `@people` and `#tags` become links with the private
//!   schemes `hl:`, `at:` and `tag:`; the app's viewer turns those back into
//!   plain text with a highlight or a colour. Trailing `^block-ids` are hidden.

use crate::status::{Format, task_box};

/// The URI scheme used for wiki links in the preview.
pub const WIKI_SCHEME: &str = "wiki:";
/// The URI scheme used for status chips: `status:<line>`.
pub const STATUS_SCHEME: &str = "status:";
/// Private schemes for pieces the viewer restyles (not clickable).
pub const HIGHLIGHT_SCHEME: &str = "hl:";
pub const MENTION_SCHEME: &str = "at:";
pub const TAG_SCHEME: &str = "tag:";

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

        // Hide a trailing ` ^block-id`, as Obsidian's reading view does.
        let without_id;
        let line = match (!in_comment)
            .then(|| crate::live::trailing_block_id(trimmed))
            .flatten()
        {
            Some(cut) => {
                without_id = format!("{}{}", &trimmed[..cut], &line[trimmed.len()..]);
                without_id.as_str()
            }
            None => line,
        };
        let trimmed = line.trim_end_matches(['\r', '\n']);

        // Keep single line breaks: end non-blank text lines with a hard
        // break (two spaces) before the line ending.
        let with_break;
        let line = if !in_comment && line.len() > trimmed.len() && keeps_break(trimmed) {
            let ending = &line[trimmed.len()..];
            with_break = format!("{trimmed}  {ending}");
            with_break.as_str()
        } else {
            line
        };
        let trimmed = line.trim_end_matches(['\r', '\n']);

        if !in_comment && let Some(callout) = callout_title(trimmed) {
            let ending = &line[trimmed.len()..];
            convert_line(&format!("{callout}{ending}"), &mut in_comment, &mut out);
            continue;
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

/// Whether a line should keep its line break: any non-blank line except
/// table rows (a break there would end the table) and headings.
fn keeps_break(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('|') && !t.starts_with('#') && !line.ends_with("  ")
}

/// `> [!warning] Render farm` becomes `> **⚠ Render farm**` (quotes kept).
fn callout_title(line: &str) -> Option<String> {
    let lead = line.len() - line.trim_start().len();
    let rest = &line[lead..];
    if !rest.starts_with('>') {
        return None;
    }
    let quotes = rest.len() - rest.trim_start_matches(['>', ' ']).len();
    let body = &rest[quotes..];
    let inner = body.strip_prefix("[!")?;
    let close = inner.find(']')?;
    let kind = inner[..close].to_ascii_lowercase();
    let title = inner[close + 1..].trim_start_matches(['+', '-']).trim();
    let title = if title.is_empty() {
        crate::live::capitalize(&kind)
    } else {
        title.to_owned()
    };
    let icon = crate::live::callout_icon(&kind);
    Some(format!("{}**{icon} {title}**  ", &line[..lead + quotes]))
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
            // A markdown link (including the chips made above): copied as
            // is, so nothing inside its label or URL is touched.
            b'[' => match markdown_link_len(&line[i..]) {
                Some(len) => i += len,
                None => i += 1,
            },
            // Bare URLs: `#` and `@` inside them aren't tags or people.
            b'h' if line[i..].starts_with("http://") || line[i..].starts_with("https://") => {
                i += line[i..].find(char::is_whitespace).unwrap_or(line.len() - i);
            }
            b'=' if line[i..].starts_with("==") => {
                let inner_start = i + 2;
                let close = line[inner_start..].find("==");
                match close {
                    Some(len)
                        if len > 0
                            && !line[inner_start..inner_start + len].contains('[')
                            && !line[inner_start..].starts_with(' ')
                            && !line[..inner_start + len].ends_with(' ') =>
                    {
                        let inner = &line[inner_start..inner_start + len];
                        out.push_str(&line[copied..i]);
                        out.push_str(&format!("[{inner}](<{HIGHLIGHT_SCHEME}>)"));
                        i = inner_start + len + 2;
                        copied = i;
                    }
                    _ => i += 2,
                }
            }
            b'@' | b'#' if crate::live::word_start(bytes, i) => {
                let len = line[i + 1..]
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'/'))
                    .count();
                let name = &line[i + 1..i + 1 + len];
                let is_tag = bytes[i] == b'#';
                if len == 0 || (is_tag && name.bytes().all(|c| c.is_ascii_digit())) {
                    i += 1;
                    continue;
                }
                let scheme = if is_tag { TAG_SCHEME } else { MENTION_SCHEME };
                out.push_str(&line[copied..i]);
                out.push_str(&format!(
                    "[{}{name}](<{scheme}{name}>)",
                    if is_tag { "\\#" } else { "@" }
                ));
                i += 1 + len;
                copied = i;
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

/// Length of a `[label](url)` link at the start of `s`.
fn markdown_link_len(s: &str) -> Option<usize> {
    let close = s.find("](")?;
    if s[1..close].contains('[') && !s[1..close].contains("\\[") {
        return None;
    }
    let end = close + 2 + s[close + 2..].find(')')?;
    Some(end + 1)
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
            "# T\n- [🔳](<status:1>) a [b](<wiki:b>)  \n  - [✅](<status:2>) c  \n- [⚠ \\[?\\]](<status:3>) d  \n"
        );
        // Not in code, and not numbered items.
        let src = "```\n- [ ] code\n```\n1. [ ] step\n";
        assert_eq!(
            to_markdown_default(src),
            "```\n- [ ] code\n```\n1. [ ] step  \n"
        );
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
            "```\n[[not a link]]\n```\n`[[inline]]` and [real](<wiki:real>)  \n"
        );
    }

    #[test]
    fn removes_comments_across_lines() {
        let src = "a %%hidden%% b\nstart %%\nlayout 1,2\n%% end\n";
        assert_eq!(to_markdown(src, &Format::builtin()), "a  b  \nstart \n\n end\n");
    }

    #[test]
    fn keeps_embeds_and_frontmatter() {
        let src = "---\ntags: [[x]]\n---\n![[image.png]]\n";
        assert_eq!(
            to_markdown(src, &Format::builtin()),
            "---\ntags: [[x]]\n---\n![[image.png]]  \n"
        );
    }

    #[test]
    fn keeps_crlf() {
        assert_eq!(
            to_markdown_default("a\r\n[[b]]\r\n"),
            "a  \r\n[b](<wiki:b>)  \r\n"
        );
    }

    #[test]
    fn marks_highlights_people_tags() {
        assert_eq!(
            to_markdown_default("Ask @ika about ==rim light== #anim/polish ^a1"),
            "Ask [@ika](<at:ika>) about [rim light](<hl:>) [\\#anim/polish](<tag:anim/polish>)"
        );
        // Not inside code, links, URLs or emails; not issue numbers.
        let src = "`@x #y` [a #b](http://x.com/#c) https://x.com/#d bob@mail.com #42";
        assert_eq!(to_markdown_default(src), src);
        // Chips stay intact.
        assert_eq!(
            to_markdown_default("- [x] done @ika"),
            "- [✅](<status:0>) done [@ika](<at:ika>)"
        );
    }

    #[test]
    fn line_breaks_and_callouts() {
        assert_eq!(
            to_markdown_default("Warm regards,\nAdams\n\n| a |\n|---|\n# H\n"),
            "Warm regards,  \nAdams  \n\n| a |\n|---|\n# H\n"
        );
        assert_eq!(
            to_markdown_default("> [!warning] Render farm\n> Book it.\n"),
            "> **⚠ Render farm**  \n> Book it.  \n"
        );
        assert_eq!(to_markdown_default("> [!tip]\n"), "> **💡 Tip**  \n");
    }
}
