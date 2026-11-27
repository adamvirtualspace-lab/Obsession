//! The open note: Live Preview, Source (raw editor) or Reading (preview),
//! its status format, undo history, and the timers behind autosave and the
//! debounced preview.

use std::borrow::Cow;
use std::cell::Cell;
use std::time::{Duration, Instant};

use iced::widget::text_editor::{self, Action, Cursor, Edit, Position};
use iced::{Element, Fill, Point, Size};

use obsession_core::history::{EditKind, History, Snapshot};
use obsession_core::status::{self, Format, Next, Source};
use obsession_core::{preview, syntax};

use crate::highlight::{LinkStates, Statuses};
use crate::live::{self, LINE_PX, Live};
use crate::pictures::Pictures;
use crate::source_scroll::SourceScroll;
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
    /// The raw markdown beside a read-only Live view of it.
    Split,
    /// Live Preview with editing off (links and status chips still click).
    Reading,
}

impl Mode {
    /// Split keeps a second, read-only Live view that follows the editor.
    pub fn shows_preview(self) -> bool {
        self == Mode::Split
    }

    /// Whether the note is edited as raw text (otherwise it's a Live view).
    fn raw(self) -> bool {
        matches!(self, Mode::Source | Mode::Split)
    }

    pub fn next(self) -> Self {
        match self {
            Mode::Live => Mode::Source,
            Mode::Source => Mode::Split,
            Mode::Split => Mode::Reading,
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
    /// A selection is being dragged at the page's edge: send
    /// `live::Message::EdgeScroll` after `live::EDGE_STEP`.
    pub edge_step: bool,
    /// A fresh editor appeared: lay the page out once more next frame (iced
    /// works out the caret and selection shapes before the editor's mixed
    /// fonts are applied, and only redoes them on the next layout).
    pub settle: bool,
}

enum Body {
    Live(Live),
    Source(text_editor::Content),
}

/// A point of the note both Split panes should show at the same height:
/// `frac` of the way down `line`, `screen` pixels below the pane's top.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    line: usize,
    frac: f32,
    screen: f32,
}

/// Keeps Split's read-only view in step with the editor.
#[derive(Debug, Default)]
struct Sync {
    /// What the view should line up with.
    anchor: Option<Anchor>,
    /// A scroll for the app to send to the view.
    pending: Option<f32>,
    /// The offset we last sent, so its echo isn't taken for the user
    /// scrolling the view.
    expect: Option<f32>,
    /// Corrections made after a scroll (heights of lines that hadn't been
    /// drawn are estimates until they are).
    tries: u8,
}

pub struct Note {
    /// Unique per opened note, so a timer from a previous note is ignored.
    pub id: u64,
    pub rel: Box<str>,
    body: Body,
    /// Split mode's read-only Live view of the Source text, refreshed a
    /// moment after typing stops.
    mirror: Option<Live>,
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
    /// The Source editor's side padding as last drawn: it grows with the
    /// window to centre the text column.
    source_inset: Cell<f32>,
    /// The Source editor's size as last drawn.
    source_size: Cell<Size>,
    /// Where the Source editor is scrolled to.
    scroll: SourceScroll,
    sync: Sync,
    /// Index of the line that closes the frontmatter (0 if there is none).
    /// Edits at or above it re-read `statuses:`.
    frontmatter_end: usize,
    /// Tasks whose character isn't in the format, as of the last full read
    /// (open, preview, save).
    pub unknown: usize,
    /// `@name` suggestions are showing: Tab picks one.
    completing: bool,
}

/// What the Source editor sends: an editor action, or Tab while `@name`
/// suggestions are showing.
#[derive(Debug, Clone)]
pub enum SourceEvent {
    Action(Action),
    Complete,
}

impl Note {
    pub fn new(id: u64, rel: Box<str>, text: &str, mode: Mode, team: Option<&Format>) -> Self {
        let format = Format::builtin();
        let body = if mode.raw() {
            Body::Source(text_editor::Content::with_text(text))
        } else {
            let mut live = Live::new(text, &format);
            live.read_only = mode == Mode::Reading;
            Body::Live(live)
        };
        let mut note = Self {
            id,
            rel,
            body,
            mirror: None,
            preview_stale: true,
            unsynced: false,
            last_edit: Instant::now(),
            preview_timer: false,
            save_timer: false,
            history: History::default(),
            statuses: Statuses::new(&format),
            source_inset: Cell::new(EDITOR_PADDING.1),
            source_size: Cell::new(Size::ZERO),
            scroll: SourceScroll::default(),
            sync: Sync::default(),
            format,
            source: Source::Default,
            warnings: Vec::new(),
            frontmatter_end: 0,
            unknown: 0,
            completing: false,
        };
        note.refresh_format(team);
        note.analyze(text);
        if mode.shows_preview() {
            note.refresh_preview_if_stale();
        }
        // An empty note starts ready to type.
        if let Body::Live(live) = &mut note.body
            && !live.read_only
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
        match (&self.body, mode.raw()) {
            (Body::Source(content), false) => {
                let cursor = content.cursor().position;
                let mut live = Live::new(&content.text(), &self.format);
                // A fresh Live knows nothing of links or suggestions yet;
                // `set_link_states` skips it when the states haven't changed.
                live.links = self.statuses.links.clone();
                live.completing = self.completing;
                if mode == Mode::Live {
                    live.activate(cursor.line, cursor.column);
                }
                self.body = Body::Live(live);
            }
            (Body::Live(live), true) => {
                let cursor = live.active.as_ref().map(|_| live.cursor());
                let mut content = text_editor::Content::with_text(&live.text());
                self.scroll.reset();
                if let Some((line, column)) = cursor {
                    content.move_to(Cursor {
                        position: Position { line, column },
                        selection: None,
                    });
                    if self.scroll.resize(self.text_area(), &content) {
                        self.scroll.reveal(&content, line, column);
                    }
                }
                self.body = Body::Source(content);
            }
            _ => {}
        }
        if let Body::Live(live) = &mut self.body {
            live.read_only = mode == Mode::Reading;
            if live.read_only {
                live.deactivate();
            }
        }
        if mode.shows_preview() {
            self.preview_stale = true;
            self.refresh_preview_if_stale();
            self.anchor_at_middle();
        } else {
            self.mirror = None;
            self.sync = Sync::default();
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
        let wheel = match action {
            Action::Scroll { lines } => Some(lines as f32 * LINE_PX),
            _ => None,
        };
        let Body::Source(content) = &mut self.body else {
            return Performed::default();
        };
        let before = (content.cursor(), content.line_count());
        content.perform(action);
        self.track_source(before, is_edit, wheel);

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
        let point = Point::new(point.x - self.source_inset.get(), point.y - EDITOR_PADDING.0);
        let before = (content.cursor(), content.line_count());
        content.perform(Action::Click(point));
        let line = content.cursor().position.line;
        self.track_source(before, false, None);
        self.task_at_line(line)
    }

    // ---- Links ----

    /// Sets how the note's links stand (re-colours them if that changed).
    pub fn link_states(&self) -> &LinkStates {
        &self.statuses.links
    }

    pub fn set_link_states(&mut self, states: LinkStates) {
        if self.statuses.links == states {
            return;
        }
        self.statuses.links = states.clone();
        if let Body::Live(live) = &mut self.body {
            live.links = states.clone();
        }
        if let Some(mirror) = &mut self.mirror {
            mirror.links = states;
        }
    }

    /// Shows `line` near the top: the caret goes there in Source, the page
    /// scrolls there in Live and Reading.
    pub fn jump_to(&mut self, line: usize) {
        let line = line.min(self.line_count().saturating_sub(1));
        match &mut self.body {
            Body::Source(content) => {
                let before = (content.cursor(), content.line_count());
                content.move_to(Cursor {
                    position: Position { line, column: 0 },
                    selection: None,
                });
                self.track_source(before, false, None);
            }
            Body::Live(live) => {
                live.deactivate();
                self.sync.anchor = Some(Anchor {
                    line,
                    frac: 0.0,
                    screen: 48.0,
                });
                self.sync_mirror();
            }
        }
    }

    /// Puts in text the app changed (a graph's nodes, wires or layout):
    /// undoable, and saved like typing.
    pub fn apply_text(&mut self, text: &str) -> Vec<(Timer, Duration)> {
        self.replace_text(text);
        self.edited()
    }

    /// Puts new text in (another note was renamed and this note's links to
    /// it were rewritten). Undo can bring the old text back.
    pub fn replace_text(&mut self, text: &str) {
        self.record(EditKind::Other);
        match &mut self.body {
            Body::Live(live) => live.reload(text, &self.format),
            Body::Source(content) => {
                let cursor = content.cursor().position;
                *content = text_editor::Content::with_text(text);
                let line = cursor.line.min(content.line_count().saturating_sub(1));
                let len = content.line(line).map_or(0, |l| l.text.len());
                content.move_to(Cursor {
                    position: Position {
                        line,
                        column: cursor.column.min(len),
                    },
                    selection: None,
                });
                self.scroll.reset();
            }
        }
        self.history.break_group();
        self.preview_stale = true;
        if self.mirror.is_some() {
            self.refresh_preview_if_stale();
        }
    }

    // ---- Split: keeping both panes at the same place ----

    /// The Source editor's text area (its size minus padding).
    fn text_area(&self) -> Size {
        let size = self.source_size.get();
        Size::new(
            size.width - 2.0 * self.source_inset.get(),
            size.height - 2.0 * EDITOR_PADDING.0,
        )
    }

    /// Follows what a Source action did to the editor's scroll, and lines
    /// the Split view up: with the caret if it moved, else with the middle.
    fn track_source(&mut self, before: (Cursor, usize), edited: bool, wheel: Option<f32>) {
        let area = self.text_area();
        let Body::Source(content) = &self.body else {
            return;
        };
        if !self.scroll.resize(area, content) {
            return;
        }
        if let Some(pixels) = wheel {
            self.scroll.scroll(pixels, content);
            self.anchor_at_middle();
            return;
        }
        let (old, old_count) = before;
        let now = content.cursor();
        if edited {
            let lines = [Some(old.position.line), old.selection.map(|p| p.line), Some(now.position.line)];
            let from = lines.iter().flatten().min().copied().unwrap_or(0);
            let to = lines.iter().flatten().max().copied().unwrap_or(0);
            self.scroll.edited(from, to, content.line_count() != old_count);
        } else if now == old {
            return;
        }
        let (line, column) = (now.position.line, now.position.column);
        self.scroll.reveal(content, line, column);
        self.anchor_at(line, column);
    }

    /// Lines the Split view up with the caret at (`line`, `column`).
    fn anchor_at(&mut self, line: usize, column: usize) {
        let Body::Source(content) = &self.body else {
            return;
        };
        let row = self.scroll.row_top(content, line, column);
        let height = self.scroll.line_height(content, line);
        let y = self.scroll.y_of(content, line, 0.0) + row;
        self.sync.anchor = Some(Anchor {
            line,
            frac: row / height.max(1.0),
            screen: EDITOR_PADDING.0 + y,
        });
        self.sync_mirror();
    }

    /// Lines the Split view up with the middle of the editor.
    fn anchor_at_middle(&mut self) {
        let area = self.text_area();
        let Body::Source(content) = &self.body else {
            return;
        };
        if !self.scroll.resize(area, content) {
            return;
        }
        let middle = self.scroll.height() / 2.0;
        let (line, frac) = self.scroll.at(content, middle);
        self.sync.anchor = Some(Anchor {
            line,
            frac,
            screen: EDITOR_PADDING.0 + middle,
        });
        self.sync_mirror();
    }

    /// The Live view on screen, if any: the note's own, or Split's.
    fn shown_live_ref(&self) -> Option<&Live> {
        match &self.body {
            Body::Live(live) => Some(live),
            Body::Source(_) => self.mirror.as_ref(),
        }
    }

    /// Asks for the Live view on screen to be scrolled to the anchor.
    fn sync_mirror(&mut self) {
        let (Some(live), Some(anchor)) = (self.shown_live_ref(), self.sync.anchor) else {
            return;
        };
        let target = (live.y_of(anchor.line, anchor.frac) - anchor.screen).max(0.0);
        self.sync.pending = Some(target);
        self.sync.expect = Some(target);
        self.sync.tries = 0;
    }

    /// A scroll the app should send to the Live view, if one is due.
    pub fn take_mirror_scroll(&mut self) -> Option<f32> {
        self.sync.pending.take()
    }

    /// The Live view scrolled: either our own scroll arriving (corrected if
    /// lines measured on the way changed the target), or the user scrolling
    /// it, which in Split scrolls the editor to match.
    fn view_scrolled(&mut self, viewport: iced::widget::scrollable::Viewport) {
        let anchor = self.sync.anchor;
        let expect = self.sync.expect;
        let tries = self.sync.tries;
        let Some(live) = self.shown_live() else {
            return;
        };
        let previous = live.viewport().0;
        live.scrolled(viewport);
        let y = viewport.absolute_offset().y;
        let max = (viewport.content_bounds().height - viewport.bounds().height).max(0.0);

        if let Some(expect) = expect {
            let ours = (y - expect).abs() < 1.5 || (expect >= max - 1.0 && y >= max - 1.0);
            if ours {
                // Lines drawn on the way may have changed the target.
                if let Some(anchor) = anchor
                    && tries < 3
                {
                    let target = (live.y_of(anchor.line, anchor.frac) - anchor.screen).max(0.0);
                    if (target.min(max) - y).abs() > 2.0 {
                        self.sync.tries += 1;
                        self.sync.pending = Some(target);
                        self.sync.expect = Some(target);
                    }
                }
                return;
            }
        }
        if (y - previous).abs() < 0.5 {
            // Only the content or size changed, not the offset.
            return;
        }

        // The user scrolled: stop steering, and in Split bring the editor's
        // middle to match.
        let middle = live.at(y + viewport.bounds().height / 2.0);
        self.sync.expect = None;
        self.sync.anchor = None;
        let (line, frac) = middle;
        let area = self.text_area();
        let Body::Source(content) = &mut self.body else {
            return;
        };
        if !self.scroll.resize(area, content) {
            return;
        }
        let screen = viewport.bounds().height / 2.0 - EDITOR_PADDING.0;
        let delta = self.scroll.y_of(content, line, frac) - screen;
        let lines = (delta / LINE_PX).round() as i32;
        if lines != 0 {
            content.perform(Action::Scroll { lines });
            self.scroll.scroll(lines as f32 * LINE_PX, content);
        }
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
                let Some(live) = self.shown_live() else {
                    return outcome;
                };
                let read_only = live.read_only;
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
                    Some(obsession_core::live::Action::Mention(name)) if !press.right => {
                        outcome.open = Some(format!("@{name}"));
                    }
                    _ if press.right => {
                        if let Some(task) = self.task_at_line(line) {
                            outcome.menu = Some((task, press.window));
                        }
                    }
                    _ => outcome.focus = !read_only,
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
            live::Message::Scrolled(viewport) => self.view_scrolled(viewport),
            live::Message::DragAt { line, cell, offset } => {
                if let Some(live) = self.shown_live() {
                    live.drag_to(line, cell, offset);
                }
            }
            live::Message::Pointer(at) => {
                outcome.edge_step = self.shown_live().is_some_and(|live| live.pointer(at));
            }
            live::Message::EdgeScroll => {
                if let Some(dy) = self.shown_live().and_then(Live::edge_scroll) {
                    outcome.scroll = Some(dy);
                    outcome.edge_step = true;
                }
            }
            // The app looks pictures up after every message.
            live::Message::PicturesShown => {}
            live::Message::MouseUp => {
                if let Some(live) = self.shown_live() {
                    outcome.focus = live.release();
                    outcome.settle = outcome.focus;
                }
            }
            live::Message::Background => {
                let Body::Live(live) = &mut self.body else {
                    return outcome;
                };
                if live.read_only {
                    return outcome;
                }
                let last = live.doc.len().saturating_sub(1);
                let end = live.doc.line(last).len();
                live.activate(last, end);
                outcome.focus = true;
            }
        }
        outcome
    }

