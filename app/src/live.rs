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
//! on the clicked character), Escape to stop editing.

use std::cell::Cell;
use std::ops::Range;

use iced::advanced::text::Span;
use iced::font::{Style as FontStyle, Weight};
use iced::keyboard::{Key, key::Named};
use iced::widget::text::LineHeight;
use iced::widget::text_editor::{self, Action, Binding, Cursor, KeyPress, Position};
use iced::widget::{Id, Space, column, container, mouse_area, row, rule, scrollable, text};
use iced::{Background, Border, Element, Fill, Font, Length, Point};

use obsession_core::doc::Doc;
use obsession_core::live::{self, Block, BlockKind, Line as Drawn, Style};
use obsession_core::status::Format;
use obsession_core::syntax::LineState;

use crate::highlight::{self, Statuses};
use crate::live_text::{LiveText, Press};
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
/// Text inset per `>` level of a quote.
const QUOTE_INDENT: f32 = 16.0;

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
    /// Scroll offset and visible height of the page.
    scroll: (f32, f32),
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
            scroll: (0.0, 1600.0),
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
    }

    pub fn scrolled(&mut self, viewport: scrollable::Viewport) {
        self.scroll = (viewport.absolute_offset().y, viewport.bounds().height);
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
            .splice(range, std::iter::repeat_with(|| Cell::new(0.0)).take(count));

        self.blocks = live::blocks(self.doc.lines());
        let roles = roles(&self.blocks, self.doc.len(), self.doc.lines());
        for (i, role) in roles.iter().enumerate() {
            let touched = (start..start + count).contains(&i);
            if touched || self.roles[i] != *role {
                self.rendered[i] = render(self.doc.line(i), *role, format);
                self.heights[i].set(0.0);
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
        self.content.perform(action);
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
        match intent {
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
                    return false;
                }
                match intent {
                    Intent::Up | Intent::Left if range.start > 0 => {
                        let target = range.start - 1;
                        let column = if intent == Intent::Left {
                            self.doc.line(target).len()
                        } else {
                            column
                        };
                        self.activate(target, column);
                    }
                    Intent::Down | Intent::Right if range.end < self.doc.len() => {
                        let column = if intent == Intent::Right { 0 } else { column };
                        self.activate(range.end, column);
                    }
                    _ => {}
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
        let raw_len = self.doc.line(line).len();
        let column = drawn.map_or(raw_len, |d| d.raw_offset(press.offset, raw_len));
        self.activate(line, column);
        None
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
        self.activate(cursor.0, cursor.1);
    }

    /// Clicks the active editor at a point (for a right-click), returning
    /// the doc line the caret landed on.
    pub fn click_editor(&mut self, point: Point) -> usize {
        self.content.perform(Action::Click(point));
        self.cursor().0
    }

    pub fn view<'a>(&'a self, statuses: &Statuses) -> Element<'a, Message> {
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
                    items.push(self.line_element(i));
                }
                if !editor_shown {
                    editor_shown = true;
                    items.push(self.editor_element(statuses));
                }
                for i in active.end.min(block.lines.end).max(block.lines.start)..block.lines.end {
                    if i >= active.end {
                        items.push(self.line_element(i));
                    }
                }
                continue;
            }
            items.push(self.block_element(block));
        }
        if gap > 0.0 {
            items.push(Space::new().height(gap - SPACING).into());
        }

        // Only the empty space below the last line starts editing at the
        // end; margins and the gaps between lines do nothing.
        let below = mouse_area(Space::new().width(Fill).height(Length::Fixed(240.0)))
            .on_press(Message::Background)
            .interaction(iced::mouse::Interaction::Text);
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

    /// A line's height: measured if it has been drawn, estimated if not.
    fn line_height(&self, line: usize) -> f32 {
        let measured = self.heights.get(line).map_or(0.0, Cell::get);
        let (text, extra) = match &self.rendered[line] {
            Rendered::Hidden => return 0.0,
            Rendered::Cells(_) => return LINE_PX + 10.0,
            Rendered::Line(drawn) if drawn.rule => return 25.0,
            Rendered::Line(drawn) => {
                let top = if drawn.heading > 0 { 10.0 } else { 0.0 };
                (drawn, top)
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
        let size = heading_size(text.heading);
        let row = size * 1.55;
        // About 0.5 em per character, on a page about 800 px wide.
        let chars = text.segs.iter().map(|s| s.text.chars().count()).sum::<usize>() as f32;
        let rows = ((chars * size * 0.5) / 800.0).ceil().max(1.0);
        rows * row + extra
    }

    fn block_height(&self, block: &Block) -> f32 {
        let lines: f32 = block.lines.clone().map(|i| self.line_height(i)).sum();
        match block.kind {
            BlockKind::Line | BlockKind::Comment | BlockKind::Table => lines,
            BlockKind::Fence => lines + 18.0,
            BlockKind::Frontmatter => lines + 36.0,
        }
    }

    fn block_element<'a>(&'a self, block: &Block) -> Element<'a, Message> {
        let lines = block.lines.clone();
        match block.kind {
            BlockKind::Line => self.line_element(lines.start),
            BlockKind::Fence => container(column(lines.map(|i| self.line_element(i))))
                .padding([8, 14])
                .width(Fill)
                .style(theme::boxed(theme::PANEL))
                .into(),
            BlockKind::Frontmatter => container(
                column![text("Properties").size(theme::SMALL).color(theme::FAINT)]
                    .extend(lines.map(|i| self.line_element(i))),
            )
            .padding([8, 14])
            .width(Fill)
            .style(theme::boxed(theme::PANEL))
            .into(),
            BlockKind::Table => column(lines.map(|i| self.line_element(i))).into(),
            BlockKind::Comment => column(lines.map(|i| self.line_element(i))).into(),
        }
    }

    fn line_element(&self, line: usize) -> Element<'_, Message> {
        match &self.rendered[line] {
            Rendered::Hidden => Space::new().height(0).into(),
            Rendered::Cells(cells) => {
                let header = self.roles[line] == Role::TableHeader;
                row(cells.iter().enumerate().map(|(c, drawn)| {
                    container(
                        LiveText::new(spans(drawn, 0, header, theme::UI_FONT), move |press| Message::Press {
                            line,
                            cell: Some(c),
                            press,
                        })
                        .size(TEXT_SIZE - 1.0)
                        .line_height(LINE_HEIGHT)
                        .font(theme::UI_FONT)
                        .color(theme::TEXT),
                    )
                    .padding([4, 10])
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
                let mut widget = LiveText::new(spans(drawn, drawn.heading, false, base), move |press| {
                    Message::Press {
                        line,
                        cell: None,
                        press,
                    }
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
                let mut element: Element<'_, Message> = container(widget)
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

    fn editor_element<'a>(&'a self, statuses: &Statuses) -> Element<'a, Message> {
        let range = self.active.clone().unwrap_or(0..1);
        let kind = self
            .block_of(range.start)
            .filter(|b| b.lines == range)
            .map_or(BlockKind::Line, |b| b.kind);
        let single = kind == BlockKind::Line && range.len() == 1;
        let first_line = self.doc.line(range.start);
        let heading = heading_level(first_line);
        let mono = !single;

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

fn heading_level(line: &str) -> u8 {
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

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Turns a drawn line into styled spans. Spans with an action carry their
/// piece index as the link, so presses can tell what was hit.
fn spans(drawn: &Drawn, heading: u8, header: bool, base: Font) -> Vec<Span<'_, usize, Font>> {
    drawn
        .segs
        .iter()
        .enumerate()
        .map(|(index, seg)| {
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
            let mut span = Span::new(seg.text.as_str());
            if s.has(Style::CODE) {
                font = theme::MONO_FONT;
                color = theme::CODE;
            }
            if s.has(Style::LINK) {
                color = theme::LINK;
            }
            if s.has(Style::TAG) {
                color = theme::TAG;
            }
            if s.has(Style::MENTION) {
                color = theme::MENTION;
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
            let span = span.font(font).color(color);
            if seg.action.is_some() {
                span.link(index)
            } else {
                span
            }
        })
        .collect()
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
}
