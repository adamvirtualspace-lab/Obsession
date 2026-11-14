//! The open note: raw editor, preview, status format, and the timers behind
//! autosave and the debounced preview.

use std::time::{Duration, Instant};

use iced::widget::text_editor::{self, Action, Cursor, Edit, Position};
use iced::widget::{markdown, scrollable};
use iced::{Element, Fill, Length, Point};

use obsession_core::status::{self, Format, Next, Source};
use obsession_core::{preview, syntax};

use crate::highlight::Statuses;
use crate::theme;

/// The preview re-renders this long after typing stops.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// Autosave runs this long after typing stops.
pub const SAVE_DELAY: Duration = Duration::from_secs(1);
/// Editor padding: top/bottom, left/right.
pub const EDITOR_PADDING: (f32, f32) = (24.0, 32.0);

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

/// What an editor action asks of the app.
#[derive(Default)]
pub struct Performed {
    pub timers: Vec<(Timer, Duration)>,
    /// Something to say in the status bar.
    pub message: Option<String>,
}

/// A task found in the open note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Task {
    pub line: usize,
    /// Byte offset of `[`.
    pub at: usize,
    pub ch: char,
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

    pub format: Format,
    pub source: Source,
    /// Problems in this note's own `statuses:`.
    pub warnings: Vec<String>,
    statuses: Statuses,
    /// Index of the line that closes the frontmatter (0 if there is none).
    /// Edits at or above it re-read `statuses:`.
    frontmatter_end: usize,
    /// Tasks whose character isn't in the format, as of the last full read
    /// (open, preview, save).
    pub unknown: usize,
}

