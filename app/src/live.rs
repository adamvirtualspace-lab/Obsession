//! Live Preview: the note drawn formatted, with the block under the caret
//! shown as raw markdown in a small editor.
//!
//! The note is held as lines (`Doc`). Each line keeps a drawn form, rebuilt
//! only for the lines an edit touched (or whose role in a block changed).
//! The active block is a `text_editor` holding just that block's lines; every
//! edit is written straight back into the `Doc`, so saving and undo always
//! see the current text.
//!
//! Moving between blocks: Up/Down/Left/Right at the edge of the editor,
//! Enter (splits a line, continuing lists), Backspace at the start and
//! Delete at the end (join lines), a click on a drawn line (the caret lands
//! on the clicked character), Escape to stop editing. Up/Down keep the
//! column they started from through shorter (or blank) lines.
//!
//! Selecting: a drag across drawn lines; past the page's top or bottom edge
//! the page scrolls along. Editable, the lines it covered open in the
//! editor with that text selected; read-only (Reading, Split's mirror), the
//! selection stays highlighted and Ctrl+C copies its drawn text.

use std::cell::Cell;
use std::ops::Range;
use std::time::Duration;

use iced::advanced::text::Span;
use iced::font::{Style as FontStyle, Weight};
use iced::keyboard::{Key, key::Named};
use iced::widget::text::LineHeight;
use iced::widget::text_editor::{self, Action, Binding, Cursor, KeyPress, Position};
use iced::widget::{Id, Space, column, container, mouse_area, row, rule, scrollable, text};
use iced::{Background, Border, Element, Fill, Font, Length, Point, Rectangle};

use obsession_core::doc::Doc;
use obsession_core::live::{self, Block, BlockKind, Line as Drawn, Style};
use obsession_core::status::Format;
use obsession_core::syntax::LineState;

use crate::highlight::{self, LinkStates, Statuses};
use crate::live_text::{LiveText, Press};
use crate::pictures::{self, PictureColumn, Pictures};
use crate::right_click::right_click;
use crate::theme;

pub const TEXT_SIZE: f32 = 16.0;
const LINE_HEIGHT: LineHeight = LineHeight::Relative(1.5);
/// One line of body text, in pixels. Source mode uses the same pitch so
/// switching modes doesn't change how many lines fit on screen.
pub const LINE_PX: f32 = TEXT_SIZE * 1.5;
/// Code, frontmatter and comments: tighter, so box-drawing art and aligned
/// text stay joined up, as in a terminal.
const CODE_LINE_HEIGHT: LineHeight = LineHeight::Relative(1.25);
const CODE_SIZE: f32 = TEXT_SIZE - 1.0;
/// Space between blocks.
const SPACING: f32 = 0.0;
/// Page padding: top/bottom, left/right.
const PAGE_PADDING: (f32, f32) = (28.0, 40.0);
/// The page's widest, padding included. All three modes centre a page this
/// wide so the text sits in the same column whichever mode is on.
pub const PAGE_WIDTH: f32 = 880.0;
/// The widest the text itself gets (the page minus its side padding).
pub const TEXT_WIDTH: f32 = PAGE_WIDTH - 2.0 * PAGE_PADDING.1;
/// How far above and below the visible part blocks are still built.
const OVERSCAN: f32 = 900.0;
/// Left inset of a list line `columns` of whitespace deep: the width those
/// spaces take in Source mode, so nesting looks the same in both modes.
fn indent_px(columns: u8) -> f32 {
    static SPACE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let space = *SPACE.get_or_init(|| text_width(" ", theme::MONO_FONT, theme::TEXT_SIZE));
    f32::from(columns) * space
}
/// A table cell's padding: top/bottom, left/right.
const CELL_PADDING: (f32, f32) = (4.0, 10.0);
/// Text inset per `>` level of a quote.
const QUOTE_INDENT: f32 = 16.0;
/// While selecting, the page scrolls when the pointer is this close to its
/// top or bottom edge (or past it)...
const EDGE: f32 = 24.0;
/// ...by this much a step per pixel into that band (2–60 px a step)...
const EDGE_SPEED: f32 = 0.5;
/// ...a step this often.
pub const EDGE_STEP: Duration = Duration::from_millis(16);

pub fn editor_id() -> Id {
    Id::new("live-editor")
}

pub fn scroll_id() -> Id {
    Id::new("live-scroll")
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(Action),
    Key(Intent),
    /// A press on a drawn line (`cell` for table cells).
    Press {
        line: usize,
        cell: Option<usize>,
        press: Press,
    },
    /// A right-click on the active editor: (editor point, window point).
    EditorRight(Point, Point),
    /// Scroll the page by this much to show the active editor.
    Reveal(f32),
    /// A click below the last line.
    Background,
    Scrolled(scrollable::Viewport),
    /// While selecting: the cursor moved over a drawn line, to this offset
    /// of its drawn text.
    DragAt {
        line: usize,
        cell: Option<usize>,
        offset: usize,
    },
    /// The left mouse button went up (anywhere in the window).
    MouseUp,
    /// While selecting: where the pointer is (a window point), anywhere in
    /// the window, for scrolling at the page's edges.
    Pointer(Point),
    /// A step of scrolling while a selection is dragged at an edge.
    EdgeScroll,
    /// Pictures came into view that haven't been looked up yet.
    PicturesShown,
}

/// A selection being dragged across drawn lines: (doc line, byte column).
#[derive(Debug, Clone, Copy)]
struct Drag {
    anchor: (usize, usize),
    head: (usize, usize),
    moved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Enter,
    Up,
    Down,
    Left,
    Right,
    JoinPrev,
    JoinNext,
    Escape,
    /// Tab with `@name` suggestions showing.
    Complete,
}

/// How a line is drawn, decided by the block it's in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Line,
    FenceEdge,
    Code,
    FrontEdge,
    Front,
    TableSeparator,
    TableRow,
    TableHeader,
    Comment,
}

enum Rendered {
    Line(Drawn),
    Cells(Vec<Drawn>),
    Hidden,
}

pub struct Live {
    pub doc: Doc,
    blocks: Vec<Block>,
    roles: Vec<Role>,
    rendered: Vec<Rendered>,
    /// The doc lines shown in the editor, if a block is being edited.
    pub active: Option<Range<usize>>,
    pub content: text_editor::Content,
    /// Bumped when the editor should be scrolled into view.
    reveal: u64,
    /// Each line's drawn height, once it has been on screen (0 = not yet).
    heights: Vec<Cell<f32>>,
    /// Each line's estimated height until then, worked out once for the
    /// page's width (0 = not yet).
    estimates: Vec<Cell<f32>>,
    /// The height of each line's pictures, the same way.
    picture_heights: Vec<Cell<f32>>,
    /// The width pictures were last laid out at (0 = not yet).
    picture_column: Cell<f32>,
    /// What's above the first line: a crew card's portrait, or nothing.
    header: Cell<f32>,
    /// Scroll offset and visible height of the page.
    scroll: (f32, f32),
    /// Reading mode and Split's mirror: links and status chips click, but
    /// nothing starts editing.
    pub read_only: bool,
    /// How each `[[link]]` stands, for its colour.
    pub links: LinkStates,
    /// A selection being dragged, until the button goes up.
    drag: Option<Drag>,
    /// A finished selection in a read-only view: highlighted until the next
    /// press; Ctrl+C copies it.
    kept: Option<Drag>,
    /// The page on screen (window coordinates) and its content's height.
    bounds: Rectangle,
    content_height: f32,
    /// While selecting: the pointer, and whether an edge scroll step is
    /// pending.
    pointer: Option<Point>,
    edge_scrolling: bool,
    /// The column (in characters) Up/Down aim for, kept while the caret
    /// passes through shorter lines.
    goal: Option<usize>,
    /// The button went down in the editor (a drag may leave it).
    editor_pressed: bool,
    /// `@name` suggestions are showing: Tab completes.
    pub completing: bool,
}

