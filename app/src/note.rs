//! The open note: Live Preview, Source (raw editor) or Reading (preview),
//! its status format, undo history, and the timers behind autosave and the
//! debounced preview.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use iced::widget::text_editor::{self, Action, Cursor, Edit, Position};
use iced::widget::{markdown, scrollable};
use iced::{Element, Fill, Length, Point};

use obsession_core::history::{EditKind, History, Snapshot};
use obsession_core::status::{self, Format, Next, Source};
use obsession_core::{preview, syntax};

use crate::highlight::Statuses;
use crate::live::{self, Live};
use crate::theme;

/// The preview re-renders this long after typing stops.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// Autosave runs this long after typing stops.
pub const SAVE_DELAY: Duration = Duration::from_secs(1);
/// Source editor padding: top/bottom, left/right.
pub const EDITOR_PADDING: (f32, f32) = (24.0, 32.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Formatted, with the line being edited shown raw.
    #[default]
    Live,
    /// The raw markdown.
    Source,
    /// The preview, read-only (status chips still click).
    Reading,
}

impl Mode {
    pub fn shows_preview(self) -> bool {
        self == Mode::Reading
    }

    pub fn next(self) -> Self {
        match self {
            Mode::Live => Mode::Source,
            Mode::Source => Mode::Reading,
            Mode::Reading => Mode::Live,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    Preview,
    Save,
}

/// What an edit asks of the app.
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

/// What a Live Preview message led to, beyond the edit itself.
#[derive(Default)]
pub struct LiveOutcome {
    pub performed: Performed,
    /// A `[[link]]` or URL to open.
    pub open: Option<String>,
    /// A task to show the status menu for, at a window point.
    pub menu: Option<(Task, Point)>,
    /// The editor should get the keyboard focus.
    pub focus: bool,
    /// Scroll the page by this much.
    pub scroll: Option<f32>,
}

enum Body {
    Live(Live),
    Source(text_editor::Content),
}

pub struct Note {
    /// Unique per opened note, so a timer from a previous note is ignored.
    pub id: u64,
    pub rel: Box<str>,
    body: Body,
    preview: markdown::Content,
    preview_stale: bool,
    /// Edits not yet handed to the store.
    pub unsynced: bool,
    last_edit: Instant,
    preview_timer: bool,
    save_timer: bool,
    history: History,

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
        let body = match mode {
            Mode::Source => Body::Source(text_editor::Content::with_text(text)),
            Mode::Live | Mode::Reading => Body::Live(Live::new(text, &format)),
        };
        let mut note = Self {
            id,
            rel,
            body,
            preview: markdown::Content::new(),
            preview_stale: true,
            unsynced: false,
            last_edit: Instant::now(),
            preview_timer: false,
            save_timer: false,
            history: History::default(),
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
        // An empty note starts ready to type.
        if let Body::Live(live) = &mut note.body
            && text.trim().is_empty()
        {
            live.activate(0, 0);
        }
        note
    }

    pub fn title(&self) -> &str {
        let name = self.rel.rsplit('/').next().unwrap_or(&self.rel);
        name.strip_suffix(".md").unwrap_or(name)
    }

    /// Switches between Live and Source bodies when the mode needs it.
    pub fn set_mode(&mut self, mode: Mode) {
        match (&self.body, mode) {
            (Body::Source(content), Mode::Live | Mode::Reading) => {
                let cursor = content.cursor().position;
                let mut live = Live::new(&content.text(), &self.format);
                if mode == Mode::Live {
                    live.activate(cursor.line, cursor.column);
                }
                self.body = Body::Live(live);
            }
            (Body::Live(live), Mode::Source) => {
                let cursor = live.active.as_ref().map(|_| live.cursor());
                let mut content = text_editor::Content::with_text(&live.text());
                if let Some((line, column)) = cursor {
                    content.move_to(Cursor {
                        position: Position { line, column },
                        selection: None,
                    });
                }
                self.body = Body::Source(content);
            }
            _ => {}
        }
        if mode.shows_preview() {
            self.refresh_preview_if_stale();
        }
    }

    pub fn is_live_editing(&self) -> bool {
        matches!(&self.body, Body::Live(live) if live.active.is_some())
    }

    // ---- Lines, whichever body holds them ----

    fn line_count(&self) -> usize {
        match &self.body {
            Body::Live(live) => live.doc.len(),
            Body::Source(content) => content.line_count(),
        }
    }

    fn line(&self, index: usize) -> Option<Cow<'_, str>> {
        match &self.body {
            Body::Live(live) => (index < live.doc.len()).then(|| Cow::Borrowed(live.doc.line(index))),
            Body::Source(content) => content.line(index).map(|l| l.text),
        }
    }

    fn lines(&self) -> impl Iterator<Item = Cow<'_, str>> {
        (0..self.line_count()).map_while(|i| self.line(i))
    }

