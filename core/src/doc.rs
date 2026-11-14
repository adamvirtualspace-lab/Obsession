//! A note as a list of lines, each with its own line ending, so a file is
//! written back byte for byte (mixed `\n` / `\r\n`, no final newline, …).
//! Live Preview edits go through here one line range at a time.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The last line of a file without a final newline.
    None,
    Lf,
    CrLf,
}

impl Ending {
    pub fn as_str(self) -> &'static str {
        match self {
            Ending::None => "",
            Ending::Lf => "\n",
            Ending::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    lines: Vec<String>,
    endings: Vec<Ending>,
    /// Used for brand-new lines: the file's first line ending.
    default_ending: Ending,
}

impl Doc {
    pub fn from_text(text: &str) -> Self {
        let mut lines = Vec::new();
        let mut endings = Vec::new();
        for piece in text.split_inclusive('\n') {
            let (line, ending) = if let Some(line) = piece.strip_suffix("\r\n") {
                (line, Ending::CrLf)
            } else if let Some(line) = piece.strip_suffix('\n') {
                (line, Ending::Lf)
            } else {
                (piece, Ending::None)
            };
            lines.push(line.to_owned());
            endings.push(ending);
        }
        // A file ending in a newline has an empty last line, as editors show.
        if endings.last() != Some(&Ending::None) {
            lines.push(String::new());
            endings.push(Ending::None);
        }
        let default_ending = endings
            .iter()
            .copied()
            .find(|e| *e != Ending::None)
            .unwrap_or(if cfg!(windows) { Ending::CrLf } else { Ending::Lf });
        Self {
            lines,
            endings,
            default_ending,
        }
    }

    pub fn to_text(&self) -> String {
        let len = self.lines.iter().map(|l| l.len() + 2).sum();
        let mut out = String::with_capacity(len);
        for (line, ending) in self.lines.iter().zip(&self.endings) {
            out.push_str(line);
            out.push_str(ending.as_str());
        }
        out
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn line(&self, index: usize) -> &str {
        self.lines.get(index).map_or("", String::as_str)
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Replaces lines `range` with `new` (at least one line). Line endings
    /// are kept positionally: the last new line takes the ending of the last
    /// replaced line, so "no final newline" survives; other new lines take
    /// the replaced line's ending at the same position, or the file default.
    pub fn replace(&mut self, range: Range<usize>, new: Vec<String>) {
        assert!(!new.is_empty(), "a replacement needs at least one line");
        let range = range.start.min(self.lines.len())..range.end.min(self.lines.len());
        let old_endings: Vec<Ending> = self.endings[range.clone()].to_vec();
        let last_old = old_endings.last().copied().unwrap_or(self.default_ending);
        let count = new.len();
        let endings = (0..count).map(|i| {
            if i + 1 == count {
                last_old
            } else {
                match old_endings.get(i) {
                    Some(Ending::None) | None => self.default_ending,
                    Some(e) => *e,
                }
            }
        });
        let endings: Vec<Ending> = endings.collect();
        self.lines.splice(range.clone(), new);
        self.endings.splice(range, endings);
    }

    /// The text of lines `range` joined with `\n`, for an editor.
    pub fn joined(&self, range: Range<usize>) -> String {
        self.lines[range].join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_byte_for_byte() {
        for text in ["", "a", "a\n", "a\r\nb\r\n", "a\nb\r\nc", "\n\n", "x\r\n\r\n"] {
            assert_eq!(Doc::from_text(text).to_text(), text, "{text:?}");
        }
    }

    #[test]
    fn replace_keeps_endings() {
        let mut doc = Doc::from_text("a\r\nb\r\nc");
        doc.replace(1..2, vec!["b1".into(), "b2".into()]);
        assert_eq!(doc.to_text(), "a\r\nb1\r\nb2\r\nc");
        doc.replace(3..4, vec!["c".into(), "d".into()]);
        assert_eq!(doc.to_text(), "a\r\nb1\r\nb2\r\nc\r\nd");
        doc.replace(0..2, vec!["ab".into()]);
        assert_eq!(doc.to_text(), "ab\r\nb2\r\nc\r\nd");
    }

    #[test]
    fn trailing_newline_is_an_empty_last_line() {
        let doc = Doc::from_text("a\n");
        assert_eq!(doc.len(), 2);
        assert_eq!(doc.line(1), "");
        assert!(Doc::from_text("").is_empty());
    }
}
