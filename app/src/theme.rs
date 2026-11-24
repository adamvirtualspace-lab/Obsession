//! Colors, fonts and widget styles. One place to tune the look.

use iced::border::Border;
use iced::widget::{button, container, text_editor, text_input};
use iced::{Background, Color, Font, Shadow, Theme, color};

pub const UI_FONT: Font = Font::with_name("Segoe UI");
pub const MONO_FONT: Font = Font::with_name("Cascadia Mono");

pub const BG: Color = color!(0x0d1117);
pub const PANEL: Color = color!(0x0a0e14);
pub const RAISED: Color = color!(0x131a24);
pub const HOVER: Color = color!(0x182131);
pub const SELECTED: Color = color!(0x1c2a3f);
pub const LINE: Color = color!(0x1c2330);
pub const TEXT: Color = color!(0xc9d1d9);
pub const MUTED: Color = color!(0x7d8796);
pub const FAINT: Color = color!(0x4b5563);
pub const ACCENT: Color = color!(0x3b82f6);
pub const SUCCESS: Color = color!(0x3fb950);
pub const WARNING: Color = color!(0xd29922);
pub const DANGER: Color = color!(0xf85149);
pub const HEADING: Color = color!(0xe6edf3);
pub const LINK: Color = color!(0x58a6ff);
/// A link to a note that doesn't exist yet (a click creates it).
pub const LINK_MISSING: Color = color!(0x3d6a9e);
pub const MENTION: Color = color!(0xd2a8ff);
pub const TAG: Color = color!(0x7ee787);
pub const CODE: Color = color!(0xffa657);

pub const ROW_HEIGHT: f32 = 26.0;

/// Compact mode: tighter toolbars, rows and panels (a preference of this
/// computer, see `settings.rs`).
static COMPACT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_compact(on: bool) {
    COMPACT.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn compact() -> bool {
    COMPACT.load(std::sync::atomic::Ordering::Relaxed)
}

/// A file tree row.
pub fn row_height() -> f32 {
    if compact() { 21.0 } else { ROW_HEIGHT }
}

/// Around a toolbar button's label.
pub fn tool_pad() -> iced::Padding {
    if compact() { [2, 8].into() } else { [4, 12].into() }
}

/// Between toolbar buttons.
pub fn tool_gap() -> f32 {
    if compact() { 6.0 } else { 10.0 }
}

/// Around the toolbar.
pub fn bar_pad() -> iced::Padding {
    if compact() { [4, 8].into() } else { [8, 12].into() }
}
pub const TEXT_SIZE: f32 = 14.0;
pub const SMALL: f32 = 12.0;

pub fn theme() -> Theme {
    Theme::custom(
        "Obsession",
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: SUCCESS,
            warning: WARNING,
            danger: DANGER,
        },
    )
}

pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        text_color: Some(TEXT),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn page(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG)),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn status_bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        text_color: Some(MUTED),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba8(0, 0, 0, 0.45))),
        ..container::Style::default()
    }
}

pub fn popup(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        text_color: Some(TEXT),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.5),
            offset: iced::Vector::new(0.0, 12.0),
            blur_radius: 32.0,
        },
        ..container::Style::default()
    }
}

/// A list row: flat, highlighted on hover and when selected.
pub fn row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => Some(Background::Color(SELECTED)),
            (false, button::Status::Hovered | button::Status::Pressed) => {
                Some(Background::Color(HOVER))
            }
            _ => None,
        };
        button::Style {
            background,
            text_color: if selected { Color::WHITE } else { TEXT },
            border: Border {
                radius: 5.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// A small toolbar button. `active` marks the current choice.
pub fn tool(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (active, status) {
            (true, _) => ACCENT,
            (false, button::Status::Hovered | button::Status::Pressed) => HOVER,
            _ => RAISED,
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: if active { Color::WHITE } else { TEXT },
            border: Border {
                color: if active { ACCENT } else { LINE },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn editor(_: &Theme, _: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(BG),
        border: Border::default(),
        placeholder: FAINT,
        value: TEXT,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

pub fn search(_: &Theme, _: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(PANEL),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: MUTED,
        placeholder: FAINT,
        value: TEXT,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

/// A title that reads as text until it's hovered or typed in.
pub fn title_input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let boxed = !matches!(status, text_input::Status::Active);
    text_input::Style {
        background: Background::Color(if boxed { PANEL } else { Color::TRANSPARENT }),
        border: Border {
            color: if boxed { LINE } else { Color::TRANSPARENT },
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: MUTED,
        placeholder: FAINT,
        value: MENTION,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

/// The line being edited in Live Preview: no box, so it sits in the page
/// like the drawn lines around it.
pub fn live_editor(_: &Theme, _: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        placeholder: FAINT,
        value: TEXT,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

/// A rounded box: code fences, properties, callouts.
pub fn boxed(fill: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(fill)),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

/// A table cell.
pub fn cell(header: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: header.then_some(Background::Color(RAISED)),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// The bar beside a quote.
pub fn quote_bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(FAINT)),
        ..container::Style::default()
    }
}

pub const HIGHLIGHT_BG: Color = color!(0x5a4a12);
/// Selected text (the editors use the same colour).
pub const SELECTION: Color = Color {
    a: 0.35,
    ..ACCENT
};
