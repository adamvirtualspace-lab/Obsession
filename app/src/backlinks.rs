//! The backlinks panel: every note that links to the open one, with the
//! line that links. A click opens that note at that line.

use iced::widget::{Space, button, column, container, scrollable, text};
use iced::{Element, Fill, Length};

use obsession_core::links::Backlink;

use crate::theme;

pub const WIDTH: f32 = 300.0;

#[derive(Debug, Clone)]
pub enum Message {
    /// Open `rel` at a 0-based line.
    Open(Box<str>, usize),
}

/// `indexed` is false while the vault's links are still being read.
pub fn view<'a>(backlinks: &'a [Backlink], indexed: bool) -> Element<'a, Message> {
    let count = backlinks.len();
    let header = text(if indexed {
        format!("Linked mentions · {count}")
    } else {
        "Linked mentions".to_owned()
    })
    .size(theme::SMALL)
    .color(theme::MUTED);

    let body: Element<'a, Message> = if !indexed {
        text("Reading the vault's links…").size(theme::SMALL).color(theme::FAINT).into()
    } else if backlinks.is_empty() {
        text("No notes link here yet.").size(theme::SMALL).color(theme::FAINT).into()
    } else {
        let mut items = column![].spacing(2);
        let mut last: Option<&str> = None;
        for link in backlinks {
            if last != Some(&*link.source) {
                last = Some(&link.source);
                let (folder, name) = match link.source.rsplit_once('/') {
                    Some((folder, name)) => (folder, name),
                    None => ("", &*link.source),
                };
                let name = name.strip_suffix(".md").unwrap_or(name);
                let title = column![text(name).size(theme::TEXT_SIZE).color(theme::HEADING)]
                    .push((!folder.is_empty()).then(|| text(folder).size(theme::SMALL).color(theme::FAINT)));
                items = items.push(Space::new().height(6)).push(
                    button(title)
                        .on_press(Message::Open(link.source.clone(), link.line))
                        .padding([4, 8])
                        .width(Fill)
                        .style(theme::row(false)),
                );
            }
            items = items.push(
                button(text(&*link.snippet).size(theme::SMALL).color(theme::MUTED))
                    .on_press(Message::Open(link.source.clone(), link.line))
                    .padding([4, 8])
                    .width(Fill)
                    .style(theme::row(false)),
            );
        }
        scrollable(items).height(Fill).into()
    };

    container(column![header, body].spacing(8))
        .padding([12, 10])
        .width(Length::Fixed(WIDTH))
        .height(Fill)
        .style(theme::panel)
        .into()
}