    pub fn text(&self) -> String {
        match &self.body {
            Body::Live(live) => live.text(),
            Body::Source(content) => content.text(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        let cursor = match &self.body {
            Body::Live(live) => live.cursor(),
            Body::Source(content) => {
                let p = content.cursor().position;
                (p.line, p.column)
            }
        };
        Snapshot {
            text: self.text(),
            cursor,
        }
    }

    // ---- Status format ----

    /// Re-reads `statuses:` from the frontmatter (only the frontmatter lines
    /// are touched). Returns whether the format changed.
    pub fn refresh_format(&mut self, team: Option<&Format>) -> bool {
        let lookup = status::from_frontmatter(self.lines());
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
        if let Body::Live(live) = &mut self.body {
            live.rebuild(&self.format);
        }
        true
    }

    fn find_frontmatter_end(&self) -> usize {
        let is_fence = |i: usize, closing: bool| {
            self.line(i).is_some_and(|line| {
                let t = line.trim_start_matches('\u{feff}').trim_end();
                t == "---" || (closing && t == "...")
            })
        };
        if !is_fence(0, false) {
            return 0;
        }
        (1..self.line_count())
            .find(|&i| is_fence(i, true))
            .unwrap_or(self.line_count())
    }

    /// Counts tasks whose character isn't in the format.
    pub fn analyze(&mut self, text: &str) {
        self.unknown = status::tasks(text)
            .into_iter()
            .filter(|&(_, _, ch)| self.format.index_of(ch).is_none())
            .count();
    }

    // ---- Source editor ----

    /// Applies a Source-editor action. A plain click on a task box moves
    /// the task to its next status.
    pub fn perform(&mut self, action: Action, team: Option<&Format>) -> Performed {
        let is_edit = action.is_edit();
        let is_click = matches!(action, Action::Click(_));
        if is_edit {
            self.record(edit_kind(&action));
        }
        let Body::Source(content) = &mut self.body else {
            return Performed::default();
        };
        content.perform(action);

        if is_click {
            return self.click_status();
        }
        if !is_edit {
            return Performed::default();
        }
        // Typing in (or just below) the frontmatter may change `statuses:`.
        if self.cursor_line() <= self.frontmatter_end + 1 {
            self.refresh_format(team);
        }
        Performed {
            timers: self.edited(),
            message: None,
        }
    }

    fn cursor_line(&self) -> usize {
        match &self.body {
            Body::Live(live) => live.cursor().0,
            Body::Source(content) => content.cursor().position.line,
        }
    }

    fn click_status(&mut self) -> Performed {
        let Body::Source(content) = &self.body else {
            return Performed::default();
        };
        let cursor = content.cursor();
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

    /// Moves the caret to `point` (Source editor coordinates, as a
    /// right-click would) and returns the task on that line, if any.
    pub fn task_at_point(&mut self, point: Point) -> Option<Task> {
        let Body::Source(content) = &mut self.body else {
            return None;
        };
        let point = Point::new(point.x - EDITOR_PADDING.1, point.y - EDITOR_PADDING.0);
        content.perform(Action::Click(point));
        let line = content.cursor().position.line;
        self.task_at_line(line)
    }

    // ---- Live Preview ----

    pub fn live_message(&mut self, message: live::Message, team: Option<&Format>) -> LiveOutcome {
        let mut outcome = LiveOutcome::default();
        let format = self.format.clone();
        match message {
            live::Message::Edit(action) => {
                if action.is_edit() {
                    let kind = edit_kind(&action);
                    self.record(kind);
                }
                let Body::Live(live) = &mut self.body else {
                    return outcome;
                };
                if live.perform(action, &format) {
                    if live.cursor().0 <= self.frontmatter_end + 1 {
                        self.refresh_format(team);
                    }
                    outcome.performed.timers = self.edited();
                }
            }
            live::Message::Key(intent) => {
                let edits = matches!(&self.body, Body::Live(live) if live.edits(intent));
                if edits {
                    self.record(EditKind::Other);
                }
                let Body::Live(live) = &mut self.body else {
                    return outcome;
                };
                let changed = live.key(intent, &format);
                outcome.focus = live.active.is_some();
                if changed {
                    if live.cursor().0 <= self.frontmatter_end + 2 {
                        self.refresh_format(team);
                    }
                    outcome.performed.timers = self.edited();
                }
            }
            live::Message::Press { line, cell, press } => {
                let Body::Live(live) = &mut self.body else {
                    return outcome;
                };
                match live.press(line, cell, press) {
                    Some(obsession_core::live::Action::Status) => {
                        if let Some(task) = self.task_at_line(line) {
                            if press.right {
                                outcome.menu = Some((task, press.window));
                            } else {
                                outcome.performed = self.advance(task);
                            }
                        }
                    }
                    Some(obsession_core::live::Action::Wiki(target)) if !press.right => {
                        outcome.open = Some(format!("{}{target}", preview::WIKI_SCHEME));
                    }
                    Some(obsession_core::live::Action::Url(url)) if !press.right => {
                        outcome.open = Some(url);
                    }
                    _ if press.right => {
                        if let Some(task) = self.task_at_line(line) {
                            outcome.menu = Some((task, press.window));
                        }
                    }
                    _ => outcome.focus = true,
                }
            }
            live::Message::EditorRight(local, window) => {
                let Body::Live(live) = &mut self.body else {
                    return outcome;
                };
                let line = live.click_editor(local);
                if let Some(task) = self.task_at_line(line) {
                    outcome.menu = Some((task, window));
                }
            }
            live::Message::Reveal(dy) => outcome.scroll = Some(dy),
            live::Message::Scrolled(viewport) => {
                if let Body::Live(live) = &mut self.body {
                    live.scrolled(viewport);
                }
            }
            live::Message::Background => {
                let Body::Live(live) = &mut self.body else {
                    return outcome;
                };
                let last = live.doc.len().saturating_sub(1);
                let end = live.doc.line(last).len();
                live.activate(last, end);
                outcome.focus = true;
            }
        }
        outcome
    }

    /// Takes an undo snapshot if this edit starts a new step.
    fn record(&mut self, kind: EditKind) {
        let line = self.cursor_line();
        if self.history.before_edit(kind, line, Instant::now()) {
            let snapshot = self.snapshot();
            self.history.push(snapshot);
        }
    }

    // ---- Undo ----

    pub fn undo(&mut self, redo: bool) -> Performed {
        let now = self.snapshot();
        let target = if redo {
            self.history.redo(now)
        } else {
            self.history.undo(now)
        };
        let Some(target) = target else {
            return Performed {
                timers: Vec::new(),
                message: Some(if redo { "Nothing to redo" } else { "Nothing to undo" }.into()),
            };
        };
        match &mut self.body {
            Body::Live(live) => live.restore(&target.text, target.cursor, &self.format),
            Body::Source(content) => {
                *content = text_editor::Content::with_text(&target.text);
                content.move_to(Cursor {
                    position: Position {
                        line: target.cursor.0,
                        column: target.cursor.1,
                    },
                    selection: None,
                });
            }
        }
        Performed {
            timers: self.edited(),
            message: None,
        }
    }

    // ---- Tasks ----

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
        let text = self.line(line)?;
        let (at, ch) = status::task_box(&text)?;
        let state = syntax::state_at(self.lines(), line);
        syntax::can_hold_task(state).then_some(Task { line, at, ch })
    }

    /// Replaces the one character between the brackets. The caret and any
    /// selection are put back where they were.
    pub fn set_status(&mut self, task: Task, ch: char) -> Vec<(Timer, Duration)> {
        if task.ch == ch {
            return Vec::new();
        }
        self.record(EditKind::Other);
        let start = task.at + 1;
        match &mut self.body {
            Body::Live(live) => {
                let raw = live.doc.line(task.line);
                let mut new = String::with_capacity(raw.len() + 4);
                new.push_str(&raw[..start]);
                new.push(ch);
                new.push_str(&raw[start + task.ch.len_utf8()..]);
                live.set_line(task.line, new, &self.format);
            }
            Body::Source(content) => {
                let saved = content.cursor();
                content.move_to(Cursor {
                    position: Position {
                        line: task.line,
                        column: start + task.ch.len_utf8(),
                    },
                    selection: Some(Position {
                        line: task.line,
                        column: start,
                    }),
                });
                content.perform(Action::Edit(Edit::Insert(ch)));
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
                content.move_to(Cursor {
                    position: shift(saved.position),
                    selection: saved.selection.map(shift),
                });
            }
        }
        self.history.break_group();
        self.edited()
    }

    // ---- Timers and preview ----

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

    pub fn refresh_preview_if_stale(&mut self) {
        if self.preview_stale {
            let text = self.text();
            self.analyze(&text);
            self.render_preview(&text);
        }
    }

    fn render_preview(&mut self, text: &str) {
        self.preview = markdown::Content::parse(&preview::to_markdown(text, &self.format));
        self.preview_stale = false;
    }

    // ---- Views ----

    pub fn view_live(&self) -> Option<Element<'_, live::Message>> {
        match &self.body {
            Body::Live(live) => Some(live.view(&self.statuses)),
            Body::Source(_) => None,
        }
    }

    pub fn view_editor(&self) -> Option<Element<'_, Action>> {
        let Body::Source(content) = &self.body else {
            return None;
        };
        Some(
            iced::widget::text_editor(content)
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
                .into(),
        )
    }