impl Live {
    pub fn new(text: &str, format: &Format) -> Self {
        let mut live = Self {
            doc: Doc::from_text(text),
            blocks: Vec::new(),
            roles: Vec::new(),
            rendered: Vec::new(),
            active: None,
            content: text_editor::Content::new(),
            reveal: 0,
            heights: Vec::new(),
            estimates: Vec::new(),
            picture_heights: Vec::new(),
            picture_column: Cell::new(0.0),
            header: Cell::new(0.0),
            scroll: (0.0, 1600.0),
            read_only: false,
            links: LinkStates::default(),
            drag: None,
            kept: None,
            bounds: Rectangle::default(),
            content_height: 0.0,
            pointer: None,
            edge_scrolling: false,
            goal: None,
            editor_pressed: false,
            completing: false,
        };
        live.rebuild(format);
        live
    }

    /// Re-draws every line (after a status format change).
    pub fn rebuild(&mut self, format: &Format) {
        self.blocks = live::blocks(self.doc.lines());
        self.roles = roles(&self.blocks, self.doc.len(), self.doc.lines());
        self.rendered = (0..self.doc.len())
            .map(|i| render(self.doc.line(i), self.roles[i], format))
            .collect();
        self.heights = (0..self.doc.len()).map(|_| Cell::new(0.0)).collect();
        self.estimates = (0..self.doc.len()).map(|_| Cell::new(0.0)).collect();
        self.picture_heights = (0..self.doc.len()).map(|_| Cell::new(0.0)).collect();
    }

    /// Replaces the whole text without editing it (Split's mirror).
    pub fn reload(&mut self, text: &str, format: &Format) {
        self.doc = Doc::from_text(text);
        self.active = None;
        self.rebuild(format);
    }

    pub fn scrolled(&mut self, viewport: scrollable::Viewport) {
        // A new width wraps lines differently: measure and estimate again.
        if (viewport.bounds().width - self.bounds.width).abs() > 0.5 {
            for cell in self.heights.iter().chain(&self.estimates) {
                cell.set(0.0);
            }
        }
        self.scroll = (viewport.absolute_offset().y, viewport.bounds().height);
        self.bounds = viewport.bounds();
        self.content_height = viewport.content_bounds().height;
        #[cfg(debug_assertions)]
        if std::env::var_os("OBSESSION_DEV_TIMING").is_some() {
            // How far the estimates were off, for the lines drawn so far.
            let (mut count, mut off, mut net) = (0, 0.0f32, 0.0f32);
            for line in 0..self.doc.len() {
                if self.heights[line].get() > 0.0 {
                    let error = self.height(line, false) - self.height(line, true);
                    count += 1;
                    off += error.abs();
                    net += error;
                }
            }
            let ours = self.y_of(self.doc.len().saturating_sub(1), 1.0) + PAGE_PADDING.0 + 240.0;
            let tables: f32 = (0..self.doc.len()).filter(|&i| matches!(self.rendered[i], Rendered::Cells(_))).map(|i| self.line_height(i)).sum();
            let rules = (0..self.doc.len()).filter(|&i| matches!(&self.rendered[i], Rendered::Line(d) if d.rule)).count();
            eprintln!(
                "TIMING live heights: {count} of {} lines drawn, estimates off by {off:.0} px ({net:+.0} net), content {:.0} px, ours {ours:.0} px, table rows {tables:.0} px, rules {rules}",
                self.doc.len(),
                self.content_height
            );
        }
    }

    pub fn text(&self) -> String {
        self.doc.to_text()
    }

    /// Replaces doc lines and re-draws only what changed.
    fn replace_lines(&mut self, range: Range<usize>, new: Vec<String>, format: &Format) {
        let count = new.len();
        let start = range.start;
        self.doc.replace(range.clone(), new);
        let placeholder = || Role::Line;
        self.roles
            .splice(range.clone(), std::iter::repeat_with(placeholder).take(count));
        self.rendered
            .splice(range.clone(), std::iter::repeat_with(|| Rendered::Hidden).take(count));
        self.heights
            .splice(range.clone(), std::iter::repeat_with(|| Cell::new(0.0)).take(count));
        self.estimates
            .splice(range.clone(), std::iter::repeat_with(|| Cell::new(0.0)).take(count));
        self.picture_heights
            .splice(range, std::iter::repeat_with(|| Cell::new(0.0)).take(count));

        self.blocks = live::blocks(self.doc.lines());
        let roles = roles(&self.blocks, self.doc.len(), self.doc.lines());
        for (i, role) in roles.iter().enumerate() {
            let touched = (start..start + count).contains(&i);
            if touched || self.roles[i] != *role {
                self.rendered[i] = render(self.doc.line(i), *role, format);
                self.heights[i].set(0.0);
                self.estimates[i].set(0.0);
                self.picture_heights[i].set(0.0);
            }
        }
        self.roles = roles;
    }

    fn block_of(&self, line: usize) -> Option<&Block> {
        let index = self
            .blocks
            .partition_point(|b| b.lines.end <= line)
            .min(self.blocks.len().saturating_sub(1));
        self.blocks.get(index)
    }

    /// Starts editing the block holding `line`, caret at `column` (bytes).
    pub fn activate(&mut self, line: usize, column: usize) {
        let line = line.min(self.doc.len().saturating_sub(1));
        let range = self
            .block_of(line)
            .map_or(line..line + 1, |b| b.lines.clone());
        self.content = text_editor::Content::with_text(&self.doc.joined(range.clone()));
        let text = self.doc.line(line);
        let column = floor_char_boundary(text, column.min(text.len()));
        self.content.move_to(Cursor {
            position: Position {
                line: line - range.start,
                column,
            },
            selection: None,
        });
        self.active = Some(range);
        self.reveal += 1;
        self.goal = None;
    }

    pub fn deactivate(&mut self) {
        self.active = None;
    }

    /// Reloads the editor from the doc (after a change made outside it),
    /// keeping the caret where it was.
    fn reload_active(&mut self) {
        if let Some(range) = self.active.clone() {
            let cursor = self.cursor();
            let range = self
                .block_of(range.start)
                .map_or(range.clone(), |b| b.lines.clone());
            self.active = Some(range);
            self.activate(cursor.0, cursor.1);
        }
    }

    /// The caret as (doc line, byte column).
    pub fn cursor(&self) -> (usize, usize) {
        let start = self.active.as_ref().map_or(0, |r| r.start);
        let p = self.content.cursor().position;
        (start + p.line, p.column)
    }

    /// Applies an editor action; returns whether the text changed.
    pub fn perform(&mut self, action: Action, format: &Format) -> bool {
        let is_edit = action.is_edit();
        let click = matches!(action, Action::Click(_));
        let drag = matches!(action, Action::Drag(_));
        self.content.perform(action);
        self.goal = None;
        if click {
            self.editor_pressed = true;
        }
        // A selection that left the editor and came back into it (not while
        // the page scrolls at an edge: the pointer is then off the editor).
        let cursor = self.cursor();
        if drag
            && !self.edge_scrolling
            && let Some(selection) = &mut self.drag
        {
            selection.head = cursor;
            selection.moved = true;
        }
        if is_edit {
            self.sync_from_editor(format);
            self.reveal += 1;
        }
        is_edit
    }

    /// Writes the editor's lines back into the doc.
    fn sync_from_editor(&mut self, format: &Format) {
        let Some(range) = self.active.clone() else {
            return;
        };
        let lines: Vec<String> = (0..self.content.line_count())
            .filter_map(|i| self.content.line(i).map(|l| l.text.into_owned()))
            .collect();
        let lines = if lines.is_empty() { vec![String::new()] } else { lines };
        let count = lines.len();
        self.replace_lines(range.clone(), lines, format);
        self.active = Some(range.start..range.start + count);
    }

    /// Whether `intent` will change the text (so undo can take a snapshot).
    pub fn edits(&self, intent: Intent) -> bool {
        let Some(range) = &self.active else {
            return false;
        };
        match intent {
            Intent::Enter => true,
            Intent::JoinPrev => range.start > 0,
            Intent::JoinNext => range.end < self.doc.len(),
            _ => false,
        }
    }

