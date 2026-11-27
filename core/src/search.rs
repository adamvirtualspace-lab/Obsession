//! Vault search: every note's text in RAM, searched line by line.
//!
//! Built the first time search is used (not at startup), on a background
//! thread, then kept current as notes change. Matching ignores case
//! (`fold`): each note keeps a folded copy whose byte offsets match the
//! original, so a hit's position points straight into the real line. The
//! copy doubles what search holds (3.5 MB of text is 7 MB, plus about
//! 1 MB of paths for 10,000 notes) but keeps a query a few milliseconds:
//! folding every note on each query took five times as long.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

/// The longest line shown in a result.
const SNIPPET: usize = 180;

struct Doc {
    rel: Box<str>,
    text: String,
    /// `text` folded (`fold`), the same length.
    lower: String,
}

#[derive(Default)]
pub struct SearchIndex {
    docs: Vec<Doc>,
    by_rel: HashMap<Box<str>, usize>,
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rel: Box<str>,
    /// 0-based line in the note.
    pub line: usize,
    /// The line, trimmed and cut to a readable length.
    pub text: String,
    /// Where the words are in `text`.
    pub ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub hits: Vec<Hit>,
    /// Every matching line, shown or not.
    pub total: usize,
    /// Notes with at least one match.
    pub notes: usize,
}

impl SearchIndex {
    /// Reads every note. Unreadable ones are skipped.
    pub fn build<'a>(root: &Path, notes: impl IntoIterator<Item = &'a str>) -> Self {
        let mut index = Self::default();
        for rel in notes {
            if let Ok(text) = std::fs::read_to_string(root.join(rel)) {
                index.insert(rel, text);
            }
        }
        index.docs.shrink_to_fit();
        index.by_rel.shrink_to_fit();
        index
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Bytes of note text held, folded copies included.
    pub fn bytes(&self) -> usize {
        self.docs.iter().map(|d| d.text.len() + d.lower.len()).sum()
    }

    pub fn update(&mut self, rel: &str, text: &str) {
        self.insert(rel, text.to_owned());
    }

    fn insert(&mut self, rel: &str, text: String) {
        let lower = fold(&text);
        match self.by_rel.get(rel) {
            Some(&i) => {
                self.docs[i].text = text;
                self.docs[i].lower = lower;
            }
            None => {
                self.by_rel.insert(rel.into(), self.docs.len());
                self.docs.push(Doc { rel: rel.into(), text, lower });
            }
        }
    }

    pub fn remove(&mut self, rel: &str) {
        let Some(i) = self.by_rel.remove(rel) else { return };
        self.docs.swap_remove(i);
        if let Some(moved) = self.docs.get(i) {
            self.by_rel.insert(moved.rel.clone(), i);
        }
    }

    /// A note moved from `old` to `new` with its text unchanged.
    pub fn rename(&mut self, old: &str, new: &str) {
        let Some(i) = self.by_rel.remove(old) else { return };
        self.docs[i].rel = new.into();
        self.by_rel.insert(new.into(), i);
    }

    /// Lines containing every word of `query` (ignoring case, `fold`).
    /// Notes whose name contains every word come first; then by path. At
    /// most `limit` hits are returned, but all are counted.
    pub fn query(&self, query: &str, limit: usize) -> Results {
        let words: Vec<String> = query.split_whitespace().map(fold).collect();
        let mut out = Results::default();
        if words.is_empty() {
            return out;
        }
        let mut order: Vec<(bool, &Doc)> = self
            .docs
            .iter()
            .map(|d| {
                let name = fold(d.rel.rsplit('/').next().unwrap_or(&d.rel));
                (!words.iter().all(|w| name.contains(w.as_str())), d)
            })
            .collect();
        order.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.rel.cmp(&b.1.rel)));
        for (_, doc) in order {
            // A quick check on the whole note before looking at lines.
            if !words.iter().all(|w| doc.lower.contains(w.as_str())) {
                continue;
            }
            let mut found = false;
            let mut start = 0;
            for (i, lower) in doc.lower.split('\n').enumerate() {
                let line = &doc.text[start..start + lower.len()];
                start += lower.len() + 1;
                if !words.iter().all(|w| lower.contains(w.as_str())) {
                    continue;
                }
                found = true;
                out.total += 1;
                if out.hits.len() < limit {
                    out.hits.push(hit(&doc.rel, i, line, lower, &words));
                }
            }
            out.notes += usize::from(found);
        }
        out
    }
}

