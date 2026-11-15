//! The graph view of a `_nodes.md`: note cards and the wires between them,
//! on a canvas you can pan (drag the background), zoom (wheel) and arrange
//! (drag a card). Positions are written back to the file's layout comment.
//!
//! Drawing is split so the costly part is cached: the grid, the cards and
//! the wires are drawn once into a cache that's only rebuilt when the view
//! moves or the graph changes. The card being dragged, its wires, hover and
//! selection go in a small overlay drawn every frame. Only what's on screen
//! is drawn, and zoomed out, cards are drawn as blocks without their tasks.

use std::cell::Cell;
use std::time::Instant;

use iced::keyboard;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Canvas, Event, Frame, Geometry, LineDash, Path, Stroke, Text};
use iced::{Color, Element, Fill, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, Vector};

use obsession_core::links::LinkIndex;
use obsession_core::nodes::{self, End, NodeKind};
use obsession_core::status::{self, Format};
use obsession_core::store::Store;
use obsession_core::vault::{Resolved, Vault};

use crate::theme;

pub const CARD_WIDTH: f32 = 260.0;
const HEADER: f32 = 52.0;
const ROW: f32 = 22.0;
const FOOTER: f32 = 30.0;
const MAX_ROWS: usize = 8;
const FUNCTION_SIZE: Size = Size::new(200.0, 44.0);
/// Below this zoom, cards are drawn as blocks with just a title.
const DETAIL_ZOOM: f32 = 0.5;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 2.5;
const GRID: f32 = 40.0;

