//! Vault search (Ctrl+Shift+F): type words, see every line that has them
//! all, Enter or a click opens the note at that line.
//!
//! The search itself is core's `search::SearchIndex`, built the first time
//! this opens. The results are a virtual list: only the rows on screen are
//! laid out (each costs about 28 KB of text layout while shown, so all 300
//! took 8.5 MB), every row the same height (`PITCH`).

use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::widget::text::{Span, Wrapping};
use iced::widget::{Id, Space, button, column, container, opaque, rich_text, row, scrollable, span, text, text_input};
use iced::{Element, Fill, Font, Length, Task};

use obsession_core::search::{Hit, Results, SearchIndex};

use crate::theme;

/// Hits kept per search (all are counted).
pub const LIMIT: usize = 300;

/// A result row's height, and the gap under it.
const ROW: f32 = 44.0;
const GAP: f32 = 2.0;
const PITCH: f32 = ROW + GAP;
/// The list's height before it has been scrolled once (about what the
/// popup leaves it).
const FIRST_HEIGHT: f32 = 560.0;

pub fn input_id() -> Id {
    Id::new("find")
}

fn list_id() -> Id {
    Id::new("find-results")
}

#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    Submit,
    Pick(usize),
    Close,
    Scrolled(Viewport),
}

#[derive(Debug, Default)]
pub struct Find {
    pub query: String,
    results: Results,
    selected: usize,
    /// The list's scroll offset and height, from `Message::Scrolled`.
    offset: f32,
    height: f32,
}

impl Find {
    /// A new query: its results start at the top.
    pub fn set_query(&mut self, index: Option<&SearchIndex>, query: String) -> Task<Message> {
        self.query = query;
        self.selected = 0;
        self.refresh(index);
        self.scroll_to(0.0)
    }

    pub fn scrolled(&mut self, viewport: Viewport) {
        self.offset = viewport.absolute_offset().y;
        self.height = viewport.bounds().height;
    }

    fn scroll_to(&mut self, y: f32) -> Task<Message> {
        self.offset = y;
        iced::widget::operation::scroll_to(list_id(), AbsoluteOffset { x: 0.0, y })
    }

    /// Runs the search again (the index arrived, or notes changed).
    pub fn refresh(&mut self, index: Option<&SearchIndex>) {
        self.results = index.map(|i| i.query(&self.query, LIMIT)).unwrap_or_default();
        self.selected = self.selected.min(self.results.hits.len().saturating_sub(1));
    }

    /// Up or Down: the selected row stays in view.
    pub fn move_selection(&mut self, down: bool) -> Task<Message> {
        let len = self.results.hits.len();
        if len == 0 {
            return Task::none();
        }
        self.selected = if down { (self.selected + 1) % len } else { (self.selected + len - 1) % len };
        let top = self.selected as f32 * PITCH;
        let height = self.shown_height();
        if top < self.offset {
            self.scroll_to(top)
        } else if top + PITCH > self.offset + height {
            self.scroll_to((top + PITCH - height).max(0.0))
        } else {
            Task::none()
        }
    }

    fn shown_height(&self) -> f32 {
        if self.height > 0.0 { self.height } else { FIRST_HEIGHT }
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
        // Only the rows on screen, one more above and below.
        let total = r.hits.len();
        let first = ((self.offset / PITCH).floor() as usize).min(total).saturating_sub(1);
        let last = (first + (self.shown_height() / PITCH).ceil() as usize + 3).min(total);
        let rows = r.hits[first..last].iter().enumerate().map(|(k, hit)| {
            let n = first + k;
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
            // One line each, cut at the popup's edge (the snippet starts
            // just before the first match), so every row is `ROW` tall.
            let body = column![
                row![
                    text(name.to_owned()).size(theme::SMALL).color(theme::LINK).wrapping(Wrapping::None),
                    text(format!("{folder} · line {}", hit.line + 1)).size(11).color(theme::FAINT).wrapping(Wrapping::None),
                ]
                .spacing(8)
                .align_y(iced::Center),
                rich_text(spans).size(theme::SMALL).wrapping(Wrapping::None),
            ]
            .spacing(2);
            let row = button(container(body).width(Fill).clip(true))
                .on_press(Message::Pick(n))
                .padding([5, 10])
                .height(ROW)
                .width(Fill)
                .style(theme::row(n == self.selected));
            container(row).padding(iced::Padding::new(0.0).bottom(GAP)).into()
        });
        let above = first as f32 * PITCH;
        let below = (total - last) as f32 * PITCH;
        let list = column![Space::new().height(above)]
            .extend(rows)
            .push(Space::new().height(below));

        let panel = container(
            column![
                input,
                text(status).size(theme::SMALL).color(theme::MUTED),
                scrollable(list).id(list_id()).on_scroll(Message::Scrolled).height(Length::Shrink),
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