    pub fn view_preview(&self) -> Element<'_, markdown::Uri> {
        // iced's default code font is the system "monospace", which on
        // Windows lacks block and box-drawing characters (█ ░ ╔ ═), so ASCII
        // art falls apart. Cascadia Mono has them all.
        let mut style = markdown::Style::from_palette(theme::theme().palette());
        style.inline_code_font = theme::MONO_FONT;
        style.code_block_font = theme::MONO_FONT;
        let mut settings = markdown::Settings::with_text_size(15, style);
        settings.code_size = iced::Pixels(13.0);
        scrollable(
            iced::widget::container(markdown::view_with(
                self.preview.items(),
                settings,
                &PreviewViewer,
            ))
                .padding([24, 32])
                .max_width(860)
                .width(Length::Fill),
        )
        .height(Fill)
        .width(Fill)
        .into()
    }
}

/// The Reading view's look. Only code blocks differ from iced's default:
/// iced gives every piece of a code block the body font (overriding the
/// code font), so box-drawing art like `█ ╔═╗` falls apart. Here code is
/// drawn as plain lines in Cascadia Mono, tightly spaced and unwrapped, with
/// a horizontal scrollbar for long lines.
struct PreviewViewer;

/// Turns the preview step's private links (`hl:`, `at:`, `tag:`) back into
/// plain text with a highlight or a colour.
fn restyle(
    spans: &[iced::advanced::text::Span<'static, markdown::Uri>],
) -> Vec<iced::advanced::text::Span<'static, markdown::Uri>> {
    use obsession_core::preview::{HIGHLIGHT_SCHEME, MENTION_SCHEME, TAG_SCHEME};
    spans
        .iter()
        .cloned()
        .map(|mut span| {
            let Some(link) = span.link.as_deref() else {
                return span;
            };
            if link.starts_with(HIGHLIGHT_SCHEME) {
                span.link = None;
                span.color(theme::TEXT)
                    .background(iced::Background::Color(theme::HIGHLIGHT_BG))
                    .padding([0, 2])
            } else if link.starts_with(MENTION_SCHEME) {
                span.link = None;
                span.color(theme::MENTION)
            } else if link.starts_with(TAG_SCHEME) {
                span.link = None;
                span.color(theme::TAG)
            } else {
                span
            }
        })
        .collect()
}