    /// Handles a key the editor handed over. Returns whether the text changed.
    pub fn key(&mut self, intent: Intent, format: &Format) -> bool {
        let Some(range) = self.active.clone() else {
            return false;
        };
        let (line, column) = self.cursor();
        let vertical = matches!(intent, Intent::Up | Intent::Down);
        // The column Up/Down aim for: kept from the first of a run of them.
        let want = match self.goal {
            Some(goal) if vertical => goal,
            _ => chars_before(self.doc.line(line), column),
        };
        self.goal = None;
        match intent {
            // The app completes the `@name`; it never gets here.
            Intent::Complete => false,
            Intent::Escape => {
                self.deactivate();
                false
            }
            Intent::Up | Intent::Down | Intent::Left | Intent::Right => {
                // Let the editor move first (wrapped lines move within the
                // line); only leave the block if the caret didn't move.
                let before = self.content.cursor().position;
                let motion = match intent {
                    Intent::Up => text_editor::Motion::Up,
                    Intent::Down => text_editor::Motion::Down,
                    Intent::Left => text_editor::Motion::Left,
                    _ => text_editor::Motion::Right,
                };
                self.content.perform(Action::Move(motion));
                if self.content.cursor().position != before {
                    if vertical {
                        self.goal = Some(want);
                    }
                    return false;
                }
                match intent {
                    Intent::Up | Intent::Left if range.start > 0 => {
                        let target = range.start - 1;
                        let column = if intent == Intent::Left {
                            self.doc.line(target).len()
                        } else {
                            byte_at_char(self.doc.line(target), want)
                        };
                        self.activate(target, column);
                    }
                    Intent::Down | Intent::Right if range.end < self.doc.len() => {
                        let column = if intent == Intent::Right {
                            0
                        } else {
                            byte_at_char(self.doc.line(range.end), want)
                        };
                        self.activate(range.end, column);
                    }
                    _ => {}
                }
                if vertical {
                    self.goal = Some(want);
                }
                false
            }
            Intent::Enter => {
                // Remove any selection first.
                if self.content.cursor().selection.is_some() {
                    self.content
                        .perform(Action::Edit(text_editor::Edit::Delete));
                    self.sync_from_editor(format);
                }
                let (line, column) = self.cursor();
                let raw = self.doc.line(line).to_owned();
                let column = floor_char_boundary(&raw, column.min(raw.len()));
                let (left, right) = raw.split_at(column);
                let prefix = continuation(&raw, format);
                // Enter on an empty list item ends the list.
                if let Some(prefix) = &prefix
                    && left.trim_end() == prefix.trim_end()
                    && right.trim().is_empty()
                {
                    self.replace_lines(line..line + 1, vec![String::new()], format);
                    self.activate(line, 0);
                    return true;
                }
                let prefix = prefix.filter(|_| column > 0).unwrap_or_default();
                let next = format!("{prefix}{}", right.trim_start_matches(' '));
                let caret = prefix.len();
                self.replace_lines(line..line + 1, vec![left.to_owned(), next], format);
                self.activate(line + 1, caret);
                true
            }
            Intent::JoinPrev => {
                if range.start == 0 {
                    return false;
                }
                let prev = range.start - 1;
                let joined_at = self.doc.line(prev).len();
                let merged = format!("{}{}", self.doc.line(prev), self.doc.line(range.start));
                self.replace_lines(prev..range.start + 1, vec![merged], format);
                self.activate(prev, joined_at);
                true
            }
            Intent::JoinNext => {
                let last = range.end - 1;
                if range.end >= self.doc.len() {
                    return false;
                }
                let joined_at = self.doc.line(last).len();
                let merged = format!("{}{}", self.doc.line(last), self.doc.line(range.end));
                self.replace_lines(last..range.end + 1, vec![merged], format);
                self.activate(last, joined_at);
                let _ = line;
                true
            }
        }
    }

    /// A press on a drawn line. Returns the action to take, if the press hit
    /// a link or chip; otherwise starts editing at the pressed character.
    pub fn press(&mut self, line: usize, cell: Option<usize>, press: Press) -> Option<live::Action> {
        let drawn = match (self.rendered.get(line), cell) {
            (Some(Rendered::Line(d)), _) => Some(d),
            (Some(Rendered::Cells(cells)), Some(c)) => cells.get(c),
            _ => None,
        };
        if let (Some(drawn), Some(link)) = (drawn, press.link)
            && let Some(action) = drawn.segs.get(link).and_then(|s| s.action.clone())
        {
            return Some(action);
        }
        if press.right {
            return None;
        }
        self.kept = None;
        // Editing starts when the button goes up: a drag selects instead
        // (read-only, a drag selects and a click does nothing).
        let raw_len = self.doc.line(line).len();
        let column = drawn.map_or(raw_len, |d| d.raw_offset(press.offset, raw_len));
        self.drag = Some(Drag {
            anchor: (line, column),
            head: (line, column),
            moved: false,
        });
        None
    }

