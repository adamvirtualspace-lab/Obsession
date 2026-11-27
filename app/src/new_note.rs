//! The "new note" box (Ctrl+N): starts with the open note's folder filled
//! in, so typing a name and pressing Enter creates the note next to it.
//! The same box renames a note (F2, or right-click in the file tree) or a
//! folder (right-click).

use iced::widget::{Id, column, container, opaque, text, text_input};
use iced::{Element, Fill, Length};

use crate::theme;

pub fn input_id() -> Id {
    Id::new("new-note")
}

#[derive(Debug, Clone)]
pub enum Message {
    Input(String),
    Submit,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    New,
    /// Rename the note at this path.
    Rename(Box<str>),
    /// Rename the folder at this path.
    RenameFolder(Box<str>),
}

#[derive(Debug)]
pub struct NewNote {
    pub input: String,
    pub error: Option<String>,
    pub purpose: Purpose,
}

impl NewNote {
    /// `folder` is the vault-relative folder to start in (`""` for the root).
    pub fn new(folder: &str) -> Self {
        let input = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        Self {
            input,
            error: None,
            purpose: Purpose::New,
        }
    }

    /// Starts with the note's current path (without `.md`) to edit.
    pub fn rename(rel: &str) -> Self {
        Self {
            input: rel.strip_suffix(".md").unwrap_or(rel).to_owned(),
            error: None,
            purpose: Purpose::Rename(rel.into()),
        }
    }

    /// Starts with the folder's current path to edit.
    pub fn rename_folder(rel: &str) -> Self {
        Self {
            input: rel.to_owned(),
            error: None,
            purpose: Purpose::RenameFolder(rel.into()),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let placeholder = match self.purpose {
            Purpose::RenameFolder(_) => "Folder name, or parent/name",
            _ => "Note name, or folder/name",
        };
        let input = text_input(placeholder, &self.input)
            .id(input_id())
            .on_input(Message::Input)
            .on_submit(Message::Submit)
            .padding(10)
            .size(15)
            .style(theme::search);

        let hint = match &self.error {
            Some(error) => text(error.as_str()).size(theme::SMALL).color(theme::WARNING),
            None => text(match self.purpose {
                Purpose::Rename(_) => "Enter to rename (links to it are updated) · Esc to cancel · / moves it",
                Purpose::RenameFolder(_) => {
                    "Enter to rename (links into it are updated) · Esc to cancel · / moves it"
                }
                Purpose::New => "Enter to create · Esc to cancel · / makes folders",
            })
            .size(theme::SMALL)
            .color(theme::MUTED),
        };
        let title = match &self.purpose {
            Purpose::New => "New note".to_owned(),
            Purpose::Rename(rel) => format!("Rename {rel}"),
            Purpose::RenameFolder(rel) => format!("Rename the folder {rel}"),
        };

        let panel = container(
            column![
                text(title).size(theme::SMALL).color(theme::MUTED),
                input,
                hint
            ]
            .spacing(8),
        )
        .padding(12)
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