    /// The Live view on screen: the note itself, or Split's mirror.
    fn shown_live(&mut self) -> Option<&mut Live> {
        match &mut self.body {
            Body::Live(live) => Some(live),
            Body::Source(_) => self.mirror.as_mut(),
        }
    }

    /// A selection is being dragged in the Live view (the app then follows
    /// the pointer, for scrolling at the edges).
    pub fn live_dragging(&self) -> bool {
        self.shown_live_ref().is_some_and(Live::dragging)
    }

    /// The text selected in Reading (or Split's mirror), to copy.
    pub fn selection_text(&self) -> Option<String> {
        self.shown_live_ref().and_then(Live::selection_text)
    }

    /// Drops a selection kept in Reading. Returns whether there was one.
    pub fn clear_selection(&mut self) -> bool {
        self.shown_live().is_some_and(Live::clear_selection)
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
                self.scroll.reset();
                let area = self.text_area();
                if let Body::Source(content) = &self.body
                    && self.scroll.resize(area, content)
                {
                    self.scroll.reveal(content, target.cursor.0, target.cursor.1);
                    self.anchor_at(target.cursor.0, target.cursor.1);
                }
            }
        }
        Performed {
            timers: self.edited(),
            message: None,
        }
    }

    // ---- Tasks ----

    /// Moves `task` to the next status in the format (`Format::click`: in
    /// the built-in format, done goes back to to do).
    pub fn advance(&mut self, task: Task) -> Performed {
        match self.format.click(task.ch, self.source) {
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
                let restored = shift(saved.position);
                content.move_to(Cursor {
                    position: restored,
                    selection: saved.selection.map(shift),
                });
                let area = self.text_area();
                if let Body::Source(content) = &self.body
                    && self.scroll.resize(area, content)
                {
                    self.scroll.edited(task.line, task.line, false);
                    self.scroll.reveal(content, restored.line, restored.column);
                }
            }
        }
        self.history.break_group();
        let timers = self.edited();
        // A click in Split's mirror should show its result right away.
        if self.mirror.is_some() {
            self.refresh_preview_if_stale();
        }
        timers
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

    /// Brings Split's mirror up to date with the Source text.
    pub fn refresh_preview_if_stale(&mut self) {
        if !self.preview_stale || !matches!(self.body, Body::Source(_)) {
            return;
        }
        let text = self.text();
        self.analyze(&text);
        match &mut self.mirror {
            // Reusing it keeps its scroll position.
            Some(mirror) => mirror.reload(&text, &self.format),
            None => {
                let mut mirror = Live::new(&text, &self.format);
                mirror.read_only = true;
                mirror.links = self.statuses.links.clone();
                self.mirror = Some(mirror);
            }
        }
        self.preview_stale = false;
        self.sync_mirror();
    }

    // ---- Views ----

    pub fn view_live(&self, pictures: &Pictures) -> Option<Element<'_, live::Message>> {
        match &self.body {
            Body::Live(live) => Some(live.view(&self.statuses, pictures)),
            Body::Source(_) => None,
        }
    }

    /// Split mode's read-only view of the Source text.
    pub fn view_mirror(&self, pictures: &Pictures) -> Option<Element<'_, live::Message>> {
        self.mirror.as_ref().map(|mirror| mirror.view(&self.statuses, pictures))
    }

    /// The picture embeds near the visible part of the Live view on screen
    /// (the note's own, or Split's): (target as written, `|width`).
    pub fn pictures_near(&self) -> Vec<(String, u32)> {
        self.shown_live_ref().map_or_else(Vec::new, Live::pictures_near)
    }

    /// The `@name` being typed right before the caret (without the `@`;
    /// empty just after typing `@`), if any.
    pub fn mention_prefix(&self) -> Option<String> {
        let (content, active) = match &self.body {
            Body::Live(live) => (&live.content, live.active.is_some()),
            Body::Source(content) => (content, true),
        };
        if !active {
            return None;
        }
        let cursor = content.cursor();
        if cursor.selection.is_some() {
            return None;
        }
        let line = content.line(cursor.position.line)?;
        let before: String = line.text.chars().take(cursor.position.column).collect();
        let at = before.rfind('@')?;
        let typed = &before[at + 1..];
        let word_start = at == 0 || !before[..at].ends_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
        let name_chars = typed.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
        (word_start && name_chars).then(|| typed.to_owned())
    }

    /// Shows or hides `@name` suggestions (Tab then completes instead of
    /// indenting).
    pub fn set_completing(&mut self, on: bool) {
        self.completing = on;
        if let Body::Live(live) = &mut self.body {
            live.completing = on;
        }
    }

    /// Replaces the `@typed` before the caret with `@name `.
    pub fn complete_mention(&mut self, typed: usize, name: &str, team: Option<&Format>) -> Performed {
        let mut actions: Vec<Action> = (0..=typed).map(|_| Action::Select(text_editor::Motion::Left)).collect();
        actions.push(Action::Edit(Edit::Paste(std::sync::Arc::new(format!("@{name} ")))));
        let mut out = Performed::default();
        for action in actions {
            let performed = match &self.body {
                Body::Live(_) => self.live_message(live::Message::Edit(action), team).performed,
                Body::Source(_) => self.perform(action, team),
            };
            out.timers.extend(performed.timers);
        }
        out
    }

    pub fn view_editor(&self) -> Option<Element<'_, SourceEvent>> {
        let Body::Source(content) = &self.body else {
            return None;
        };
        let statuses = &self.statuses;
        let inset = &self.source_inset;
        let editor_size = &self.source_size;
        // The editor spans the whole pane (so the wheel and the scrollbar
        // work everywhere), with side padding that centres the text in the
        // same column as Live Preview and Reading.
        Some(
            iced::widget::responsive(move |size| {
                let side = ((size.width - live::TEXT_WIDTH) / 2.0).max(EDITOR_PADDING.1);
                inset.set(side);
                editor_size.set(size);
                let completing = self.completing;
                iced::widget::text_editor(content)
                    .on_action(SourceEvent::Action)
                    .key_binding(move |press| {
                        let focused = matches!(press.status, text_editor::Status::Focused { .. });
                        #[cfg(debug_assertions)]
                        if std::env::var_os("OBSESSION_DEV_KEYS").is_some() {
                            eprintln!("source binding {:?} focused={focused} completing={completing}", press.key);
                        }
                        if completing
                            && focused
                            && press.key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab)
                        {
                            return Some(text_editor::Binding::Custom(SourceEvent::Complete));
                        }
                        text_editor::Binding::from_key_press(press)
                    })
                    .highlight_with::<crate::highlight::Markdown>(
                        statuses.clone(),
                        crate::highlight::format,
                    )
                    .font(theme::MONO_FONT)
                    .size(theme::TEXT_SIZE)
                    .line_height(iced::Pixels(live::LINE_PX))
                    .padding(iced::Padding {
                        top: EDITOR_PADDING.0,
                        bottom: EDITOR_PADDING.0,
                        left: side,
                        right: side,
                    })
                    .height(Fill)
                    .style(theme::editor)
                    .into()
            })
            .into(),
        )
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
