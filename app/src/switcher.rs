//! The quick switcher (Ctrl+P): type part of a name, Enter to open.

use iced::widget::{Id, button, column, container, opaque, row, text, text_input};
use iced::{Element, Fill, Length};

use obsession_core::fuzzy;
use obsession_core::vault::{Kind, Vault};

use crate::theme;

const MAX_RESULTS: usize = 12;

pub fn input_id() -> Id {
    Id::new("switcher")
}

#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    Submit,
    Pick(u32),
    Close,
}

#[derive(Debug, Default)]
pub struct Switcher {
    query: String,
    /// Indexes into `vault.entries`, best match first.
    results: Vec<u32>,
    selected: usize,
}

impl Switcher {
    pub fn new(vault: &Vault) -> Self {
        let mut switcher = Self::default();
        switcher.search(vault);
        switcher
    }

    pub fn set_query(&mut self, vault: &Vault, query: String) {
        self.query = query;
        self.selected = 0;
        self.search(vault);
    }

    pub fn move_selection(&mut self, down: bool) {
        if self.results.is_empty() {
            return;
        }
        let len = self.results.len();
        self.selected = if down {
            (self.selected + 1) % len
        } else {
            (self.selected + len - 1) % len
        };
    }

    pub fn selected(&self) -> Option<u32> {
        self.results.get(self.selected).copied()
    }

    fn search(&mut self, vault: &Vault) {
        let mut scored: Vec<(i32, u32)> = vault
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == Kind::Note)
            .filter_map(|(i, e)| fuzzy::score(&self.query, &e.rel).map(|s| (s, i as u32)))
            .collect();
        // Only the top few are shown, so a partial sort is enough.
        let take = MAX_RESULTS.min(scored.len());
        if take > 0 {
            scored.select_nth_unstable_by(take - 1, |a, b| b.0.cmp(&a.0));
            scored.truncate(take);
            scored.sort_unstable_by_key(|s| std::cmp::Reverse(s.0));
        }
        self.results = scored.into_iter().map(|(_, i)| i).collect();
    }

    pub fn view<'a>(&'a self, vault: &'a Vault) -> Element<'a, Message> {
        let input = text_input("Find a note…", &self.query)
            .id(input_id())
            .on_input(Message::Query)
            .on_submit(Message::Submit)
            .padding(10)
            .size(15)
            .style(theme::search);

        let rows = self.results.iter().enumerate().map(|(n, &index)| {
            let entry = &vault.entries[index as usize];
            let folder = entry.parent();
            button(
                row![
                    text(entry.title()).size(theme::TEXT_SIZE),
                    text(folder).size(theme::SMALL).color(theme::MUTED),
                ]
                .spacing(10)
                .align_y(iced::Center),
            )
            .on_press(Message::Pick(index))
            .padding([6, 10])
            .width(Fill)
            .style(theme::row(n == self.selected))
            .into()
        });

        let empty = self.results.is_empty().then(|| {
            text("No matching notes")
                .size(theme::SMALL)
                .color(theme::MUTED)
        });

        let panel = container(
            column![input]
                .push(column(rows).spacing(2))
                .push(empty)
                .spacing(8),
        )
        .padding(10)
        .width(Length::Fixed(560.0))
        .style(theme::popup);

        let backdrop = iced::widget::mouse_area(
            container(opaque(panel))
                .padding(iced::Padding::new(0.0).top(90.0))
                .center_x(Fill)
                .height(Fill)
                .style(theme::backdrop),
        )
        .on_press(Message::Close);

        opaque(backdrop)
    }
}