    /// Whether a selection may be dragged now (drawn lines then report
    /// where the cursor is).
    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.editor_pressed
    }

    /// The cursor moved over a drawn line while the button is down.
    pub fn drag_to(&mut self, line: usize, cell: Option<usize>, offset: usize) {
        let drawn = match (self.rendered.get(line), cell) {
            (Some(Rendered::Line(d)), _) => Some(d),
            (Some(Rendered::Cells(cells)), Some(c)) => cells.get(c),
            _ => None,
        };
        let raw_len = self.doc.line(line).len();
        let column = drawn.map_or(raw_len, |d| d.raw_offset(offset, raw_len));
        self.drag_head(line, column);
    }

    /// Moves the end of the selection being dragged to (line, byte column).
    fn drag_head(&mut self, line: usize, column: usize) {
        if self.drag.is_none() && self.editor_pressed && self.active.is_some() {
            // The drag began in the editor: its anchor is where it began.
            let start = self.active.as_ref().map_or(0, |r| r.start);
            let cursor = self.content.cursor();
            let anchor = cursor.selection.unwrap_or(cursor.position);
            self.drag = Some(Drag {
                anchor: (start + anchor.line, anchor.column),
                head: (line, column),
                moved: false,
            });
        }
        if let Some(drag) = &mut self.drag {
            drag.head = (line, column);
            drag.moved |= drag.head != drag.anchor;
        }
    }

    /// The button went up. A plain click starts editing where it landed; a
    /// drag edits every line it covered, with that text selected. Returns
    /// whether the editor should take the keyboard.
    pub fn release(&mut self) -> bool {
        self.editor_pressed = false;
        self.pointer = None;
        self.edge_scrolling = false;
        let Some(drag) = self.drag.take() else {
            return false;
        };
        if self.read_only {
            self.kept = drag.moved.then_some(drag);
            return false;
        }
        if !drag.moved {
            self.activate(drag.anchor.0, drag.anchor.1);
            return true;
        }
        self.select(drag.anchor, drag.head);
        true
    }

    /// Edits the blocks from `anchor`'s line to `head`'s with the text
    /// between them selected (copy, cut, typing and Delete then work as in
    /// any editor).
    fn select(&mut self, anchor: (usize, usize), head: (usize, usize)) {
        let last = self.doc.len().saturating_sub(1);
        let (anchor, head) = ((anchor.0.min(last), anchor.1), (head.0.min(last), head.1));
        let (lo, hi) = (anchor.0.min(head.0), anchor.0.max(head.0));
        let start = self.block_of(lo).map_or(lo, |b| b.lines.start.min(lo));
        let end = self.block_of(hi).map_or(hi + 1, |b| b.lines.end.max(hi + 1));
        self.content = text_editor::Content::with_text(&self.doc.joined(start..end));
        let position = |(line, column): (usize, usize)| {
            let text = self.doc.line(line);
            Position {
                line: line - start,
                column: floor_char_boundary(text, column.min(text.len())),
            }
        };
        let (head, anchor) = (position(head), position(anchor));
        self.content.move_to(Cursor {
            position: head,
            selection: Some(anchor),
        });
        // No reveal: the end the drag reached is on screen, and revealing
        // the editor would scroll back to the selection's first line.
        self.active = Some(start..end);
    }

    /// The pointer moved while selecting. Returns whether a step of edge
    /// scrolling should start (`Message::EdgeScroll` after `EDGE_STEP`).
    pub fn pointer(&mut self, at: Point) -> bool {
        if !self.dragging() {
            return false;
        }
        self.pointer = Some(at);
        if self.edge_scrolling || self.edge_speed() == 0.0 {
            return false;
        }
        self.edge_scrolling = true;
        true
    }

    /// How far a step of edge scrolling goes: up is negative; 0 when the
    /// pointer isn't near an edge, or the page can't go further that way.
    fn edge_speed(&self) -> f32 {
        let Some(p) = self.pointer else {
            return 0.0;
        };
        let b = self.bounds;
        if b.height <= 0.0 {
            return 0.0;
        }
        let (offset, visible) = self.scroll;
        let max = (self.content_height - visible).max(0.0);
        let speed = |into: f32| (into * EDGE_SPEED).clamp(2.0, 60.0);
        if p.y < b.y + EDGE && offset > 0.5 {
            -speed(b.y + EDGE - p.y)
        } else if p.y > b.y + b.height - EDGE && offset < max - 0.5 {
            speed(p.y - (b.y + b.height - EDGE))
        } else {
            0.0
        }
    }

    /// A step of edge scrolling: the selection's end moves to the line at
    /// the pointer's height after it. Returns how far to scroll, or `None`
    /// when it should stop.
    pub fn edge_scroll(&mut self) -> Option<f32> {
        let speed = if self.dragging() { self.edge_speed() } else { 0.0 };
        let Some(p) = self.pointer.filter(|_| speed != 0.0) else {
            self.edge_scrolling = false;
            return None;
        };
        let (offset, visible) = self.scroll;
        let max = (self.content_height - visible).max(0.0);
        let after = (offset + speed).clamp(0.0, max);
        // Counted as scrolled now, in case the next step comes before the
        // scrollable reports it.
        self.scroll.0 = after;
        let (line, _) = self.at(after + (p.y - self.bounds.y).clamp(0.0, visible));
        let column = if speed > 0.0 { self.doc.line(line).len() } else { 0 };
        self.drag_head(line, column);
        Some(after - offset)
    }

    /// The kept selection's text as drawn (Reading's Ctrl+C): lines joined
    /// with newlines, table cells with tabs.
    pub fn selection_text(&self) -> Option<String> {
        let kept = self.kept?;
        let (from, to) = if kept.anchor <= kept.head { (kept.anchor, kept.head) } else { (kept.head, kept.anchor) };
        let last = self.doc.len().saturating_sub(1);
        let mut out = Vec::new();
        for line in from.0.min(last)..=to.0.min(last) {
            match &self.rendered[line] {
                Rendered::Hidden => {}
                Rendered::Cells(cells) => out.push(cells.iter().map(drawn_text).collect::<Vec<_>>().join("\t")),
                Rendered::Line(drawn) => {
                    let text = drawn_text(drawn);
                    let piece = self
                        .selected(line)
                        .and_then(|raw| drawn_range(drawn, raw))
                        .and_then(|r| text.get(r))
                        .unwrap_or("");
                    out.push(piece.to_owned());
                }
            }
        }
        Some(out.join("\n"))
    }

    /// Drops the kept selection. Returns whether there was one.
    pub fn clear_selection(&mut self) -> bool {
        self.kept.take().is_some()
    }

    /// The raw bytes of `line` inside the selection being dragged (or the
    /// one kept in a read-only view).
    fn selected(&self, line: usize) -> Option<Range<usize>> {
        let drag = self.drag.filter(|d| d.moved).or(self.kept)?;
        let (from, to) = if drag.anchor <= drag.head {
            (drag.anchor, drag.head)
        } else {
            (drag.head, drag.anchor)
        };
        if line < from.0 || line > to.0 {
            return None;
        }
        let len = self.doc.line(line).len();
        let start = if line == from.0 { from.1.min(len) } else { 0 };
        let end = if line == to.0 { to.1.min(len) } else { len };
        (start < end).then_some(start..end)
    }

    /// Changes one line from outside the editor (a status click).
    pub fn set_line(&mut self, line: usize, text: String, format: &Format) {
        self.replace_lines(line..line + 1, vec![text], format);
        if self.active.as_ref().is_some_and(|r| r.contains(&line)) {
            self.reload_active();
        }
    }

    /// Puts the whole text back (undo/redo) with the caret at `cursor`.
    pub fn restore(&mut self, text: &str, cursor: (usize, usize), format: &Format) {
        self.doc = Doc::from_text(text);
        self.rebuild(format);
        if !self.read_only {
            self.activate(cursor.0, cursor.1);
        }
    }

    /// Clicks the active editor at a point (for a right-click), returning
    /// the doc line the caret landed on.
    pub fn click_editor(&mut self, point: Point) -> usize {
        self.content.perform(Action::Click(point));
        self.cursor().0
    }

    pub fn view<'a>(&'a self, statuses: &Statuses, pictures: &Pictures) -> Element<'a, Message> {
        let mut items: Vec<Element<'a, Message>> = Vec::new();
        let mut editor_shown = false;
        let active = self.active.clone().unwrap_or(usize::MAX..usize::MAX);

        // Only blocks near the visible part of the page are built; the rest
        // are stood in for by empty space of the same (measured or
        // estimated) height. The edited block is always built.
        let (offset, visible) = self.scroll;
        let top = offset - OVERSCAN;
        let bottom = offset + visible + OVERSCAN;
        let mut y = PAGE_PADDING.0;
        let mut gap = 0.0;

        // A crew card's portrait, above its text.
        self.header.set(0.0);
        if let Some(portrait) = pictures.header() {
            items.push(
                container(
                    iced::widget::image(portrait)
                        .width(Length::Fixed(pictures::PORTRAIT as f32))
                        .height(Length::Fixed(pictures::PORTRAIT as f32))
                        .border_radius(pictures::PORTRAIT as f32 / 2.0),
                )
                .padding(iced::Padding::new(0.0).bottom(12.0))
                .into(),
            );
            y += pictures::PORTRAIT as f32 + 12.0;
            self.header.set(pictures::PORTRAIT as f32 + 12.0);
        }

        for block in &self.blocks {
            let overlaps = block.lines.start < active.end && active.start < block.lines.end;
            let height = self.block_height(block);
            let near = y + height >= top && y <= bottom;
            y += height + SPACING;
            if !near && !overlaps {
                gap += height + SPACING;
                continue;
            }
            if gap > 0.0 {
                items.push(Space::new().height(gap - SPACING).into());
                gap = 0.0;
            }
            if overlaps {
                for i in block.lines.start..active.start.max(block.lines.start) {
                    items.push(self.line_element(i, pictures));
                }
                if !editor_shown {
                    editor_shown = true;
                    items.push(self.editor_element(statuses));
                }
                for i in active.end.min(block.lines.end).max(block.lines.start)..block.lines.end {
                    if i >= active.end {
                        items.push(self.line_element(i, pictures));
                    }
                }
                continue;
            }
            items.push(self.block_element(block, pictures));
        }
        if gap > 0.0 {
            items.push(Space::new().height(gap - SPACING).into());
        }

        // Only the empty space below the last line starts editing at the
        // end; margins and the gaps between lines do nothing.
        let space = Space::new().width(Fill).height(Length::Fixed(240.0));
        let below: Element<'a, Message> = if self.read_only || self.dragging() {
            space.into()
        } else {
            mouse_area(space)
                .on_press(Message::Background)
                .interaction(iced::mouse::Interaction::Text)
                .into()
        };
        let page = column(items)
            .push(below)
            .spacing(SPACING)
            .padding([PAGE_PADDING.0, PAGE_PADDING.1])
            .max_width(PAGE_WIDTH)
            .width(Fill);

        scrollable(container(page).center_x(Fill))
        .id(scroll_id())
        .on_scroll(Message::Scrolled)
        .height(Fill)
        .width(Fill)
        .into()
    }

    /// The width to decode a picture for (see `Pictures::decode_width`).
    fn decode_width(&self, spec: Option<u32>) -> Option<u32> {
        Pictures::decode_width(spec, self.picture_column.get())
    }

    /// The picture embeds in the blocks `view` builds (near the visible
    /// part of the page): (target as written, the width to decode it for).
    /// Empty until pictures have been laid out once (the column's unknown).
    pub fn pictures_near(&self) -> Vec<(String, u32)> {
        let (offset, visible) = self.scroll;
        let (top, bottom) = (offset - OVERSCAN, offset + visible + OVERSCAN);
        let active = self.active.clone().unwrap_or(usize::MAX..usize::MAX);
        let mut y = PAGE_PADDING.0 + self.header.get();
        let mut out = Vec::new();
        for block in &self.blocks {
            let height = self.block_height(block);
            let near = y + height >= top && y <= bottom;
            let editing = block.lines.start < active.end && active.start < block.lines.end;
            y += height + SPACING;
            if !near && !editing {
                continue;
            }
            for i in block.lines.clone() {
                if let Rendered::Line(drawn) = &self.rendered[i] {
                    out.extend(
                        drawn
                            .pictures
                            .iter()
                            .filter_map(|p| Some((p.target.clone(), self.decode_width(p.width)?))),
                    );
                }
            }
        }
        out
    }

    /// A line's height: measured if it has been drawn, estimated if not.
    fn line_height(&self, line: usize) -> f32 {
        self.height(line, true)
    }

    /// A line's height, measured (if `measured` and it has been drawn) or
    /// estimated.
    fn height(&self, line: usize, measured: bool) -> f32 {
        let measured = if measured { self.heights.get(line).map_or(0.0, Cell::get) } else { 0.0 };
        let (text, extra) = match &self.rendered[line] {
            Rendered::Hidden => return 0.0,
            // The tallest cell, and the cells' padding.
            Rendered::Cells(_) if measured > 0.0 => return measured + CELL_PADDING.0 * 2.0,
            Rendered::Cells(cells) => {
                let cell = &self.estimates[line];
                if cell.get() == 0.0 {
                    let width = self.text_column() / cells.len().max(1) as f32 - CELL_PADDING.1 * 2.0;
                    let size = TEXT_SIZE - 1.0;
                    let rows = cells
                        .iter()
                        .map(|c| wrapped_rows(&drawn_text(c), size, width.max(size * 2.0), false))
                        .max()
                        .unwrap_or(1);
                    cell.set(rows as f32 * size * 1.5);
                }
                return cell.get() + CELL_PADDING.0 * 2.0;
            }
            Rendered::Line(drawn) if drawn.rule => return 25.0,
            Rendered::Line(drawn) => {
                let top = if drawn.heading > 0 { 10.0 } else { 0.0 };
                let pictures = if drawn.pictures.is_empty() {
                    0.0
                } else {
                    let measured = self.picture_heights[line].get();
                    if measured > 0.0 { measured } else { pictures::estimate(drawn.pictures.len()) }
                };
                if pictures_only(drawn) {
                    return top + pictures;
                }
                (drawn, top + pictures)
            }
        };
        if measured > 0.0 {
            return measured + extra;
        }
        let mono = matches!(
            self.roles[line],
            Role::Code | Role::FenceEdge | Role::Front | Role::Comment
        );
        if mono {
            return CODE_SIZE * 1.25;
        }
        let cell = &self.estimates[line];
        if cell.get() == 0.0 {
            cell.set(self.estimate(text));
        }
        cell.get() + extra
    }

    /// The text column's width: the page, as wide as the view allows,
    /// minus its side padding.
    fn text_column(&self) -> f32 {
        let page = if self.bounds.width > 0.0 { self.bounds.width.min(PAGE_WIDTH) } else { PAGE_WIDTH };
        page - 2.0 * PAGE_PADDING.1
    }

    /// A line's text height before it's drawn: its words wrapped at the
    /// width it will get, with each character's real width.
    fn estimate(&self, drawn: &Drawn) -> f32 {
        let width = self.text_column() - indent_px(drawn.indent) - f32::from(drawn.quote) * QUOTE_INDENT;
        let size = heading_size(drawn.heading);
        let rows = wrapped_rows(&drawn_text(drawn), size, width.max(size * 4.0), drawn.heading > 0);
        rows as f32 * size * 1.5
    }

    fn block_height(&self, block: &Block) -> f32 {
        let lines: f32 = block.lines.clone().map(|i| self.line_height(i)).sum();
        match block.kind {
            BlockKind::Line | BlockKind::Comment | BlockKind::Table => lines,
            BlockKind::Fence => lines + 18.0,
            BlockKind::Frontmatter => lines + 36.0,
        }
    }

    /// The scroll offset and visible height of the page.
    pub fn viewport(&self) -> (f32, f32) {
        self.scroll
    }

    /// Where a line's own text starts inside its block (boxes have padding,
    /// and the frontmatter a "Properties" label).
    fn block_inset(kind: BlockKind) -> f32 {
        match kind {
            BlockKind::Fence => 8.0,
            BlockKind::Frontmatter => 28.0,
            _ => 0.0,
        }
    }

    /// The page y of a point `frac` of the way down `line` (measured
    /// heights where lines have been drawn, estimates elsewhere).
    pub fn y_of(&self, line: usize, frac: f32) -> f32 {
        let mut y = PAGE_PADDING.0 + self.header.get();
        for block in &self.blocks {
            if block.lines.end <= line {
                y += self.block_height(block) + SPACING;
                continue;
            }
            y += Self::block_inset(block.kind);
            for i in block.lines.start..line.max(block.lines.start) {
                y += self.line_height(i);
            }
            return y + frac * self.line_height(line.min(self.doc.len().saturating_sub(1)));
        }
        y
    }

    /// The line at page y `y`, and how far into it.
    pub fn at(&self, y: f32) -> (usize, f32) {
        let mut top = PAGE_PADDING.0 + self.header.get();
        for block in &self.blocks {
            let height = self.block_height(block);
            if y >= top + height + SPACING {
                top += height + SPACING;
                continue;
            }
            let mut line_top = top + Self::block_inset(block.kind);
            for i in block.lines.clone() {
                let h = self.line_height(i);
                if y < line_top + h || i + 1 == block.lines.end {
                    let frac = if h > 0.0 { (y - line_top) / h } else { 0.0 };
                    return (i, frac.clamp(0.0, 1.0));
                }
                line_top += h;
            }
        }
        (self.doc.len().saturating_sub(1), 1.0)
    }

    fn block_element<'a>(&'a self, block: &Block, pictures: &Pictures) -> Element<'a, Message> {
        let lines = block.lines.clone();
        match block.kind {
            BlockKind::Line => self.line_element(lines.start, pictures),
            BlockKind::Fence => container(column(lines.map(|i| self.line_element(i, pictures))))
                .padding([8, 14])
                .width(Fill)
                .style(theme::boxed(theme::PANEL))
                .into(),
            BlockKind::Frontmatter => container(
                column![text("Properties").size(theme::SMALL).color(theme::FAINT)]
                    .extend(lines.map(|i| self.line_element(i, pictures))),
            )
            .padding([8, 14])
            .width(Fill)
            .style(theme::boxed(theme::PANEL))
            .into(),
            BlockKind::Table => column(lines.map(|i| self.line_element(i, pictures))).into(),
            BlockKind::Comment => column(lines.map(|i| self.line_element(i, pictures))).into(),
        }
    }

    fn line_element<'a>(&'a self, line: usize, pictures: &Pictures) -> Element<'a, Message> {
        match &self.rendered[line] {
            Rendered::Hidden => Space::new().height(0).into(),
            Rendered::Cells(cells) => {
                let header = self.roles[line] == Role::TableHeader;
                row(cells.iter().enumerate().map(|(c, drawn)| {
                    container(
                        LiveText::new(spans(drawn, 0, header, theme::UI_FONT, &self.links, None), move |press| Message::Press {
                            line,
                            cell: Some(c),
                            press,
                        })
                        .on_drag(self.dragging(), move |offset| Message::DragAt {
                            line,
                            cell: Some(c),
                            offset,
                        })
                        .measure_tallest(&self.heights[line])
                        .size(TEXT_SIZE - 1.0)
                        .line_height(LINE_HEIGHT)
                        .font(theme::UI_FONT)
                        .color(theme::TEXT),
                    )
                    .padding([CELL_PADDING.0, CELL_PADDING.1])
                    .width(Fill)
                    .style(theme::cell(header))
                    .into()
                }))
                .into()
            }
            Rendered::Line(drawn) => {
                let role = self.roles[line];
                let mono = matches!(
                    role,
                    Role::Code | Role::FenceEdge | Role::Front | Role::Comment
                );
                if drawn.rule {
                    return container(rule::horizontal(1)).padding([12, 0]).into();
                }
                let size = heading_size(drawn.heading);
                let base = if mono { theme::MONO_FONT } else { theme::UI_FONT };
                let selected = self.selected(line).and_then(|raw| drawn_range(drawn, raw));
                let mut widget = LiveText::new(
                    spans(drawn, drawn.heading, false, base, &self.links, selected),
                    move |press| Message::Press {
                        line,
                        cell: None,
                        press,
                    },
                )
                .on_drag(self.dragging(), move |offset| Message::DragAt {
                    line,
                    cell: None,
                    offset,
                })
                .measure(&self.heights[line])
                .size(if mono { CODE_SIZE } else { size })
                .line_height(if mono { CODE_LINE_HEIGHT } else { LINE_HEIGHT })
                .color(theme::TEXT);
                if mono {
                    widget = widget.no_wrap();
                }
                widget = widget.font(if mono {
                    theme::MONO_FONT
                } else if drawn.heading > 0 {
                    Font {
                        weight: Weight::Semibold,
                        ..theme::UI_FONT
                    }
                } else {
                    theme::UI_FONT
                });
                let widget: Element<'_, Message> = widget.into();
                let top = if drawn.heading > 0 { 10.0 } else { 0.0 };
                // A line of only pictures shows just them; otherwise they go
                // under its text.
                let body: Element<'_, Message> = match self.picture_column(line, drawn, pictures) {
                    None => widget,
                    Some(pics) if pictures_only(drawn) => pics,
                    Some(pics) => column![widget, pics].into(),
                };
                let mut element: Element<'_, Message> = container(body)
                    .padding(iced::Padding::new(0.0).top(top).left(indent_px(drawn.indent)))
                    .width(Fill)
                    .clip(mono)
                    .into();
                if drawn.quote > 0 {
                    // The text sets the height; the bars (a later stack
                    // layer) stretch to it.
                    let depth = f32::from(drawn.quote);
                    let text = container(element)
                        .padding(iced::Padding::new(0.0).left(depth * QUOTE_INDENT))
                        .width(Fill);
                    let bars = row((0..drawn.quote).map(|_| {
                        container(Space::new().width(3).height(Fill))
                            .style(theme::quote_bar)
                            .into()
                    }))
                    .spacing(13)
                    .height(Fill);
                    element = iced::widget::stack![text, bars].into();
                }
                element
            }
        }
    }

    /// A line's pictures, if it has any.
    fn picture_column<'a>(&'a self, line: usize, drawn: &Drawn, pictures: &Pictures) -> Option<Element<'a, Message>> {
        if drawn.pictures.is_empty() {
            return None;
        }
        let items = drawn
            .pictures
            .iter()
            .map(|p| pictures::Item {
                name: p.target.rsplit('/').next().unwrap_or(&p.target).to_owned(),
                width: p.width,
                shown: pictures.shown(&p.target, self.decode_width(p.width)),
            })
            .collect();
        // Each picture stands for its placeholder piece of the line.
        let starts: Vec<(usize, usize)> = drawn
            .pictures
            .iter()
            .map(|p| (p.seg, drawn.segs[..p.seg].iter().map(|s| s.text.len()).sum()))
            .collect();
        let read_only = self.read_only;
        Some(
            PictureColumn::new(items, &self.picture_heights[line], &self.picture_column, move |i, right, window| {
                let (seg, offset) = starts[i];
                Message::Press {
                    line,
                    cell: None,
                    press: pictures::press(seg, offset, read_only, right, window),
                }
            })
            .on_pending(Message::PicturesShown)
            .into(),
        )
    }

    fn editor_element<'a>(&'a self, statuses: &Statuses) -> Element<'a, Message> {
        let range = self.active.clone().unwrap_or(0..1);
        let kind = self
            .block_of(range.start)
            .filter(|b| b.lines == range)
            .map_or(BlockKind::Line, |b| b.kind);
        let single = kind == BlockKind::Line && range.len() == 1;
        // A selection over ordinary lines edits them as plain text; code,
        // tables and the frontmatter keep the monospace box.
        let plain = self
            .blocks
            .iter()
            .filter(|b| b.lines.start < range.end && range.start < b.lines.end)
            .all(|b| b.kind == BlockKind::Line);
        let first_line = self.doc.line(range.start);
        let heading = if single { heading_level(first_line) } else { 0 };
        let mono = !plain;

        let completing = self.completing;
        let cursor = self.content.cursor();
        let last = self.content.line_count().saturating_sub(1);
        let line_len = self
            .content
            .line(cursor.position.line)
            .map_or(0, |l| l.text.len());
        let has_selection = cursor.selection.is_some();
        let at_start = cursor.position.line == 0 && cursor.position.column == 0;
        let at_end = cursor.position.line == last && cursor.position.column >= line_len;
        let on_first = cursor.position.line == 0;
        let on_last = cursor.position.line == last;

        let first_state = if range.start == 0 {
            LineState::Start
        } else {
            LineState::Normal
        };

        let editor = iced::widget::text_editor(&self.content)
            .id(editor_id())
            .on_action(Message::Edit)
            .key_binding(move |press: KeyPress| {
                #[cfg(debug_assertions)]
                if std::env::var_os("OBSESSION_DEV_KEYS").is_some() {
                    eprintln!("live binding {:?} {:?} completing={completing}", press.key, press.status);
                }
                if !matches!(press.status, text_editor::Status::Focused { .. }) {
                    return Binding::from_key_press(press);
                }
                let m = press.modifiers;
                let plain = !m.shift() && !m.control() && !m.alt() && !m.logo();
                let custom = |intent| Some(Binding::Custom(Message::Key(intent)));
                match press.key.as_ref() {
                    Key::Named(Named::Enter) if single && !m.shift() => custom(Intent::Enter),
                    Key::Named(Named::Backspace) if at_start && !has_selection => {
                        custom(Intent::JoinPrev)
                    }
                    Key::Named(Named::Delete) if at_end && !has_selection => {
                        custom(Intent::JoinNext)
                    }
                    Key::Named(Named::ArrowUp) if plain && on_first => custom(Intent::Up),
                    Key::Named(Named::ArrowDown) if plain && on_last => custom(Intent::Down),
                    Key::Named(Named::ArrowLeft) if plain && at_start && !has_selection => {
                        custom(Intent::Left)
                    }
                    Key::Named(Named::ArrowRight) if plain && at_end && !has_selection => {
                        custom(Intent::Right)
                    }
                    Key::Named(Named::Escape) => custom(Intent::Escape),
                    Key::Named(Named::Tab) if completing => custom(Intent::Complete),
                    _ => Binding::from_key_press(press),
                }
            })
            .highlight_with::<highlight::Markdown>(
                statuses.clone().starting(first_state),
                highlight::live_format,
            )
            .font(if mono {
                theme::MONO_FONT
            } else if heading > 0 {
                Font {
                    weight: Weight::Semibold,
                    ..theme::UI_FONT
                }
            } else {
                theme::UI_FONT
            })
            .size(if mono { CODE_SIZE } else { heading_size(heading) })
            .line_height(if mono { CODE_LINE_HEIGHT } else { LINE_HEIGHT })
            .padding(0)
            .style(theme::live_editor);

        let editor = right_click(editor, Message::EditorRight)
            .ignore_wheel()
            .reveal(self.reveal, Message::Reveal);

        let top = if heading > 0 { 10.0 } else { 0.0 };
        // Keep the text where it was drawn: shift the raw line right so its
        // first character of text lines up with the drawn one, and the
        // markup (`- [ ] `, `> `, indentation) appears to its left.
        let left = if single {
            let font = if heading > 0 {
                Font {
                    weight: Weight::Semibold,
                    ..theme::UI_FONT
                }
            } else {
                theme::UI_FONT
            };
            self.align_offset(range.start, first_state, font, heading_size(heading))
        } else {
            0.0
        };
        let mut element: Element<'a, Message> = container(editor)
            .padding(iced::Padding::new(0.0).top(top).left(left))
            .width(Fill)
            .into();
        if mono {
            element = container(element)
                .padding([8, 14])
                .style(theme::boxed(theme::PANEL))
                .into();
        }
        element
    }
}

