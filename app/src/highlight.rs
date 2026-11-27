//! Editor highlighting on top of `obsession_core::syntax`.
//!
//! The editor re-highlights from the first edited line downward, and only
//! for the lines it lays out, so typing costs a few lines of work.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use iced::advanced::text::highlighter::{self, Format};
use iced::{Font, Theme};

use obsession_core::links::{self, LinkState};
use obsession_core::status;
use obsession_core::syntax::{Kind, LineState, scan_line};

use crate::theme;

/// How each `[[link]]` in the open note stands (keyed by
/// [`links::dest_key`]), shared by the editor, Live Preview and Split.
#[derive(Debug, Clone, Default)]
pub struct LinkStates(Arc<HashMap<String, LinkState>>);

impl LinkStates {
    pub fn new(map: HashMap<String, LinkState>) -> Self {
        Self(Arc::new(map))
    }

    /// These states with `updates` (keyed as `get` looks them up), or
    /// `None` if nothing changes.
    pub fn with(&self, updates: impl IntoIterator<Item = (String, LinkState)>) -> Option<Self> {
        let mut map: Option<HashMap<String, LinkState>> = None;
        for (key, state) in updates {
            let key = links::dest_key(&key);
            if self.0.get(&key) != Some(&state) {
                map.get_or_insert_with(|| (*self.0).clone()).insert(key, state);
            }
        }
        map.map(Self::new)
    }

    /// The state of a link to `dest` (`target#sub`, maybe `|alias`).
    /// Unknown links (typed since the last check) count as fine.
    pub fn get(&self, dest: &str) -> LinkState {
        self.0.get(&links::dest_key(dest)).copied().unwrap_or(LinkState::Ok)
    }
}

impl PartialEq for LinkStates {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

/// What the highlighter needs from the note: its status format and its
/// links' states. When either changes, the whole note is re-coloured.
#[derive(Debug, Clone, PartialEq)]
pub struct Statuses {
    format: status::Format,
    pub links: LinkStates,
    /// The state of the editor's first line: `Start` for a whole note, and
    /// `Normal` for a Live Preview block in the middle of one.
    first: LineState,
}

impl Statuses {
    pub fn new(format: &status::Format) -> Self {
        Self {
            format: format.clone(),
            links: LinkStates::default(),
            first: LineState::Start,
        }
    }

    pub fn starting(mut self, first: LineState) -> Self {
        self.first = first;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Mark {
    Syntax(Kind),
    Open,
    Finished,
    Unknown,
    Link(LinkState),
    /// An `@name` with no crew card (amber, as Live Preview draws it).
    UnknownMention,
}

pub struct Markdown {
    /// `states[n]` is the state at the start of line `n`.
    states: Vec<LineState>,
    current: usize,
    statuses: Statuses,
}

impl highlighter::Highlighter for Markdown {
    type Settings = Statuses;
    type Highlight = Mark;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Mark)>;

    fn new(settings: &Self::Settings) -> Self {
        Self {
            states: vec![settings.first],
            current: 0,
            statuses: settings.clone(),
        }
    }

    fn update(&mut self, settings: &Self::Settings) {
        self.statuses = settings.clone();
        self.states = vec![settings.first];
        self.current = 0;
    }

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
        let format = &self.statuses.format;
        spans
            .into_iter()
            .map(|(range, kind)| {
                let mark = match kind {
                    Kind::TaskBox => {
                        let ch = line[range.start + 1..].chars().next().unwrap_or(' ');
                        match format.index_of(ch) {
                            None => Mark::Unknown,
                            Some(_) if format.is_finished(ch) => Mark::Finished,
                            Some(_) => Mark::Open,
                        }
                    }
                    Kind::Link => {
                        let text = &line[range.clone()];
                        let inner = text.trim_start_matches('!');
                        match inner.strip_prefix("[[").and_then(|i| i.strip_suffix("]]")) {
                            Some(dest) => Mark::Link(self.statuses.links.get(dest)),
                            None => Mark::Syntax(Kind::Link),
                        }
                    }
                    // The same check as a drawn line: an `@name` the link
                    // states mark missing has no crew card.
                    Kind::Mention if self.statuses.links.get(&line[range.clone()]) == LinkState::Missing => Mark::UnknownMention,
                    kind => Mark::Syntax(kind),
                };
                (range, mark)
            })
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn current_line(&self) -> usize {
        self.current
    }
}

/// Markup that Live Preview's edited line shows in the mono font, so it
/// looks as it does in Source mode (`[ ]` keeps its width instead of
/// collapsing to `[]` in a proportional font).
pub fn is_markup(kind: Kind) -> bool {
    matches!(kind, Kind::TaskBox | Kind::ListMarker | Kind::Quote | Kind::Code)
}

/// [`format`] plus the mono font for markup, for Live Preview's edited line.
pub fn live_format(mark: &Mark, theme: &Theme) -> Format<Font> {
    let markup = match mark {
        Mark::Open | Mark::Finished | Mark::Unknown => true,
        Mark::Link(_) | Mark::UnknownMention => false,
        Mark::Syntax(kind) => is_markup(*kind),
    };
    Format {
        font: markup.then_some(theme::MONO_FONT),
        ..format(mark, theme)
    }
}

/// A link's colour: normal, dim when the note doesn't exist yet, amber
/// when it's ambiguous or its `#heading` is missing.
pub fn link_color(state: LinkState) -> iced::Color {
    match state {
        LinkState::Ok => theme::LINK,
        LinkState::Missing => theme::LINK_MISSING,
        LinkState::Ambiguous | LinkState::MissingSection => theme::WARNING,
    }
}

/// Colour only: a bold face isn't guaranteed for the editor font, and a
/// fallback font would break the monospace grid (`[ ]` turned into `[]`).
pub fn format(mark: &Mark, _: &Theme) -> Format<Font> {
    let color = match mark {
        Mark::Open => theme::ACCENT,
        Mark::Finished => theme::SUCCESS,
        Mark::Unknown => theme::DANGER,
        Mark::Link(state) => link_color(*state),
        Mark::UnknownMention => theme::WARNING,
        Mark::Syntax(kind) => match kind {
            Kind::Heading => theme::HEADING,
            Kind::ListMarker | Kind::Quote | Kind::Frontmatter => theme::MUTED,
            Kind::TaskBox => theme::ACCENT,
            Kind::Link => theme::LINK,
            Kind::Mention => theme::MENTION,
            Kind::BlockId | Kind::Comment => theme::FAINT,
            Kind::Tag => theme::TAG,
            Kind::Code => theme::CODE,
        },
    };
    Format {
        color: Some(color),
        font: None,
    }
}
