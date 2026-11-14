//! The right-click status menu: every status in the note's format with its
//! symbol, label and character, the current one highlighted.

use iced::widget::{button, column, container, mouse_area, opaque, pin, row, text};
use iced::{Element, Fill, Point, Size};

use obsession_core::status::Format;

use crate::note::Task;
use crate::theme;

const WIDTH: f32 = 240.0;
const ROW: f32 = 30.0;

#[derive(Debug, Clone)]
pub enum Message {
    Pick(char),
    Close,
}

#[derive(Debug, Clone)]
pub struct StatusMenu {
    pub task: Task,
    /// Window coordinates of the click.
    pub at: Point,
}

impl StatusMenu {
    pub fn view<'a>(&'a self, format: &'a Format, window: Size) -> Element<'a, Message> {
        let current = format.index_of(self.task.ch);
        let rows = format.statuses().iter().enumerate().map(|(i, status)| {
            button(
                row![
                    text(&*status.symbol).size(theme::TEXT_SIZE).width(22),
                    text(&*status.label).size(theme::TEXT_SIZE).width(Fill),
                    text(format!("[{}]", status.ch))
                        .size(theme::SMALL)
                        .font(theme::MONO_FONT)
                        .color(theme::MUTED),
                ]
                .spacing(8)
                .align_y(iced::Center),
            )
            .on_press(Message::Pick(status.ch))
            .padding([5, 10])
            .height(ROW)
            .width(Fill)
            .style(theme::row(current == Some(i)))
            .into()
        });

        let mut list = column(rows).spacing(2);
        if current.is_none() {
            list = list.push(
                container(
                    text(format!("⚠ [{}] isn't in this format", self.task.ch))
                        .size(theme::SMALL)
                        .color(theme::WARNING),
                )
                .padding([4, 10]),
            );
        }
        let panel = container(list).padding(6).width(WIDTH).style(theme::popup);

        // Keep the menu inside the window: flip up or left near the edges.
        let height = format.statuses().len() as f32 * (ROW + 2.0) + 12.0
            + if current.is_none() { 26.0 } else { 0.0 };
        let x = if self.at.x + WIDTH > window.width {
            (self.at.x - WIDTH).max(0.0)
        } else {
            self.at.x
        };
        let y = if self.at.y + height > window.height {
            (self.at.y - height).max(0.0)
        } else {
            self.at.y
        };

        // A click anywhere else closes the menu.
        opaque(
            mouse_area(pin(opaque(panel)).x(x).y(y).width(Fill).height(Fill))
                .on_press(Message::Close)
                .on_right_press(Message::Close),
        )
    }
}