impl Live {
    /// How far right the raw line must start so that its text begins where
    /// the drawn line's text begins (never negative: a heading's `## ` just
    /// pushes its text right, as in Obsidian).
    fn align_offset(&self, line: usize, state: LineState, font: Font, size: f32) -> f32 {
        let Some(Rendered::Line(drawn)) = self.rendered.get(line) else {
            return 0.0;
        };
        let raw = self.doc.line(line);
        // The first piece of real text, and the raw byte it starts at.
        let (before, raw_at) = match drawn
            .segs
            .iter()
            .position(|s| s.verbatim && !s.text.is_empty())
        {
            Some(i) => (
                drawn.segs[..i].iter().map(|s| s.text.as_str()).collect::<String>(),
                drawn.segs[i].raw.start,
            ),
            None => (drawn.text(), raw.len()),
        };
        let drawn_x = indent_px(drawn.indent)
            + f32::from(drawn.quote) * QUOTE_INDENT
            + text_width(&before, font, size);
        // The raw prefix is drawn in two fonts: markup (`- [ ] `, `> `) in
        // mono, as the editor's highlighter sets it, and the rest in `font`.
        let raw_at = raw_at.min(raw.len());
        let (spans, _) = obsession_core::syntax::scan_line(state, raw);
        let mut raw_x = 0.0;
        let mut at = 0;
        for (range, kind) in spans {
            if range.start >= raw_at {
                break;
            }
            if !highlight::is_markup(kind) {
                continue;
            }
            let end = range.end.min(raw_at);
            raw_x += text_width(&raw[at..range.start], font, size);
            raw_x += text_width(&raw[range.start..end], theme::MONO_FONT, size);
            at = end;
        }
        raw_x += text_width(&raw[at..raw_at], font, size);
        (drawn_x - raw_x).max(0.0)
    }
}

