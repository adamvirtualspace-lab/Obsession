//! Vault search (Ctrl+Shift+F): type words, see every line that has them
//! all, Enter or a click opens the note at that line.
//!
//! The search itself is core's `search::SearchIndex`, built the first time
//! this opens.

use iced::widget::text::Span;
use iced::widget::{Id, button, column, container, opaque, rich_text, row, scrollable, span, text, text_input};
use iced::{Element, Fill, Font, Length};

use obsession_core::search::{Hit, Results, SearchIndex};

use crate::theme;

/// Hits kept per search (all are counted).
pub const LIMIT: usize = 300;

pub fn input_id() -> Id {
    Id::new("find")
}

#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    Submit,
    Pick(usize),
    Close,
}

#[derive(Debug, Default)]
pub struct Find {
    pub query: String,
    results: Results,
    selected: usize,
}

impl Find {
    pub fn set_query(&mut self, index: Option<&SearchIndex>, query: String) {
        self.query = query;
        self.selected = 0;
        self.refresh(index);
    }

    /// Runs the search again (the index arrived, or notes changed).
    pub fn refresh(&mut self, index: Option<&SearchIndex>) {
        self.results = index.map(|i| i.query(&self.query, LIMIT)).unwrap_or_default();
        self.selected = self.selected.min(self.results.hits.len().saturating_sub(1));
    }

    pub fn move_selection(&mut self, down: bool) {
        let len = self.results.hits.len();
        if len == 0 {
            return;
        }
        self.selected = if down { (self.selected + 1) % len } else { (self.selected + len - 1) % len };
    }

    pub fn hit(&self, i: usize) -> Option<&Hit> {
        self.results.hits.get(i)
    }

    pub fn selected(&self) -> Option<&Hit> {
        self.hit(self.selected)
    }

    pub fn view(&self, ready: bool) -> Element<'_, Message> {
        let input = text_input("Search every note…", &self.query)
            .id(input_id())
            .on_input(Message::Query)
            .on_submit(Message::Submit)
            .padding(10)
            .size(15)
            .style(theme::search);

        let r = &self.results;
        let status = if !ready {
            "Reading your notes for the first time…".to_owned()
        } else if self.query.trim().is_empty() {
            "Every word must be on the line; case doesn't matter. Enter opens the selected line.".to_owned()
        } else if r.total == 0 {
            "No matches".to_owned()
        } else {
            let lines = if r.total == 1 { "1 line".to_owned() } else { format!("{} lines", r.total) };
            let notes = if r.notes == 1 { "1 note".to_owned() } else { format!("{} notes", r.notes) };
            let shown = if r.hits.len() < r.total { format!(" (first {} shown)", r.hits.len()) } else { String::new() };
            format!("{lines} in {notes}{shown}")
        };

        let semibold = Font {
            weight: iced::font::Weight::Semibold,
            ..theme::UI_FONT
        };
        let rows = r.hits.iter().enumerate().map(|(n, hit)| {
            let path = hit.rel.trim_end_matches(".md");
            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
            let mut spans: Vec<Span<'_, (), Font>> = Vec::new();
            let mut at = 0;
            for range in &hit.ranges {
                if range.start > at {
                    spans.push(span(&hit.text[at..range.start]).color(theme::TEXT));
                }
                spans.push(span(&hit.text[range.clone()]).color(theme::HEADING).font(semibold).background(theme::HIGHLIGHT_BG));
                at = range.end;
            }
            if at < hit.text.len() {
                spans.push(span(&hit.text[at..]).color(theme::TEXT));
            }
            button(
                column![
                    row![
                        text(name.to_owned()).size(theme::SMALL).color(theme::LINK),
                        text(format!("{folder} · line {}", hit.line + 1)).size(11).color(theme::FAINT),
                    ]
                    .spacing(8)
                    .align_y(iced::Center),
                    rich_text(spans).size(theme::SMALL),
                ]
                .spacing(2),
            )
            .on_press(Message::Pick(n))
            .padding([5, 10])
            .width(Fill)
            .style(theme::row(n == self.selected))
            .into()
        });

        let panel = container(
            column![
                input,
                text(status).size(theme::SMALL).color(theme::MUTED),
                scrollable(column(rows).spacing(2)).height(Length::Shrink),
            ]
            .spacing(8),
        )
        .padding(10)
        .width(Length::Fixed(760.0))
        .max_height(620)
        .style(theme::popup);

        opaque(
            iced::widget::mouse_area(
                container(opaque(panel))
                    .padding(iced::Padding::new(0.0).top(70.0))
                    .center_x(Fill)
                    .height(Fill)
                    .style(theme::backdrop),
            )
            .on_press(Message::Close),
        )
    }
}
