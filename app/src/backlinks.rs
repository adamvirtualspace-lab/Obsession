//! The backlinks panel: every note that links to the open one, with the
//! line that links (drawn formatted, as Live Preview would), and its
//! unlinked mentions: lines that write the note's name without a link.
//! A click opens that note at that line.
//!
//! Unlinked mentions need every note's text, which only vault search
//! keeps; so they're looked for once asked (`Message::FindUnlinked`
//! builds search if it isn't yet), and then for each note opened.

use iced::font::{Style as FontStyle, Weight};
use iced::widget::text::Span;
use iced::widget::{Space, button, column, container, rich_text, scrollable, span, text};
use iced::{Background, Border, Element, Fill, Font, Length};

use obsession_core::links::Backlink;
use obsession_core::live::{self, Style};
use obsession_core::search::{Hit, Results};
use obsession_core::status::Format;

use crate::theme;

pub const WIDTH: f32 = 300.0;

/// The most unlinked mentions listed.
pub const UNLINKED_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub enum Message {
    /// Open `rel` at a 0-based line.
    Open(Box<str>, usize),
    /// Look for unlinked mentions (and from now on, for every note opened).
    FindUnlinked,
}

#[derive(Debug, Default)]
pub struct Panel {
    pub links: Vec<Backlink>,
    /// Each link's line, drawn.
    lines: Vec<live::Line>,
    /// Unlinked mentions are wanted (asked for once this session).
    pub want_unlinked: bool,
    /// The unlinked mentions found, and the note they're for.
    pub unlinked: Option<(Box<str>, Results)>,
}

impl Panel {
    pub fn set_links(&mut self, links: Vec<Backlink>, format: &Format) {
        if links == self.links {
            return;
        }
        self.lines = links.iter().map(|l| live::line(&l.snippet, format)).collect();
        self.links = links;
    }

    pub fn clear(&mut self) {
        self.links.clear();
        self.lines.clear();
    }

    /// The unlinked mentions are for another note than `rel` (or none yet).
    pub fn unlinked_stale(&self, rel: &str) -> bool {
        self.want_unlinked && self.unlinked.as_ref().is_none_or(|(r, _)| **r != *rel)
    }

    /// `indexed` is false while the vault's links are still being read;
    /// `searching` while search is being built for unlinked mentions.
    pub fn view(&self, indexed: bool, searching: bool) -> Element<'_, Message> {
        let count = self.links.len();
        let header = text(if indexed {
            format!("Linked mentions · {count}")
        } else {
            "Linked mentions".to_owned()
        })
        .size(theme::SMALL)
        .color(theme::MUTED);

        let mut items = column![header].spacing(2);
        if !indexed {
            items = items.push(text("Reading the vault's links…").size(theme::SMALL).color(theme::FAINT));
        } else if self.links.is_empty() {
            items = items.push(text("No notes link here yet.").size(theme::SMALL).color(theme::FAINT));
        } else {
            let mut last: Option<&str> = None;
            for (link, line) in self.links.iter().zip(&self.lines) {
                if last != Some(&*link.source) {
                    last = Some(&link.source);
                    items = items.push(Space::new().height(6)).push(source_title(&link.source, link.line));
                }
                items = items.push(
                    button(rich_text(snippet_spans(line)).size(theme::SMALL))
                        .on_press(Message::Open(link.source.clone(), link.line))
                        .padding([4, 8])
                        .width(Fill)
                        .style(theme::row(false)),
                );
            }
        }