#[derive(Debug, Clone)]
pub enum Message {
    /// A card was dragged to a new (graph) position.
    Moved(usize, Point),
    /// Double-click: open the card's note.
    Open(usize),
    /// A wire was dragged from one card's handle to another card.
    Connect(usize, usize),
    /// Delete was pressed with this selected.
    Delete(Selected),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selected {
    Card(usize),
    Wire(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Note,
    /// The link leads nowhere (double-click creates the note).
    Missing,
    /// Several notes match; the card shows the one Obsidian would open.
    Ambiguous,
    /// `fn Name`: a function node (its pins come in milestone 5).
    Function,
}

#[derive(Debug, Clone)]
pub struct TaskRow {
    pub symbol: String,
    pub text: String,
    pub done: bool,
    pub unknown: bool,
}

#[derive(Debug, Clone)]
pub struct Card {
    /// The node as `## Nodes` writes it (and the layout names it).
    pub key: String,
    /// The note it leads to.
    pub rel: Option<Box<str>>,
    pub title: String,
    pub folder: String,
    pub kind: CardKind,
    pub tasks: Vec<TaskRow>,
    pub done: usize,
    pub pos: Point,
    accent: Color,
}

impl Card {
    pub fn size(&self) -> Size {
        match self.kind {
            CardKind::Function => FUNCTION_SIZE,
            CardKind::Missing => Size::new(CARD_WIDTH, HEADER + FOOTER),
            _ => {
                let rows = self.tasks.len().min(MAX_ROWS) + usize::from(self.tasks.len() > MAX_ROWS);
                Size::new(CARD_WIDTH, HEADER + rows as f32 * ROW + FOOTER)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct WireView {
    pub from: usize,
    pub to: usize,
    /// From a `[[link]]` in a note, not from `## Wires`.
    pub link: bool,
    /// The function on it, e.g. `Wait For Done(...)`.
    pub label: Option<String>,
    /// Its line in `_nodes.md`.
    pub line: Option<usize>,
}

pub struct Graph {
    pub cards: Vec<Card>,
    pub wires: Vec<WireView>,
    pub show_links: bool,
    /// Changed when a different graph is opened, so the view fits it.
    pub fit: u64,
    cache: Cache,
    /// What the cache was drawn for.
    drawn: Cell<Option<DrawKey>>,
    version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawKey {
    pan: Vector,
    zoom: f32,
    size: Size,
    version: u64,
    lifted: Option<usize>,
    links: bool,
}

/// Screen <-> graph coordinates.
#[derive(Debug, Clone, Copy)]
struct View {
    pan: Vector,
    zoom: f32,
}

impl View {
    fn screen(&self, p: Point) -> Point {
        Point::new(p.x * self.zoom + self.pan.x, p.y * self.zoom + self.pan.y)
    }

    fn graph(&self, p: Point) -> Point {
        Point::new((p.x - self.pan.x) / self.zoom, (p.y - self.pan.y) / self.zoom)
    }
}

#[derive(Debug, Default)]
pub struct State {
    pan: Vector,
    /// 0 until the first event fits the graph to the view.
    zoom: f32,
    interaction: Interaction,
    hover: Option<Hover>,
    selected: Option<Selected>,
    last_click: Option<(usize, Instant)>,
    started: Option<Instant>,
    /// The graph the view was last fitted to.
    fitted: Option<u64>,
}

#[derive(Debug, Default, Clone, Copy)]
enum Interaction {
    #[default]
    Idle,
    Panning {
        from: Point,
        pan: Vector,
    },
    Dragging {
        card: usize,
        grab: Vector,
        at: Point,
        moved: bool,
    },
    Wiring {
        from: usize,
        to: Point,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hover {
    Card(usize),
    Handle(usize),
    Wire(usize),
}

impl Graph {
    /// Reads the graph in `text` (the file at `rel`) and every note it
    /// shows (through the store, so each is read from disk at most once).
    pub fn build(
        text: &str,
        rel: &str,
        vault: &Vault,
        store: &mut Store,
        team: Option<&Format>,
        links: &LinkIndex,
        show_links: bool,
    ) -> Self {
        let graph = nodes::parse(text);
        let mut cards: Vec<Card> = graph
            .nodes
            .iter()
            .map(|node| card(node, rel, vault, store, team))
            .collect();

        // Wires from `## Wires`.
        let find = |end: &End, cards: &[Card]| -> Option<usize> {
            match end {
                End::Note { target, .. } => {
                    let to = vault.resolve_from(target, Some(rel)).entry().map(|e| e.rel.clone());
                    cards.iter().position(|c| match (&c.rel, &to) {
                        (Some(a), Some(b)) => a == b,
                        _ => c.key.eq_ignore_ascii_case(target),
                    })
                }
                End::Pin { function, .. } => cards
                    .iter()
                    .position(|c| c.kind == CardKind::Function && c.title.eq_ignore_ascii_case(function)),
            }
        };
        let mut wires: Vec<WireView> = graph
            .wires
            .iter()
            .filter_map(|w| {
                Some(WireView {
                    from: find(&w.from, &cards)?,
                    to: find(&w.to, &cards)?,
                    link: false,
                    label: w.function.clone(),
                    line: Some(w.line),
                })
            })
            .collect();

        // `[[links]]` between two cards: from the linked note to the note
        // with the link.
        for (i, c) in cards.iter().enumerate() {
            let Some(source) = &c.rel else { continue };
            for target in links.targets(source) {
                let Some(to) = vault.resolve_from(target, Some(source)).entry() else {
                    continue;
                };
                let Some(j) = cards.iter().position(|d| d.rel.as_deref() == Some(&*to.rel)) else {
                    continue;
                };
                let known = wires.iter().any(|w| w.from == j && w.to == i);
                if j != i && !known {
                    wires.push(WireView {
                        from: j,
                        to: i,
                        link: true,
                        label: None,
                        line: None,
                    });
                }
            }
        }

        // Positions: from the layout, the rest placed automatically.
        let mut placed: Vec<bool> = Vec::with_capacity(cards.len());
        for (card, node) in cards.iter_mut().zip(&graph.nodes) {
            match graph.position(&node.key) {
                Some((x, y)) => {
                    card.pos = Point::new(x, y);
                    placed.push(true);
                }
                None => placed.push(false),
            }
        }
        auto_place(&mut cards, &placed, &wires);

        Self {
            cards,
            wires,
            show_links,
            fit: 0,
            cache: Cache::new(),
            drawn: Cell::new(None),
            version: 0,
        }
    }

    /// Something about the cards changed: redraw them.
    pub fn touch(&mut self) {
        self.version += 1;
    }

    /// Every card's position, for the layout comment.
    pub fn layout(&self) -> Vec<(&str, f32, f32)> {
        self.cards.iter().map(|c| (c.key.as_str(), c.pos.x, c.pos.y)).collect()
    }

    pub fn view(&self) -> Element<'_, Message> {
        Canvas::new(self).width(Fill).height(Fill).into()
    }

    fn visible_wires(&self) -> impl Iterator<Item = (usize, &WireView)> {
        self.wires
            .iter()
            .enumerate()
            .filter(|(_, w)| self.show_links || !w.link)
    }

    fn card_rect(&self, index: usize, state: &State) -> Rectangle {
        let card = &self.cards[index];
        let pos = match state.interaction {
            Interaction::Dragging { card: c, at, .. } if c == index => at,
            _ => card.pos,
        };
        Rectangle::new(pos, card.size())
    }

    /// The wire's ends in graph coordinates: out of the right side of the
    /// source card, into the left side of the target.
    fn wire_ends(&self, wire: &WireView, state: &State) -> (Point, Point) {
        let a = self.card_rect(wire.from, state);
        let b = self.card_rect(wire.to, state);
        (
            Point::new(a.x + a.width, a.y + a.height.min(HEADER + 40.0) / 2.0),
            Point::new(b.x, b.y + b.height.min(HEADER + 40.0) / 2.0),
        )
    }

    fn hit(&self, state: &State, view: View, p: Point) -> Option<Hover> {
        let g = view.graph(p);
        // Cards on top of wires; later cards on top of earlier ones.
        for i in (0..self.cards.len()).rev() {
            let r = self.card_rect(i, state);
            let handle = Point::new(r.x + r.width, r.y + r.height.min(HEADER + 40.0) / 2.0);
            if self.cards[i].kind != CardKind::Function && view.screen(handle).distance(p) <= 8.0 {
                return Some(Hover::Handle(i));
            }
            if r.contains(g) {
                return Some(Hover::Card(i));
            }
        }
        self.visible_wires()
            .find(|(_, w)| {
                let (a, b) = self.wire_ends(w, state);
                curve_points(a, b).windows(2).any(|s| {
                    distance_to_segment(p, view.screen(s[0]), view.screen(s[1])) <= 6.0
                })
            })
            .map(|(i, _)| Hover::Wire(i))
    }

    /// Fits every card in view (at most 100% zoom).
    fn fit(&self, state: &mut State, bounds: Size) {
        if self.cards.is_empty() {
            state.zoom = 1.0;
            state.pan = Vector::new(bounds.width / 2.0 - CARD_WIDTH / 2.0, 80.0);
            return;
        }
        let mut min = Point::new(f32::MAX, f32::MAX);
        let mut max = Point::new(f32::MIN, f32::MIN);
        for card in &self.cards {
            let size = card.size();
            min = Point::new(min.x.min(card.pos.x), min.y.min(card.pos.y));
            max = Point::new(max.x.max(card.pos.x + size.width), max.y.max(card.pos.y + size.height));
        }
        let margin = 60.0;
        let zoom = ((bounds.width - 2.0 * margin) / (max.x - min.x))
            .min((bounds.height - 2.0 * margin) / (max.y - min.y))
            .clamp(MIN_ZOOM, 1.0);
        let centre = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        state.zoom = zoom;
        state.pan = Vector::new(bounds.width / 2.0 - centre.x * zoom, bounds.height / 2.0 - centre.y * zoom);
    }
}

impl canvas::Program<Message> for Graph {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if state.fitted != Some(self.fit) {
            self.fit(state, bounds.size());
            state.fitted = Some(self.fit);
            state.started.get_or_insert_with(Instant::now);
            state.selected = None;
            state.hover = None;
            state.interaction = Interaction::Idle;
            return Some(canvas::Action::request_redraw());
        }
        let view = View {
            pan: state.pan,
            zoom: state.zoom,
        };
        let point = cursor.position_in(bounds);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let p = point?;
                if *button == mouse::Button::Middle {
                    state.interaction = Interaction::Panning { from: p, pan: state.pan };
                    return Some(canvas::Action::capture());
                }
                if *button != mouse::Button::Left {
                    return None;
                }
                let hit = self.hit(state, view, p);
                match hit {
                    Some(Hover::Handle(i)) => {
                        state.interaction = Interaction::Wiring { from: i, to: view.graph(p) };
                    }
                    Some(Hover::Card(i)) => {
                        state.selected = Some(Selected::Card(i));
                        let now = Instant::now();
                        let double = state
                            .last_click
                            .is_some_and(|(c, at)| c == i && now.duration_since(at).as_millis() < 450);
                        state.last_click = Some((i, now));
                        if double {
                            state.last_click = None;
                            return Some(canvas::Action::publish(Message::Open(i)).and_capture());
                        }
                        let pos = self.cards[i].pos;
                        state.interaction = Interaction::Dragging {
                            card: i,
                            grab: view.graph(p) - pos,
                            at: pos,
                            moved: false,
                        };
                    }
                    Some(Hover::Wire(i)) => state.selected = Some(Selected::Wire(i)),
                    None => {
                        state.selected = None;
                        state.interaction = Interaction::Panning { from: p, pan: state.pan };
                    }
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = cursor.position_in(bounds).or_else(|| {
                    // Keep dragging when the cursor leaves the canvas.
                    cursor.position().map(|c| Point::new(c.x - bounds.x, c.y - bounds.y))
                })?;
                match &mut state.interaction {
                    Interaction::Panning { from, pan } => {
                        state.pan = *pan + (p - *from);
                        Some(canvas::Action::request_redraw())
                    }
                    Interaction::Dragging { grab, at, moved, .. } => {
                        let to = view.graph(p) - *grab;
                        *moved |= to != *at;
                        *at = to;
                        Some(canvas::Action::request_redraw())
                    }
                    Interaction::Wiring { to, .. } => {
                        *to = view.graph(p);
                        Some(canvas::Action::request_redraw())
                    }
                    Interaction::Idle => {
                        let hover = point.and_then(|p| self.hit(state, view, p));
                        if hover != state.hover {
                            state.hover = hover;
                            return Some(canvas::Action::request_redraw());
                        }
                        None
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left | mouse::Button::Middle)) => {
                let done = std::mem::take(&mut state.interaction);
                match done {
                    Interaction::Dragging { card, at, moved: true, .. } => {
                        Some(canvas::Action::publish(Message::Moved(card, at)))
                    }
                    Interaction::Wiring { from, .. } => {
                        let target = point.and_then(|p| match self.hit(state, view, p) {
                            Some(Hover::Card(i) | Hover::Handle(i)) if i != from => Some(i),
                            _ => None,
                        });
                        Some(match target {
                            Some(to) => canvas::Action::publish(Message::Connect(from, to)),
                            None => canvas::Action::request_redraw(),
                        })
                    }
                    Interaction::Idle => None,
                    _ => Some(canvas::Action::request_redraw()),
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = point?;
                let steps = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
                };
                let zoom = (state.zoom * 1.12f32.powf(steps)).clamp(MIN_ZOOM, MAX_ZOOM);
                // Keep the point under the cursor where it is.
                let g = view.graph(p);
                state.zoom = zoom;
                state.pan = Vector::new(p.x - g.x * zoom, p.y - g.y * zoom);
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                use keyboard::key::Named;
                match (key.as_ref(), state.selected) {
                    (keyboard::Key::Named(Named::Delete | Named::Backspace), Some(selected)) => {
                        state.selected = None;
                        Some(canvas::Action::publish(Message::Delete(selected)))
                    }
                    (keyboard::Key::Named(Named::Escape), Some(_)) => {
                        state.selected = None;
                        Some(canvas::Action::request_redraw())
                    }
                    _ => None,
                }
            }
            Event::Window(iced::window::Event::RedrawRequested(_)) => {
                // Marching ants on the hovered or selected wire only.
                let animated = matches!(state.hover, Some(Hover::Wire(_)))
                    || matches!(state.selected, Some(Selected::Wire(_)));
                animated.then(|| canvas::Action::request_redraw_at(Instant::now() + std::time::Duration::from_millis(50)))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let zoom = if state.zoom == 0.0 { 1.0 } else { state.zoom };
        let view = View { pan: state.pan, zoom };
        let lifted = match state.interaction {
            Interaction::Dragging { card, .. } => Some(card),
            _ => None,
        };
        let key = DrawKey {
            pan: state.pan,
            zoom,
            size: bounds.size(),
            version: self.version,
            lifted,
            links: self.show_links,
        };
        if self.drawn.get() != Some(key) {
            self.cache.clear();
            self.drawn.set(Some(key));
        }
        let screen = Rectangle::new(Point::ORIGIN, bounds.size());

        let still = self.cache.draw(renderer, bounds.size(), |frame| {
            draw_grid(frame, view);
            for (_, wire) in self.visible_wires() {
                if lifted.is_some_and(|c| wire.from == c || wire.to == c) {
                    continue;
                }
                let (a, b) = self.wire_ends(wire, state);
                draw_wire(frame, view, a, b, wire, false, 0, screen);
            }
            for (i, card) in self.cards.iter().enumerate() {
                if Some(i) != lifted {
                    draw_card(frame, view, card, card.pos, screen);
                }
            }
        });

        // The overlay: what moves or reacts.
        let mut frame = Frame::new(renderer, bounds.size());
        let ants = state
            .started
            .map_or(0, |s| (s.elapsed().as_millis() / 50) as usize % 11);
        for (i, wire) in self.visible_wires() {
            let lit = state.hover == Some(Hover::Wire(i)) || state.selected == Some(Selected::Wire(i));
            let moving = lifted.is_some_and(|c| wire.from == c || wire.to == c);
            if lit || moving {
                let (a, b) = self.wire_ends(wire, state);
                draw_wire(&mut frame, view, a, b, wire, lit, if lit { 11 - ants } else { 0 }, screen);
            }
        }
        if let Some(i) = lifted {
            let r = self.card_rect(i, state);
            draw_card(&mut frame, view, &self.cards[i], r.position(), screen);
        }
        let outline = |frame: &mut Frame, i: usize, color: Color| {
            let r = self.card_rect(i, state);
            let p = view.screen(r.position());
            frame.stroke(
                &Path::rounded_rectangle(p, r.size() * zoom, (8.0 * zoom).into()),
                Stroke::default().with_color(color).with_width(1.5),
            );
        };
        if let Some(Selected::Card(i)) = state.selected {
            outline(&mut frame, i, theme::ACCENT);
        }
        if let Some(Hover::Card(i) | Hover::Handle(i)) = state.hover
            && self.cards[i].kind != CardKind::Function
        {
            let r = self.card_rect(i, state);
            let handle = view.screen(Point::new(r.x + r.width, r.y + r.height.min(HEADER + 40.0) / 2.0));
            let lit = state.hover == Some(Hover::Handle(i));
            frame.fill(
                &Path::circle(handle, if lit { 6.0 } else { 4.5 }),
                if lit { theme::ACCENT } else { theme::MUTED },
            );
        }
        if let Interaction::Wiring { from, to } = state.interaction {
            let r = self.card_rect(from, state);
            let a = Point::new(r.x + r.width, r.y + r.height.min(HEADER + 40.0) / 2.0);
            let segments = [6.0 * zoom, 5.0 * zoom];
            frame.stroke(
                &curve(view, a, to),
                Stroke {
                    line_dash: LineDash { segments: &segments, offset: 0 },
                    ..Stroke::default().with_color(theme::ACCENT).with_width(1.5)
                },
            );
        }
        vec![still, frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.interaction {
            Interaction::Panning { .. } | Interaction::Dragging { .. } => return mouse::Interaction::Grabbing,
            Interaction::Wiring { .. } => return mouse::Interaction::Crosshair,
            Interaction::Idle => {}
        }
        if !cursor.is_over(bounds) {
            return mouse::Interaction::None;
        }
        match state.hover {
            Some(Hover::Card(_)) => mouse::Interaction::Grab,
            Some(Hover::Handle(_)) => mouse::Interaction::Crosshair,
            Some(Hover::Wire(_)) => mouse::Interaction::Pointer,
            None => mouse::Interaction::None,
        }
    }
}

/// One node's card: the note's name, folder and tasks.
fn card(node: &nodes::Node, rel: &str, vault: &Vault, store: &mut Store, team: Option<&Format>) -> Card {
    let blank = |kind, title: &str, folder: &str| Card {
        key: node.key.clone(),
        rel: None,
        title: title.to_owned(),
        folder: folder.to_owned(),
        kind,
        tasks: Vec::new(),
        done: 0,
        pos: Point::ORIGIN,
        accent: accent(folder),
    };
    let target = match &node.kind {
        NodeKind::Function { name } => return blank(CardKind::Function, name, ""),
        NodeKind::File { target } => target,
    };
    let (entry, kind) = match vault.resolve_from(target, Some(rel)) {
        Resolved::One(entry) => (entry, CardKind::Note),
        Resolved::Ambiguous { picked, .. } => (picked, CardKind::Ambiguous),
        Resolved::NotFound | Resolved::Attachment => {
            let name = target.rsplit('/').next().unwrap_or(target);
            let folder = target.rsplit_once('/').map_or("", |(f, _)| f);
            return blank(CardKind::Missing, name, folder);
        }
    };
    let mut card = blank(kind, entry.title(), entry.parent());
    card.rel = Some(entry.rel.clone());
    let Ok(text) = store.get(&entry.rel) else {
        return card;
    };
    let lookup = status::from_frontmatter(text.lines());
    let (format, _) = status::resolve(lookup.as_ref(), team);
    let lines: Vec<&str> = text.lines().collect();
    for (line, at, ch) in status::tasks(text) {
        let raw = lines.get(line).copied().unwrap_or("");
        let after = raw.get(at + 2 + ch.len_utf8()..).unwrap_or("").trim();
        let after = match obsession_core::live::trailing_block_id(after) {
            Some(cut) => after[..cut].trim_end(),
            None => after,
        };
        let done = format.is_finished(ch);
        card.done += usize::from(done);
        card.tasks.push(TaskRow {
            symbol: format.get(ch).map_or_else(|| "⚠".to_owned(), |s| s.symbol.to_string()),
            text: after.to_owned(),
            done,
            unknown: format.index_of(ch).is_none(),
        });
    }
    card
}

/// A colour per top-level folder, so departments stand apart.
fn accent(folder: &str) -> Color {
    const PALETTE: [Color; 7] = [
        theme::ACCENT,
        theme::SUCCESS,
        theme::WARNING,
        theme::MENTION,
        theme::TAG,
        theme::CODE,
        theme::LINK,
    ];
    let top = folder.split('/').next().unwrap_or(folder).to_lowercase();
    if top.is_empty() {
        return theme::MUTED;
    }
    let hash = top.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    PALETTE[hash as usize % PALETTE.len()]
}

/// Places cards that have no saved position: in columns by how far down
/// the wires they are, below anything already placed.
fn auto_place(cards: &mut [Card], placed: &[bool], wires: &[WireView]) {
    if placed.iter().all(|&p| p) {
        return;
    }
    let n = cards.len();
    // Columns by longest path from a source (Kahn's order), following the
    // wires in `## Wires` only: link wires come and go as notes change (and
    // as the vault's links are read), and the layout shouldn't. A loop is
    // broken at the card with the fewest wires still coming in.
    let mut depth = vec![0usize; n];
    let mut incoming = vec![0usize; n];
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for w in wires.iter().filter(|w| !w.link) {
        if w.from != w.to && !next[w.from].contains(&w.to) {
            next[w.from].push(w.to);
            incoming[w.to] += 1;
        }
    }
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&i| incoming[i] == 0).collect();
    let mut done = vec![false; n];
    loop {
        while let Some(u) = queue.pop_front() {
            if std::mem::replace(&mut done[u], true) {
                continue;
            }
            for &v in &next[u] {
                if done[v] {
                    continue;
                }
                depth[v] = depth[v].max(depth[u] + 1);
                incoming[v] = incoming[v].saturating_sub(1);
                if incoming[v] == 0 {
                    queue.push_back(v);
                }
            }
        }
        match (0..n).filter(|&i| !done[i]).min_by_key(|&i| incoming[i]) {
            Some(i) => queue.push_back(i),
            None => break,
        }
    }
    let base_y = cards
        .iter()
        .zip(placed)
        .filter(|(_, p)| **p)
        .map(|(c, _)| c.pos.y + c.size().height)
        .fold(None, |m: Option<f32>, y| Some(m.map_or(y, |m| m.max(y))))
        .map_or(0.0, |y| y + 80.0);
    let base_x = cards
        .iter()
        .zip(placed)
        .filter(|(_, p)| **p)
        .map(|(c, _)| c.pos.x)
        .fold(None, |m: Option<f32>, x| Some(m.map_or(x, |m| m.min(x))))
        .unwrap_or(0.0);
    let mut column_y: Vec<f32> = Vec::new();
    for i in 0..n {
        if placed[i] {
            continue;
        }
        let d = depth[i];
        if column_y.len() <= d {
            column_y.resize(d + 1, base_y);
        }
        cards[i].pos = Point::new(base_x + d as f32 * (CARD_WIDTH + 90.0), column_y[d]);
        column_y[d] += cards[i].size().height + 40.0;
    }
}

fn draw_grid(frame: &mut Frame, view: View) {
    let size = frame.size();
    frame.fill_rectangle(Point::ORIGIN, size, theme::BG);
    let step = if view.zoom < 0.5 { GRID * 4.0 } else { GRID } * view.zoom;
    if step < 8.0 {
        return;
    }
    let start_x = view.pan.x.rem_euclid(step);
    let start_y = view.pan.y.rem_euclid(step);
    let dot = Color { a: 0.5, ..theme::LINE };
    let mut x = start_x;
    while x < size.width {
        let mut y = start_y;
        while y < size.height {
            frame.fill_rectangle(Point::new(x - 1.0, y - 1.0), Size::new(2.0, 2.0), dot);
            y += step;
        }
        x += step;
    }
}

/// Points along a wire's curve (graph coordinates).
fn curve_points(a: Point, b: Point) -> Vec<Point> {
    let dx = ((b.x - a.x).abs() / 2.0).max(60.0);
    let c1 = Point::new(a.x + dx, a.y);
    let c2 = Point::new(b.x - dx, b.y);
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let x = u * u * u * a.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * b.x;
            let y = u * u * u * a.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * b.y;
            Point::new(x, y)
        })
        .collect()
}

fn curve(view: View, a: Point, b: Point) -> Path {
    let dx = ((b.x - a.x).abs() / 2.0).max(60.0);
    Path::new(|p| {
        p.move_to(view.screen(a));
        p.bezier_curve_to(
            view.screen(Point::new(a.x + dx, a.y)),
            view.screen(Point::new(b.x - dx, b.y)),
            view.screen(b),
        );
    })
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let len = ab.x * ab.x + ab.y * ab.y;
    if len == 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + ab.x * t, a.y + ab.y * t))
}

#[allow(clippy::too_many_arguments)]
fn draw_wire(frame: &mut Frame, view: View, a: Point, b: Point, wire: &WireView, lit: bool, ants: usize, screen: Rectangle) {
    // Skip wires entirely off screen.
    let (sa, sb) = (view.screen(a), view.screen(b));
    let reach = ((sb.x - sa.x).abs() / 2.0).max(60.0 * view.zoom);
    let bbox = Rectangle::new(
        Point::new(sa.x.min(sb.x) - reach, sa.y.min(sb.y) - 10.0),
        Size::new((sa.x - sb.x).abs() + 2.0 * reach, (sa.y - sb.y).abs() + 20.0),
    );
    if !bbox.intersects(&screen) {
        return;
    }
    let color = if lit {
        theme::ACCENT
    } else if wire.link {
        Color { a: 0.55, ..theme::MUTED }
    } else {
        theme::MUTED
    };
    let z = view.zoom.max(0.4);
    let segments = [6.0 * z, 5.0 * z];
    frame.stroke(
        &curve(view, a, b),
        Stroke {
            line_dash: LineDash { segments: &segments, offset: ants },
            ..Stroke::default().with_color(color).with_width(if lit { 2.2 } else { 1.5 })
        },
    );
    // Arrowhead.
    let s = 7.0 * z;
    frame.fill(
        &Path::new(|p| {
            p.move_to(sb);
            p.line_to(Point::new(sb.x - s, sb.y - s * 0.6));
            p.line_to(Point::new(sb.x - s, sb.y + s * 0.6));
            p.close();
        }),
        color,
    );
    if let Some(label) = &wire.label
        && view.zoom >= DETAIL_ZOOM
    {
        let mid = curve_points(a, b)[12];
        let name = label.split('(').next().unwrap_or(label).trim();
        frame.fill_text(Text {
            content: format!("ƒ {name}"),
            position: view.screen(Point::new(mid.x, mid.y - 16.0)),
            color: if lit { theme::ACCENT } else { theme::MUTED },
            size: Pixels(11.0 * view.zoom),
            font: theme::UI_FONT,
            align_x: iced::widget::text::Alignment::Center,
            ..Text::default()
        });
    }
}

/// Cuts `text` to about `width` pixels at `size`, with an ellipsis.
fn fit(text: &str, width: f32, size: f32) -> String {
    let max = (width / (size * 0.52)).max(1.0) as usize;
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn label(frame: &mut Frame, text: String, at: Point, size: f32, color: Color, font: Font) {
    frame.fill_text(Text {
        content: text,
        position: at,
        color,
        size: Pixels(size),
        font,
        ..Text::default()
    });
}

fn draw_card(frame: &mut Frame, view: View, card: &Card, pos: Point, screen: Rectangle) {
    let z = view.zoom;
    let size = card.size();
    let p = view.screen(pos);
    let rect = Rectangle::new(p, size * z);
    if !rect.intersects(&screen) {
        return;
    }
    let radius = 8.0 * z;
    let body = Path::rounded_rectangle(p, rect.size(), radius.into());
    let semibold = Font {
        weight: iced::font::Weight::Semibold,
        ..theme::UI_FONT
    };

    if card.kind == CardKind::Function {
        frame.fill(&body, theme::RAISED);
        frame.stroke(&body, Stroke::default().with_color(theme::MENTION).with_width(1.2));
        label(
            frame,
            fit(&format!("ƒ  {}", card.title), size.width - 24.0, 14.0),
            Point::new(p.x + 12.0 * z, p.y + 13.0 * z),
            14.0 * z,
            theme::MENTION,
            semibold,
        );
        return;
    }

    frame.fill(&body, theme::RAISED);
    let border = match card.kind {
        CardKind::Ambiguous => theme::WARNING,
        _ => theme::LINE,
    };
    if card.kind == CardKind::Missing {
        let segments = [5.0 * z, 4.0 * z];
        frame.stroke(
            &body,
            Stroke {
                line_dash: LineDash { segments: &segments, offset: 0 },
                ..Stroke::default().with_color(theme::FAINT).with_width(1.2)
            },
        );
    } else {
        frame.stroke(&body, Stroke::default().with_color(border).with_width(1.0));
        // The folder's colour along the top.
        frame.fill(
            &Path::rounded_rectangle(p, Size::new(rect.width, 4.0 * z), radius.into()),
            card.accent,
        );
    }

    let pad = 12.0 * z;
    let inner = size.width - 24.0;
    let title_color = if card.kind == CardKind::Missing { theme::LINK_MISSING } else { theme::HEADING };

    // Zoomed out: a block with the title.
    if z < DETAIL_ZOOM {
        label(
            frame,
            fit(&card.title, inner, 15.0),
            Point::new(p.x + pad, p.y + 14.0 * z),
            15.0 * z,
            title_color,
            semibold,
        );
        return;
    }

    label(frame, fit(&card.title, inner, 15.0), Point::new(p.x + pad, p.y + 11.0 * z), 15.0 * z, title_color, semibold);
    let subtitle = match card.kind {
        CardKind::Missing => "No such note yet · double-click to create".to_owned(),
        CardKind::Ambiguous => format!("⚠ several notes match · {}", card.folder),
        _ => card.folder.clone(),
    };
    let subtitle_color = if card.kind == CardKind::Ambiguous { theme::WARNING } else { theme::FAINT };
    label(frame, fit(&subtitle, inner, 11.0), Point::new(p.x + pad, p.y + 31.0 * z), 11.0 * z, subtitle_color, theme::UI_FONT);

    let mut y = HEADER;
    for task in card.tasks.iter().take(MAX_ROWS) {
        let row = Point::new(p.x + pad, p.y + y * z);
        label(frame, task.symbol.clone(), row, 12.0 * z, theme::TEXT, theme::UI_FONT);
        let color = if task.unknown {
            theme::WARNING
        } else if task.done {
            theme::MUTED
        } else {
            theme::TEXT
        };
        let text = fit(&task.text, inner - 24.0, 13.0);
        let width = text.chars().count() as f32 * 13.0 * 0.46 * z;
        label(frame, text, Point::new(row.x + 22.0 * z, row.y), 13.0 * z, color, theme::UI_FONT);
        if task.done {
            frame.fill_rectangle(
                Point::new(row.x + 22.0 * z, row.y + 8.5 * z),
                Size::new(width.min((inner - 24.0) * z), 1.0),
                theme::MUTED,
            );
        }
        y += ROW;
    }
    if card.tasks.len() > MAX_ROWS {
        label(
            frame,
            format!("+{} more", card.tasks.len() - MAX_ROWS),
            Point::new(p.x + pad + 22.0 * z, p.y + y * z),
            12.0 * z,
            theme::FAINT,
            theme::UI_FONT,
        );
    }

    if card.kind != CardKind::Missing {
        let total = card.tasks.len();
        let foot = p.y + (size.height - FOOTER + 8.0) * z;
        let text = if total == 0 {
            "No tasks".to_owned()
        } else {
            format!("{} / {total} done", card.done)
        };
        label(frame, text, Point::new(p.x + pad, foot), 11.0 * z, theme::MUTED, theme::UI_FONT);
        if total > 0 {
            let bar_x = p.x + 110.0 * z;
            let bar_w = (size.width - 110.0 - 12.0) * z;
            let bar_y = foot + 6.0 * z;
            frame.fill(
                &Path::rounded_rectangle(Point::new(bar_x, bar_y), Size::new(bar_w, 4.0 * z), (2.0 * z).into()),
                theme::LINE,
            );
            let share = card.done as f32 / total as f32;
            if share > 0.0 {
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(bar_x, bar_y),
                        Size::new(bar_w * share, 4.0 * z),
                        (2.0 * z).into(),
                    ),
                    theme::SUCCESS,
                );
            }
        }
    }
}
