//! Editor highlighting on top of `obsession_core::syntax`.
//!
//! The editor re-highlights from the first edited line downward, and only
//! for the lines it lays out, so typing costs a few lines of work.

use std::ops::Range;

use iced::advanced::text::highlighter::{self, Format};
use iced::{Font, Theme};

use obsession_core::syntax::{Kind, LineState, scan_line};

use crate::theme;

pub struct Markdown {
    /// `states[n]` is the state at the start of line `n`.
    states: Vec<LineState>,
    current: usize,
}

impl highlighter::Highlighter for Markdown {
    type Settings = ();
    type Highlight = Kind;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Kind)>;

    fn new(_: &Self::Settings) -> Self {
        Self {
            states: vec![LineState::Start],
            current: 0,
        }
    }

    fn update(&mut self, _: &Self::Settings) {}

    fn change_line(&mut self, line: usize) {
        let line = line.min(self.states.len() - 1);
        self.states.truncate(line + 1);
        self.current = line;
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let state = self.states[self.current.min(self.states.len() - 1)];
        let (spans, next) = scan_line(state, line);
        self.current += 1;
        if self.states.len() <= self.current {
            self.states.push(next);
        } else {
            self.states[self.current] = next;
        }
        spans.into_iter()
    }

    fn current_line(&self) -> usize {
        self.current
    }
}

/// Colour only: a bold face isn't guaranteed for the editor font, and a
/// fallback font would break the monospace grid (`[ ]` turned into `[]`).
pub fn format(kind: &Kind, _: &Theme) -> Format<Font> {
    let color = match kind {
        Kind::Heading => theme::HEADING,
        Kind::ListMarker | Kind::Quote | Kind::Frontmatter => theme::MUTED,
        Kind::TaskBox => theme::ACCENT,
        Kind::Link => theme::LINK,
        Kind::Mention => theme::MENTION,
        Kind::BlockId | Kind::Comment => theme::FAINT,
        Kind::Tag => theme::TAG,
        Kind::Code => theme::CODE,
    };
    Format {
        color: Some(color),
        font: None,
    }
}