        items = items.push(Space::new().height(16));
        let unlinked = self.unlinked.as_ref().map(|(_, r)| r);
        items = items.push(
            text(match unlinked {
                Some(r) => format!("Unlinked mentions · {}", r.total),
                None => "Unlinked mentions".to_owned(),
            })
            .size(theme::SMALL)
            .color(theme::MUTED),
        );
        match unlinked {
            _ if searching => {
                items = items.push(text("Reading your notes…").size(theme::SMALL).color(theme::FAINT));
            }
            None => {
                items = items.push(
                    button(text("Find unlinked mentions").size(theme::SMALL))
                        .on_press(Message::FindUnlinked)
                        .padding([4, 8])
                        .style(theme::row(false)),
                );
            }
            Some(r) if r.total == 0 => {
                items = items.push(text("No other note names this one without a link.").size(theme::SMALL).color(theme::FAINT));
            }
            Some(r) => {
                let mut last: Option<&str> = None;
                for hit in &r.hits {
                    if last != Some(&*hit.rel) {
                        last = Some(&hit.rel);
                        items = items.push(Space::new().height(6)).push(source_title(&hit.rel, hit.line));
                    }
                    items = items.push(
                        button(rich_text(hit_spans(hit)).size(theme::SMALL))
                            .on_press(Message::Open(hit.rel.clone(), hit.line))
                            .padding([4, 8])
                            .width(Fill)
                            .style(theme::row(false)),
                    );
                }
                if r.hits.len() < r.total {
                    items = items.push(
                        text(format!("First {} shown", r.hits.len())).size(theme::SMALL).color(theme::FAINT),
                    );
                }
            }
        }

        container(scrollable(items).height(Fill))
            .padding([12, 10])
            .width(Length::Fixed(WIDTH))
            .height(Fill)
            .style(theme::panel)
            .into()
    }
}

/// A note's name (and folder) as a heading over its lines.
fn source_title(rel: &str, line: usize) -> Element<'_, Message> {
    let (folder, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let name = name.strip_suffix(".md").unwrap_or(name);
    let title = column![text(name).size(theme::TEXT_SIZE).color(theme::HEADING)]
        .push((!folder.is_empty()).then(|| text(folder).size(theme::SMALL).color(theme::FAINT)));
    button(title)
        .on_press(Message::Open(rel.into(), line))
        .padding([4, 8])
        .width(Fill)
        .style(theme::row(false))
        .into()
}

/// A linking line as Live Preview draws it: bold, italics, code, links,
/// tags, `@names` and status symbols, without the markdown.
fn snippet_spans(line: &live::Line) -> Vec<Span<'_, (), Font>> {
    let base = if line.heading > 0 {
        Font {
            weight: Weight::Semibold,
            ..theme::UI_FONT
        }
    } else {
        theme::UI_FONT
    };
    line.segs
        .iter()
        .filter(|seg| !seg.text.is_empty())
        .map(|seg| {
            let s = seg.style;
            let mut font = base;
            if s.has(Style::BOLD) {
                font.weight = Weight::Bold;
            }
            if s.has(Style::ITALIC) {
                font.style = FontStyle::Italic;
            }
            let mut color = if line.heading > 0 { theme::HEADING } else { theme::MUTED };
            let mut piece = span(seg.text.as_str());
            if s.has(Style::CODE) {
                font = theme::MONO_FONT;
                color = theme::CODE;
                piece = piece.background(Background::Color(theme::RAISED)).border(Border {
                    radius: 3.0.into(),
                    ..Border::default()
                });
            }
            if s.has(Style::LINK) {
                color = theme::LINK;
            }
            if s.has(Style::TAG) {
                color = theme::TAG;
            }
            if s.has(Style::MENTION) {
                color = theme::MENTION;
            }
            if s.has(Style::MARKER) || s.has(Style::FAINT) {
                color = theme::FAINT;
            }
            if s.has(Style::DONE) && !s.has(Style::CHIP) {
                color = theme::FAINT;
                piece = piece.strikethrough(true);
            }
            if s.has(Style::STRIKE) {
                piece = piece.strikethrough(true);
            }
            if s.has(Style::HIGHLIGHT) {
                piece = piece.background(Background::Color(theme::HIGHLIGHT_BG));
            }
            piece.font(font).color(color)
        })
        .collect()
}

/// An unlinked mention's line, its name highlighted.
fn hit_spans(hit: &Hit) -> Vec<Span<'_, (), Font>> {
    let strong = Font {
        weight: Weight::Semibold,
        ..theme::UI_FONT
    };
    let mut spans = Vec::new();
    let mut at = 0;
    for range in &hit.ranges {
        if range.start > at {
            spans.push(span(&hit.text[at..range.start]).color(theme::MUTED));
        }
        spans.push(span(&hit.text[range.clone()]).color(theme::HEADING).font(strong).background(theme::HIGHLIGHT_BG));
        at = range.end;
    }
    if at < hit.text.len() {
        spans.push(span(&hit.text[at..]).color(theme::MUTED));
    }
    spans
}