impl Note {
    pub fn new(id: u64, rel: Box<str>, text: &str, mode: Mode, team: Option<&Format>) -> Self {
        let format = Format::builtin();
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
            statuses: Statuses::new(&format),
            format,
            source: Source::Default,
            warnings: Vec::new(),
            frontmatter_end: 0,
            unknown: 0,
        };
        note.refresh_format(team);
        note.analyze(text);
        if mode.shows_preview() {
            note.render_preview(text);
        }
        note
    }

    pub fn title(&self) -> &str {
        let name = self.rel.rsplit('/').next().unwrap_or(&self.rel);
        name.strip_suffix(".md").unwrap_or(name)
    }

    /// Re-reads `statuses:` from the frontmatter (only the frontmatter lines
    /// are touched). Returns whether the format changed.
    pub fn refresh_format(&mut self, team: Option<&Format>) -> bool {
        let lookup = status::from_frontmatter(self.editor.lines().map(|line| line.text));
        let (format, source) = status::resolve(lookup.as_ref(), team);
        self.warnings = lookup.map(|l| l.warnings).unwrap_or_default();
        self.source = source;
        self.frontmatter_end = self.find_frontmatter_end();
        if format == self.format {
            return false;
        }
        self.statuses = Statuses::new(&format);
        self.format = format;
        self.preview_stale = true;
        true
    }

    fn find_frontmatter_end(&self) -> usize {
        let is_fence = |i: usize, closing: bool| {
            self.editor.line(i).is_some_and(|line| {
                let t = line.text.trim_start_matches('\u{feff}').trim_end();
                t == "---" || (closing && t == "...")
            })
        };
        if !is_fence(0, false) {
            return 0;
        }
        (1..self.editor.line_count())
            .find(|&i| is_fence(i, true))
            .unwrap_or(self.editor.line_count())
    }

    /// Counts tasks whose character isn't in the format.
    pub fn analyze(&mut self, text: &str) {
        self.unknown = status::tasks(text)
            .into_iter()
            .filter(|&(_, _, ch)| self.format.index_of(ch).is_none())
            .count();
    }

    /// Applies an editor action. A plain click on a task box moves the task
    /// to its next status.
    pub fn perform(&mut self, action: Action, team: Option<&Format>) -> Performed {
        let is_edit = action.is_edit();
        let is_click = matches!(action, Action::Click(_));
        self.editor.perform(action);

        if is_click {
            return self.click_status();
        }
        if !is_edit {
            return Performed::default();
        }
        // Typing in (or just below) the frontmatter may change `statuses:`.
        if self.editor.cursor().position.line <= self.frontmatter_end + 1 {
            self.refresh_format(team);
        }
        Performed {
            timers: self.edited(),
            message: None,
        }
    }

    fn click_status(&mut self) -> Performed {
        let cursor = self.editor.cursor();
        if cursor.selection.is_some() {
            return Performed::default();
        }
        let Some(task) = self.task_at_line(cursor.position.line) else {
            return Performed::default();
        };
        let column = cursor.position.column;
        let box_end = task.at + 2 + task.ch.len_utf8();
        if !(task.at..=box_end).contains(&column) {
            return Performed::default();
        }
        self.advance(task)
    }

    /// Moves `task` to the next status in the format.
    pub fn advance(&mut self, task: Task) -> Performed {
        match self.format.next(task.ch) {
            Next::To(next) => Performed {
                timers: self.set_status(task, next),
                message: None,
            },
            Next::Finished => Performed {
                timers: Vec::new(),
                message: Some("Already at the last status. Right-click to pick another.".into()),
            },
            Next::Unknown => Performed {
                timers: Vec::new(),
                message: Some(format!(
                    "⚠ [{}] isn't in this note's statuses. Right-click to pick one.",
                    task.ch
                )),
            },
        }
    }

    /// The task on `line`, if that line is a task outside code, frontmatter
    /// and comments.
    pub fn task_at_line(&self, line: usize) -> Option<Task> {
        let text = self.editor.line(line)?.text;
        let (at, ch) = status::task_box(&text)?;
        let state = syntax::state_at(self.editor.lines().map(|l| l.text), line);
        syntax::can_hold_task(state).then_some(Task { line, at, ch })
    }

    /// Moves the caret to `point` (editor coordinates, as a right-click
    /// would) and returns the task on that line, if any.
    pub fn task_at_point(&mut self, point: Point) -> Option<Task> {
        let point = Point::new(point.x - EDITOR_PADDING.1, point.y - EDITOR_PADDING.0);
        self.editor.perform(Action::Click(point));
        self.task_at_line(self.editor.cursor().position.line)
    }

    /// Replaces the one character between the brackets. The caret and any
    /// selection are put back where they were.
    pub fn set_status(&mut self, task: Task, ch: char) -> Vec<(Timer, Duration)> {
        if task.ch == ch {
            return Vec::new();
        }
        let saved = self.editor.cursor();
        let start = task.at + 1;
        self.editor.move_to(Cursor {
            position: Position {
                line: task.line,
                column: start + task.ch.len_utf8(),
            },
            selection: Some(Position {
                line: task.line,
                column: start,
            }),
        });
        self.editor.perform(Action::Edit(Edit::Insert(ch)));

        let shift = |p: Position| {
            if p.line == task.line && p.column > start {
                let column = p.column + ch.len_utf8();
                Position {
                    column: column.saturating_sub(task.ch.len_utf8()),
                    ..p
                }
            } else {
                p
            }
        };
        self.editor.move_to(Cursor {
            position: shift(saved.position),
            selection: saved.selection.map(shift),
        });
        self.edited()
    }

    /// Bookkeeping after any change to the text.
    fn edited(&mut self) -> Vec<(Timer, Duration)> {
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
            self.analyze(&text);
            self.render_preview(&text);
        }
    }

    fn render_preview(&mut self, text: &str) {
        self.preview = markdown::Content::parse(&preview::to_markdown(text, &self.format));
        self.preview_stale = false;
    }

    pub fn view_editor(&self) -> Element<'_, Action> {
        iced::widget::text_editor(&self.editor)
            .on_action(|action| action)
            .highlight_with::<crate::highlight::Markdown>(
                self.statuses.clone(),
                crate::highlight::format,
            )
            .font(theme::MONO_FONT)
            .size(theme::TEXT_SIZE)
            .padding([EDITOR_PADDING.0, EDITOR_PADDING.1])
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
