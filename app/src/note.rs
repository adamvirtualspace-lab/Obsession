//! The open note: raw editor, preview, and the timers behind autosave and
//! the debounced preview.

use std::time::{Duration, Instant};

use iced::widget::{markdown, scrollable, text_editor};
use iced::{Element, Fill, Length};

use obsession_core::preview;

use crate::theme;

/// The preview re-renders this long after typing stops.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// Autosave runs this long after typing stops.
pub const SAVE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Edit,
    Preview,
    #[default]
    Split,
}

impl Mode {
    pub fn shows_preview(self) -> bool {
        matches!(self, Mode::Preview | Mode::Split)
    }

    pub fn next(self) -> Self {
        match self {
            Mode::Edit => Mode::Split,
            Mode::Split => Mode::Preview,
            Mode::Preview => Mode::Edit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    Preview,
    Save,
}

pub struct Note {
    /// Unique per opened note, so a timer from a previous note is ignored.
    pub id: u64,
    pub rel: Box<str>,
    pub editor: text_editor::Content,
    preview: markdown::Content,
    preview_stale: bool,
    /// Edits not yet handed to the store.
    pub unsynced: bool,
    last_edit: Instant,
    preview_timer: bool,
    save_timer: bool,
}

impl Note {
    pub fn new(id: u64, rel: Box<str>, text: &str, mode: Mode) -> Self {
        let mut note = Self {
            id,
            rel,
            editor: text_editor::Content::with_text(text),
            preview: markdown::Content::new(),
            preview_stale: true,
            unsynced: false,
            last_edit: Instant::now(),
            preview_timer: false,
            save_timer: false,
        };
        if mode.shows_preview() {
            note.render_preview(text);
        }
        note
    }

    pub fn title(&self) -> &str {
        let name = self.rel.rsplit('/').next().unwrap_or(&self.rel);
        name.strip_suffix(".md").unwrap_or(name)
    }

    /// Applies an editor action. Returns the timers that need starting.
    pub fn perform(&mut self, action: text_editor::Action) -> Vec<(Timer, Duration)> {
        let is_edit = action.is_edit();
        self.editor.perform(action);
        if !is_edit {
            return Vec::new();
        }
        self.unsynced = true;
        self.preview_stale = true;
        self.last_edit = Instant::now();

        let mut timers = Vec::new();
        if !self.preview_timer {
            self.preview_timer = true;
            timers.push((Timer::Preview, PREVIEW_DELAY));
        }
        if !self.save_timer {
            self.save_timer = true;
            timers.push((Timer::Save, SAVE_DELAY));
        }
        timers
    }

    /// A timer fired. Returns `Ok(())` if its work is due now, or the time
    /// left if typing continued in the meantime (the timer is re-armed).
    pub fn timer_fired(&mut self, timer: Timer) -> Result<(), Duration> {
        let delay = match timer {
            Timer::Preview => PREVIEW_DELAY,
            Timer::Save => SAVE_DELAY,
        };
        let elapsed = self.last_edit.elapsed();
        if elapsed < delay {
            return Err(delay - elapsed);
        }
        match timer {
            Timer::Preview => self.preview_timer = false,
            Timer::Save => self.save_timer = false,
        }
        Ok(())
    }

    pub fn text(&self) -> String {
        self.editor.text()
    }

    pub fn refresh_preview_if_stale(&mut self) {
        if self.preview_stale {
            let text = self.editor.text();
            self.render_preview(&text);
        }
    }

    fn render_preview(&mut self, text: &str) {
        self.preview = markdown::Content::parse(&preview::to_markdown(text));
        self.preview_stale = false;
    }

    pub fn view_editor(&self) -> Element<'_, text_editor::Action> {
        text_editor(&self.editor)
            .on_action(|action| action)
            .highlight_with::<crate::highlight::Markdown>((), crate::highlight::format)
            .font(theme::MONO_FONT)
            .size(theme::TEXT_SIZE)
            .padding([24, 32])
            .height(Fill)
            .style(theme::editor)
            .into()
    }

    pub fn view_preview(&self) -> Element<'_, markdown::Uri> {
        let style = markdown::Style::from_palette(theme::theme().palette());
        let settings = markdown::Settings::with_text_size(15, style);
        scrollable(
            iced::widget::container(markdown::view(self.preview.items(), settings))
                .padding([24, 32])
                .max_width(860)
                .width(Length::Fill),
        )
        .height(Fill)
        .width(Fill)
        .into()
    }
}