impl<'a> markdown::Viewer<'a, markdown::Uri> for PreviewViewer {
    fn on_link_click(url: markdown::Uri) -> markdown::Uri {
        url
    }

    fn paragraph(
        &self,
        settings: markdown::Settings,
        text: &markdown::Text,
    ) -> Element<'a, markdown::Uri> {
        iced::widget::rich_text(restyle(&text.spans(settings.style)))
            .size(settings.text_size)
            .on_link_click(Self::on_link_click)
            .into()
    }

    fn heading(
        &self,
        settings: markdown::Settings,
        level: &'a markdown::HeadingLevel,
        text: &'a markdown::Text,
        index: usize,
    ) -> Element<'a, markdown::Uri> {
        use markdown::HeadingLevel as H;
        let size = match level {
            H::H1 => settings.h1_size,
            H::H2 => settings.h2_size,
            H::H3 => settings.h3_size,
            H::H4 => settings.h4_size,
            H::H5 => settings.h5_size,
            H::H6 => settings.h6_size,
        };
        let top = if index > 0 { settings.text_size.0 / 2.0 } else { 0.0 };
        iced::widget::container(
            iced::widget::rich_text(restyle(&text.spans(settings.style)))
                .size(size)
                .on_link_click(Self::on_link_click),
        )
        .padding(iced::Padding::new(0.0).top(top))
        .into()
    }

    fn code_block(
        &self,
        settings: markdown::Settings,
        _language: Option<&'a str>,
        code: &'a str,
        _lines: &'a [markdown::Text],
    ) -> Element<'a, markdown::Uri> {
        use iced::widget::{column, container, text};
        let lines = code
            .strip_suffix('\n')
            .unwrap_or(code)
            .split('\n')
            .map(|line| {
                text(line.strip_suffix('\r').unwrap_or(line))
                    .font(theme::MONO_FONT)
                    .size(settings.code_size)
                    .line_height(iced::widget::text::LineHeight::Relative(1.25))
                    .wrapping(iced::widget::text::Wrapping::None)
                    .color(theme::TEXT)
                    .into()
            });
        container(
            scrollable(container(column(lines)).padding(settings.code_size)).direction(
                scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::default()
                        .width(settings.code_size / 2)
                        .scroller_width(settings.code_size / 2),
                ),
            ),
        )
        .width(Fill)
        .padding(settings.code_size / 4)
        .style(theme::boxed(theme::PANEL))
        .into()
    }
}

/// Typing within a line groups into one undo step; anything else is its own.
fn edit_kind(action: &Action) -> EditKind {
    match action {
        Action::Edit(Edit::Insert(c)) if *c != '\n' => EditKind::Typing,
        Action::Edit(Edit::Backspace | Edit::Delete) => EditKind::Typing,
        _ => EditKind::Other,
    }
}