/// The width of `text` as drawn (trailing spaces included).
fn text_width(text: &str, font: Font, size: f32) -> f32 {
    use iced::advanced::text::Paragraph as _;
    type P = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;
    if text.is_empty() {
        return 0.0;
    }
    let measure = |content: &str| {
        P::with_text(iced::advanced::Text {
            content,
            bounds: iced::Size::INFINITE,
            size: iced::Pixels(size),
            line_height: LINE_HEIGHT,
            font,
            align_x: iced::widget::text::Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::widget::text::Shaping::Advanced,
            wrapping: iced::widget::text::Wrapping::None,
        })
        .min_width()
    };
    // A marker after the text keeps trailing spaces in the measurement.
    measure(&format!("{text}|")) - measure("|")
}

/// Decides how each line is drawn from the blocks.
fn roles(blocks: &[Block], len: usize, lines: &[String]) -> Vec<Role> {
    let mut roles = vec![Role::Line; len];
    for block in blocks {
        let range = block.lines.clone();
        for i in range.clone() {
            let first = i == range.start;
            let last = i + 1 == range.end;
            roles[i] = match block.kind {
                BlockKind::Line => Role::Line,
                BlockKind::Fence => {
                    let closes = last && i != range.start && {
                        let t = lines[i].trim();
                        !t.is_empty() && t.chars().all(|c| c == '`' || c == '~')
                    };
                    if first || closes {
                        Role::FenceEdge
                    } else {
                        Role::Code
                    }
                }
                BlockKind::Frontmatter if first || last => Role::FrontEdge,
                BlockKind::Frontmatter => Role::Front,
                BlockKind::Table if i == range.start + 1 => Role::TableSeparator,
                BlockKind::Table if first => Role::TableHeader,
                BlockKind::Table => Role::TableRow,
                BlockKind::Comment => Role::Comment,
            };
        }
    }
    roles
}