impl SearchIndex {
    /// The note `rel`'s unlinked mentions: lines in other notes that write
    /// its name (no folder, no `.md`; case ignored, `fold`) as whole words
    /// outside links and code (`links::unlinked_mentions`). By path; at
    /// most `limit` returned, all counted.
    pub fn unlinked(&self, rel: &str, limit: usize) -> Results {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let name = fold(name.strip_suffix(".md").unwrap_or(name));
        let mut out = Results::default();
        if name.trim().is_empty() {
            return out;
        }
        let mut docs: Vec<&Doc> = self.docs.iter().filter(|d| *d.rel != *rel && d.lower.contains(&name)).collect();
        docs.sort_by(|a, b| a.rel.cmp(&b.rel));
        for doc in docs {
            let found = crate::links::unlinked_mentions(&doc.text, &doc.lower, &name);
            if found.is_empty() {
                continue;
            }
            out.notes += 1;
            for (line, ranges) in found {
                out.total += 1;
                if out.hits.len() < limit {
                    out.hits.push(hit_at(&doc.rel, line, &doc.text, &ranges));
                }
            }
        }
        out
    }
}

/// `text` with its letters lowercased, for matching that ignores case.
/// Each character keeps its UTF-8 length, so a position in the result is
/// the same position in `text`: `É` → `é`, `Σ` → `σ`, `Ж` → `ж`. The few
/// whose lowercase is longer or shorter (`İ`, the Kelvin sign) stay as
/// they are, and a final `ς` becomes `σ` so `ΟΔΟΣ` finds `οδος`.
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    text.chars().map(fold_char).collect()
}

fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    if c == 'ς' {
        return 'σ';
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) if l.len_utf8() == c.len_utf8() => l,
        _ => c,
    }
}

/// A hit on `line` of `text`, at `ranges` (bytes in the line).
fn hit_at(rel: &str, line: usize, text: &str, ranges: &[Range<usize>]) -> Hit {
    let raw = text.split('\n').nth(line).unwrap_or("");
    let raw = raw.strip_suffix('\r').unwrap_or(raw);
    let lead = raw.len() - raw.trim_start().len();
    let first = ranges.first().map_or(lead, |r| r.start);
    let mut from = lead.max(first.saturating_sub(40));
    while !raw.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = raw.trim_end().len().max(from);
    if to - from > SNIPPET {
        to = from + SNIPPET;
        while !raw.is_char_boundary(to) {
            to -= 1;
        }
    }
    Hit {
        rel: rel.into(),
        line,
        text: raw[from..to].to_owned(),
        ranges: ranges
            .iter()
            .filter(|r| r.start >= from && r.end <= to)
            .map(|r| r.start - from..r.end - from)
            .collect(),
    }
}

