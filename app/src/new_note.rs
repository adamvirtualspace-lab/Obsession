//! The "new note" box (Ctrl+N): starts with the open note's folder filled
//! in, so typing a name and pressing Enter creates the note next to it.

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

#[derive(Debug)]
pub struct NewNote {
    pub input: String,
    pub error: Option<String>,
}

impl NewNote {
    /// `folder` is the vault-relative folder to start in (`""` for the root).
    pub fn new(folder: &str) -> Self {
        let input = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        Self { input, error: None }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let input = text_input("Note name, or folder/name", &self.input)
            .id(input_id())
            .on_input(Message::Input)
            .on_submit(Message::Submit)
            .padding(10)
            .size(15)
            .style(theme::search);

        let hint = match &self.error {
            Some(error) => text(error.as_str()).size(theme::SMALL).color(theme::WARNING),
            None => text("Enter to create · Esc to cancel · / makes folders")
                .size(theme::SMALL)
                .color(theme::MUTED),
        };

        let panel = container(
            column![
                text("New note").size(theme::SMALL).color(theme::MUTED),
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