fn render(raw: &str, role: Role, format: &Format) -> Rendered {
    match role {
        Role::Line => Rendered::Line(live::line(raw, format)),
        Role::FenceEdge => Rendered::Line(live::verbatim(raw, Style(Style::FAINT))),
        Role::Code => Rendered::Line(live::verbatim(raw, Style(Style::CODE))),
        Role::FrontEdge | Role::TableSeparator => Rendered::Hidden,
        Role::Front => Rendered::Line(live::verbatim(raw, Style(Style::MARKER))),
        Role::TableRow | Role::TableHeader => Rendered::Cells(live::table_cells(raw, format)),
        Role::Comment => Rendered::Line(live::verbatim(raw, Style(Style::FAINT))),
    }
}

/// What a new line after `raw` starts with when Enter is pressed: the same
/// indentation and list marker (`- `, `3. ` → `4. `, a task gets a fresh box
/// with the format's first status).
fn continuation(raw: &str, format: &Format) -> Option<String> {
    let lead = raw.len() - raw.trim_start_matches([' ', '\t', '>']).len();
    let indent = &raw[..lead];
    let rest = &raw[lead..];
    let mut chars = rest.chars();
    let marker = chars.next()?;
    if matches!(marker, '-' | '*' | '+') && rest[1..].starts_with(' ') {
        if obsession_core::status::task_box(raw).is_some() {
            let first = format.statuses().first().map_or(' ', |s| s.ch);
            return Some(format!("{indent}{marker} [{first}] "));
        }
        return Some(format!("{indent}{marker} "));
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let after = &rest[digits..];
        if let Some(sep) = after.chars().next().filter(|c| matches!(c, '.' | ')'))
            && after[1..].starts_with(' ')
        {
            let n: u64 = rest[..digits].parse().ok()?;
            return Some(format!("{indent}{}{sep} ", n + 1));
        }
    }
    if indent.contains('>') {
        return Some(indent.to_owned());
    }
    None
}

fn callout_color(kind: Option<&str>) -> iced::Color {
    match kind.unwrap_or("note") {
        "warning" | "caution" | "attention" => theme::WARNING,
        "danger" | "error" | "bug" | "failure" | "fail" | "missing" => theme::DANGER,
        "tip" | "hint" | "important" | "success" | "check" | "done" => theme::SUCCESS,
        "question" | "help" | "faq" | "example" => theme::MENTION,
        _ => theme::LINK,
    }
}

/// The line is nothing but picture embeds (and spaces).
fn pictures_only(drawn: &Drawn) -> bool {
    !drawn.pictures.is_empty()
        && drawn
            .segs
            .iter()
            .enumerate()
            .all(|(i, s)| s.text.trim().is_empty() || drawn.pictures.iter().any(|p| p.seg == i))
}

fn heading_level(line: &str) -> u8 {
    let lead = line.bytes().take(4).take_while(|&b| b == b' ').count();
    if lead > 3 {
        return 0;
    }
    let line = &line[lead..];
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && matches!(line.as_bytes().get(hashes), None | Some(b' ' | b'\t')) {
        hashes as u8
    } else {
        0
    }
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 28.0,
        2 => 23.0,
        3 => 19.5,
        4 => 17.0,
        _ => TEXT_SIZE,
    }
}

/// How many characters come before byte `byte` of `text`.
fn chars_before(text: &str, byte: usize) -> usize {
    text[..floor_char_boundary(text, byte.min(text.len()))].chars().count()
}