fn hit(rel: &str, line: usize, text: &str, lower: &str, words: &[String]) -> Hit {
    let text = text.strip_suffix('\r').unwrap_or(text);
    let lower = &lower[..text.len()];
    // Trim, and if long, start a little before the first match.
    let lead = text.len() - text.trim_start().len();
    let first = words.iter().filter_map(|w| lower.find(w.as_str())).min().unwrap_or(lead);
    let mut from = lead.max(first.saturating_sub(40));
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = text.trim_end().len().max(from);
    if to - from > SNIPPET {
        to = from + SNIPPET;
        while !text.is_char_boundary(to) {
            to -= 1;
        }
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for w in words {
        let mut at = from;
        while let Some(i) = lower[at..to].find(w.as_str()) {
            let s = at + i;
            ranges.push(s - from..s - from + w.len());
            at = s + w.len().max(1);
        }
    }
    ranges.sort_by_key(|r| r.start);
    // Merge overlaps.
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    let prefix = if from > lead { "…" } else { "" };
    let suffix = if to < text.trim_end().len() { "…" } else { "" };
    let shift = prefix.len();
    Hit {
        rel: rel.into(),
        line,
        text: format!("{prefix}{}{suffix}", &text[from..to]),
        ranges: merged.into_iter().map(|r| r.start + shift..r.end + shift).collect(),
    }
}

#[cfg(test)]
mod unlinked_tests {
    use super::*;

    #[test]
    fn folds_case_beyond_ascii_keeping_offsets() {
        assert_eq!(fold("ÉTÉ Été été"), "été été été");
        assert_eq!(fold("ΟΔΟΣ οδος"), "οδοσ οδοσ");
        assert_eq!(fold("Жук"), "жук");
        // Lowercase of a different length: left as it is.
        assert_eq!(fold("İ \u{212A} K"), "İ \u{212A} k");
        for text in ["Straße ÀÉÎÕÜ ǅ ΣΑΣ İstanbul \u{212A}elvin 東京 🎬", "plain ASCII"] {
            let folded = fold(text);
            assert_eq!(folded.len(), text.len());
            assert!(text.char_indices().all(|(i, _)| folded.is_char_boundary(i)));
        }
    }

    #[test]
    fn search_and_unlinked_mentions_ignore_case_beyond_ascii() {
        let mut index = SearchIndex::default();
        index.update("Équipe.md", "# Équipe");
        index.update("notes.md", "Réunion avec l'ÉQUIPE demain\nnothing");
        let r = index.query("équipe RÉUNION", 10);
        assert_eq!((r.total, r.notes), (1, 1));
        let hit = &r.hits[0];
        assert_eq!(hit.text, "Réunion avec l'ÉQUIPE demain");
        let marked: Vec<&str> = hit.ranges.iter().map(|range| &hit.text[range.clone()]).collect();
        assert_eq!(marked, ["Réunion", "ÉQUIPE"]);
        // The note named "Équipe" comes first.
        assert_eq!(&*index.query("ÉQUIPE", 10).hits[0].rel, "Équipe.md");
        let unlinked = index.unlinked("Équipe.md", 10);
        assert_eq!(unlinked.total, 1);
        assert_eq!(&unlinked.hits[0].text[unlinked.hits[0].ranges[0].clone()], "ÉQUIPE");
    }

    #[test]
    fn lists_unlinked_mentions_of_a_note() {
        let mut index = SearchIndex::default();
        index.update("anim/Shot 40.md", "Shot 40 is me");
        index.update("b.md", "Linked: [[Shot 40]]\nNot linked: shot 40 here");
        index.update("a.md", "nothing");
        let r = index.unlinked("anim/Shot 40.md", 10);
        assert_eq!((r.total, r.notes), (1, 1));
        let hit = &r.hits[0];
        assert_eq!((&*hit.rel, hit.line, hit.text.as_str()), ("b.md", 1, "Not linked: shot 40 here"));
        assert_eq!(hit.ranges, [12..19]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_lines_with_every_word() {
        let mut index = SearchIndex::default();
        index.update("anim/sh040.md", "# Anim\r\n- [ ] Polish the WALK cycle @ika\r\n- [x] Blocking\r\n");
        index.update("notes/walk.md", "Walk notes\nnothing here\n");
        index.update("fx/water.md", "foam and walk\n");
        let r = index.query("walk", 10);
        assert_eq!((r.total, r.notes), (3, 3));
        // The note named "walk" first, then by path.
        let order: Vec<&str> = r.hits.iter().map(|h| &*h.rel).collect();
        assert_eq!(order, ["notes/walk.md", "anim/sh040.md", "fx/water.md"]);
        let h = &r.hits[1];
        assert_eq!((h.line, h.text.as_str()), (1, "- [ ] Polish the WALK cycle @ika"));
        assert_eq!(&h.text[h.ranges[0].clone()], "WALK");

        let both = index.query("polish  ika", 10);
        assert_eq!(both.total, 1);
        assert_eq!(both.hits[0].ranges.len(), 2);
        assert_eq!(index.query("polish nothing", 10).total, 0);
        assert_eq!(index.query("  ", 10), Results::default());

        // Kept current.
        index.update("fx/water.md", "no match\n");
        index.remove("notes/walk.md");
        assert_eq!(index.query("walk", 10).total, 1);
        assert_eq!(index.len(), 2);
        assert_eq!(index.query("anim", 1).hits.len(), 1);
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let mut index = SearchIndex::default();
        let line = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        index.update("x.md", &line);
        let h = &index.query("needle", 5).hits[0];
        assert!(h.text.starts_with('…') && h.text.ends_with('…'));
        assert_eq!(&h.text[h.ranges[0].clone()], "needle");
        assert!(h.text.chars().count() <= SNIPPET + 2);
    }
}
