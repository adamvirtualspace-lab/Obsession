//! The file tree, drawn as a virtual list: only the rows on screen are built,
//! so a 10,000-note vault costs the same per frame as a 10-note one.

use std::collections::HashSet;

use iced::widget::scrollable::Viewport;
use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Element, Fill, Length};

use obsession_core::vault::{Kind, Vault};

use crate::theme;

#[derive(Debug, Clone)]
pub enum Message {
    Toggle(u32),
    Open(u32),
    /// Right-click on a note: rename it.
    Rename(u32),
    Scrolled(Viewport),
}

#[derive(Debug, Default)]
pub struct Tree {
    expanded: HashSet<Box<str>>,
    /// Indexes into `vault.entries` of the rows that are currently shown.
    visible: Vec<u32>,
    offset: f32,
    height: f32,
}

impl Tree {
    pub fn new(vault: &Vault) -> Self {
        let mut tree = Self {
            height: 1200.0,
            ..Self::default()
        };
        tree.refresh(vault);
        tree
    }

    /// Recomputes which rows are visible after folders open or close.
    pub fn refresh(&mut self, vault: &Vault) {
        self.visible.clear();
        let mut hidden_below: Option<u16> = None;
        for (index, entry) in vault.entries.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if entry.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            self.visible.push(index as u32);
            if entry.kind == Kind::Folder && !self.expanded.contains(&entry.rel) {
                hidden_below = Some(entry.depth);
            }
        }
    }

    pub fn toggle(&mut self, vault: &Vault, index: u32) {
        let Some(entry) = vault.entries.get(index as usize) else {
            return;
        };
        if !self.expanded.remove(&entry.rel) {
            self.expanded.insert(entry.rel.clone());
        }
        self.refresh(vault);
    }

    /// Opens every folder above `rel`, so the note shows in the tree.
    pub fn reveal(&mut self, vault: &Vault, rel: &str) {
        let mut changed = false;
        let mut parent = rel;
        while let Some((up, _)) = parent.rsplit_once('/') {
            changed |= self.expanded.insert(up.into());
            parent = up;
        }
        if changed {
            self.refresh(vault);
        }
    }

    pub fn scrolled(&mut self, viewport: Viewport) {
        self.offset = viewport.absolute_offset().y;
        self.height = viewport.bounds().height;
    }

    pub fn view<'a>(&'a self, vault: &'a Vault, open: Option<&str>) -> Element<'a, Message> {
        let total = self.visible.len();
        let first = ((self.offset / theme::ROW_HEIGHT).floor() as usize).min(total);
        // One extra row above and below so scrolling never shows a gap.
        let first = first.saturating_sub(1);
        let count = (self.height / theme::ROW_HEIGHT).ceil() as usize + 3;
        let last = (first + count).min(total);

        let rows = self.visible[first..last].iter().map(|&index| {
            let entry = &vault.entries[index as usize];
            let indent = 10.0 + f32::from(entry.depth) * 14.0;
            let (icon, message) = match entry.kind {
                Kind::Folder if self.expanded.contains(&entry.rel) => ("▾", Message::Toggle(index)),
                Kind::Folder => ("▸", Message::Toggle(index)),
                Kind::Note => (" ", Message::Open(index)),
            };
            let selected = open == Some(&*entry.rel);
            let label = row![
                Space::new().width(indent),
                text(icon).size(theme::SMALL).color(theme::MUTED).width(14),
                // One line per row: a long name is cut off at the panel
                // edge instead of wrapping into the rows below.
                text(entry.title())
                    .size(theme::TEXT_SIZE)
                    .wrapping(text::Wrapping::None),
            ]
            .align_y(iced::Center);
            let label = container(label).width(Fill).clip(true);

            let row = button(label)
                .on_press(message)
                .padding([0, 4])
                .height(theme::ROW_HEIGHT)
                .width(Fill)
                .style(theme::row(selected));
            match entry.kind {
                // Right-click a note to rename it.
                Kind::Note => iced::widget::mouse_area(row)
                    .on_right_press(Message::Rename(index))
                    .into(),
                Kind::Folder => row.into(),
            }
        });

        let above = first as f32 * theme::ROW_HEIGHT;
        let below = (total - last) as f32 * theme::ROW_HEIGHT;
        let list = column![Space::new().height(above)]
            .extend(rows)
            .push(Space::new().height(below))
            .padding([0, 6]);

        let body: Element<'a, Message> = if total == 0 {
            container(text("No notes in this folder yet.").size(theme::SMALL).color(theme::MUTED))
                .padding(16)
                .into()
        } else {
            scrollable(list)
                .on_scroll(Message::Scrolled)
                .height(Fill)
                .into()
        };

        column![
            container(text(vault.name()).size(theme::SMALL).color(theme::MUTED))
                .padding([10, 16]),
            body,
        ]
        .width(Length::Fixed(260.0))
        .height(Fill)
        .into()
    }
}