/// The byte where character `chars` of `text` starts (the end if it's
/// shorter).
fn byte_at_char(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// How many rows `text` takes when wrapped at `width` (words kept whole
/// unless one is wider than a row), in the UI font at `size`.
fn wrapped_rows(text: &str, size: f32, width: f32, bold: bool) -> usize {
    // Semibold headings run a little wider.
    let k = if bold { size * 1.05 } else { size };
    let (mut rows, mut x) = (1, 0.0f32);
    for word in text.split_inclusive(' ') {
        let body = word.trim_end_matches(' ');
        let w = body.chars().map(advance).sum::<f32>() * k;
        let spaces = (word.len() - body.len()) as f32 * advance(' ') * k;
        if x > 0.0 && x + w > width {
            rows += 1;
            x = 0.0;
        }
        if w > width {
            for c in body.chars() {
                let a = advance(c) * k;
                if x > 0.0 && x + a > width {
                    rows += 1;
                    x = 0.0;
                }
                x += a;
            }
        } else {
            x += w;
        }
        x += spaces;
    }
    rows
}

/// A character's width in the UI font, per pixel of size (measured once).
fn advance(c: char) -> f32 {
    thread_local! {
        static ADVANCES: std::cell::RefCell<std::collections::HashMap<char, f32>> = Default::default();
    }
    ADVANCES.with_borrow_mut(|known| {
        *known
            .entry(c)
            .or_insert_with(|| text_width(c.encode_utf8(&mut [0; 4]), theme::UI_FONT, 100.0) / 100.0)
    })
}

/// A drawn line's text, as shown.
fn drawn_text(drawn: &Drawn) -> String {
    drawn.segs.iter().map(|s| s.text.as_str()).collect()
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Turns a drawn line into styled spans. Spans with an action carry their
/// piece index as the link, so presses can tell what was hit.
/// Where a raw byte range of a line falls in its drawn text. Pieces drawn
/// differently from the raw text (chips, aliases) count whole.
fn drawn_range(drawn: &Drawn, raw: Range<usize>) -> Option<Range<usize>> {
    let mut at = 0;
    let mut out: Option<Range<usize>> = None;
    for seg in &drawn.segs {
        let len = seg.text.len();
        if seg.raw.start < raw.end && raw.start < seg.raw.end.max(seg.raw.start + 1) && len > 0 {
            let (a, b) = if seg.verbatim {
                let a = raw.start.saturating_sub(seg.raw.start).min(len);
                let b = (raw.end - seg.raw.start).min(len);
                (floor_char_boundary(&seg.text, a), floor_char_boundary(&seg.text, b))
            } else {
                (0, len)
            };
            if a < b {
                let piece = at + a..at + b;
                out = Some(match out {
                    Some(r) => r.start.min(piece.start)..r.end.max(piece.end),
                    None => piece,
                });
            }
        }
        at += len;
    }
    out
}

fn spans<'a>(
    drawn: &'a Drawn,
    heading: u8,
    header: bool,
    base: Font,
    links: &LinkStates,
    selected: Option<Range<usize>>,
) -> Vec<Span<'a, usize, Font>> {
    let mut at = 0;
    drawn
        .segs
        .iter()
        .enumerate()
        .flat_map(|(index, seg)| {
            // The selection splits a piece into up to three spans.
            let start = at;
            at += seg.text.len();
            let mut cuts = vec![0, seg.text.len()];
            if let Some(sel) = &selected {
                for edge in [sel.start, sel.end] {
                    if edge > start && edge < at {
                        cuts.push(edge - start);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            let pieces: Vec<(Range<usize>, bool)> = cuts
                .windows(2)
                .map(|w| {
                    let inside = selected
                        .as_ref()
                        .is_some_and(|sel| start + w[0] >= sel.start && start + w[1] <= sel.end);
                    (w[0]..w[1], inside)
                })
                .collect();
            let pieces = if pieces.is_empty() { vec![(0..0, false)] } else { pieces };
            pieces
                .into_iter()
                .map(move |(range, inside)| styled(drawn, seg, index, &seg.text[range], heading, header, base, links, inside))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn styled<'a>(
    drawn: &Drawn,
    seg: &live::Seg,
    index: usize,
    piece: &'a str,
    heading: u8,
    header: bool,
    base: Font,
    links: &LinkStates,
    selected: bool,
) -> Span<'a, usize, Font> {
    let s = seg.style;
    let mut font = if heading > 0 || header || s.has(Style::BOLD) {
        Font {
            weight: if s.has(Style::BOLD) { Weight::Bold } else { Weight::Semibold },
            ..base
        }
    } else {
        base
    };
    if s.has(Style::ITALIC) {
        font.style = FontStyle::Italic;
    }
    let mut color = if heading > 0 { theme::HEADING } else { theme::TEXT };
    let mut span = Span::new(piece);
    if s.has(Style::CODE) {
        font = theme::MONO_FONT;
        color = theme::CODE;
    }
    if s.has(Style::LINK) {
        color = match &seg.action {
            Some(live::Action::Wiki(dest)) => highlight::link_color(links.get(dest)),
            _ => theme::LINK,
        };
    }
    if s.has(Style::TAG) {
        color = theme::TAG;
    }
    if s.has(Style::MENTION) {
        // An `@name` without a crew card shows as a warning.
        color = match &seg.action {
            Some(live::Action::Mention(name)) if links.get(&format!("@{name}")) == obsession_core::links::LinkState::Missing => {
                theme::WARNING
            }
            _ => theme::MENTION,
        };
    }
    if s.has(Style::MARKER) || s.has(Style::FAINT) {
        color = theme::MUTED;
    }
    if s.has(Style::DONE) && !s.has(Style::CHIP) {
        color = theme::MUTED;
        span = span.strikethrough(true);
    }
    if s.has(Style::UNKNOWN) {
        color = theme::WARNING;
    }
    if s.has(Style::STRIKE) {
        span = span.strikethrough(true);
    }
    if s.has(Style::HIGHLIGHT) {
        span = span.background(Background::Color(theme::HIGHLIGHT_BG)).padding([0, 2]);
    }
    if s.has(Style::CODE) {
        span = span
            .background(Background::Color(theme::RAISED))
            .border(Border {
                radius: 3.0.into(),
                ..Border::default()
            })
            .padding([0, 3]);
    }
    if s.has(Style::CALLOUT) {
        color = callout_color(drawn.callout.as_deref());
    }
    if selected {
        span = span.background(Background::Color(theme::SELECTION)).padding(0);
    }
    let span = span.font(font).color(color);
    if seg.action.is_some() {
        span.link(index)
    } else {
        span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_lists() {
        let f = Format::builtin();
        assert_eq!(continuation("- item", &f).as_deref(), Some("- "));
        assert_eq!(continuation("  - [x] done", &f).as_deref(), Some("  - [ ] "));
        assert_eq!(continuation("9. nine", &f).as_deref(), Some("10. "));
        assert_eq!(continuation("> quoted", &f).as_deref(), Some("> "));
        assert_eq!(continuation("plain", &f), None);
    }

    #[test]
    fn up_and_down_keep_the_column_through_blank_lines() {
        let f = Format::builtin();
        let mut live = Live::new("abcdefghij\n\nklmnopqrst\nuv\nwxyzwxyzwx\n", &f);
        live.activate(0, 7);
        live.key(Intent::Down, &f);
        assert_eq!(live.cursor(), (1, 0));
        live.key(Intent::Down, &f);
        assert_eq!(live.cursor(), (2, 7));
        live.key(Intent::Down, &f);
        assert_eq!(live.cursor(), (3, 2));
        live.key(Intent::Down, &f);
        assert_eq!(live.cursor(), (4, 7));
        live.key(Intent::Up, &f);
        live.key(Intent::Up, &f);
        assert_eq!(live.cursor(), (2, 7));
        // A move to the side starts afresh from where the caret is.
        live.key(Intent::Left, &f);
        live.key(Intent::Down, &f);
        live.key(Intent::Down, &f);
        assert_eq!(live.cursor(), (4, 6));
    }

    #[test]
    fn reading_keeps_a_selection_and_copies_what_is_shown() {
        let f = Format::builtin();
        let mut live = Live::new("# Title\n\nSome **bold** text\n", &f);
        live.read_only = true;
        let press = |offset| Press {
            offset,
            link: None,
            right: false,
            window: Point::ORIGIN,
        };
        assert_eq!(live.press(0, None, press(0)), None);
        live.drag_to(2, None, 9);
        assert!(!live.release(), "Reading doesn't start editing");
        assert_eq!(live.active, None);
        assert_eq!(live.selection_text().as_deref(), Some("Title\n\nSome bold"));
        // A click (no drag) drops it.
        live.press(2, None, press(1));
        live.release();
        assert_eq!(live.selection_text(), None);
    }
}
