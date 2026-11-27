//! The graph view of a `_nodes.md`: note cards and the wires between them,
//! on a canvas you can pan (drag the background), zoom (wheel) and arrange
//! (drag a card). Positions are written back to the file's layout comment.
//!
//! Drawing is split so the costly part is cached: the grid, the cards and
//! the wires are drawn once into a cache that's only rebuilt when the view
//! moves or the graph changes. The card being dragged, its wires, hover and
//! selection go in a small overlay drawn every frame. Only what's on screen
//! is drawn, and zoomed out, cards are drawn as blocks without their tasks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Instant;

use iced::keyboard;
use iced::mouse;
use iced::advanced::graphics::cache::{Cached, Group};
use iced::widget::canvas::{self, Cache, Canvas, Event, Frame, Geometry, LineDash, Path, Stroke, Text};
use iced::widget::image::Handle;
use iced::{Color, Element, Fill, Font, Pixels, Point, Radians, Rectangle, Renderer, Size, Theme, Vector};

use obsession_core::crew::{self, Badge, Member};
use obsession_core::func::{self, Library, PortKind};
use obsession_core::links::{self, LinkIndex};
use obsession_core::nodes::{self, End, NodeKind};
use obsession_core::views::View as SavedView;
use obsession_core::status::{self, Format};
use obsession_core::store::Store;
use obsession_core::vault::{Resolved, Vault};

use crate::smooth::{self, Shapes};
use crate::theme;

pub const CARD_WIDTH: f32 = 260.0;
const HEADER: f32 = 52.0;
const ROW: f32 = 22.0;
const FOOTER: f32 = 30.0;
const MAX_ROWS: usize = 8;
const FUNCTION_WIDTH: f32 = 220.0;
const PIN_ROW: f32 = 22.0;
/// Below this zoom, cards are drawn as blocks with just a title.
const DETAIL_ZOOM: f32 = 0.5;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 2.5;
const GRID: f32 = 40.0;
/// The card hand: card size, and how much of a resting card shows.
const HAND_W: f32 = 150.0;
const HAND_H: f32 = 132.0;
const HAND_PEEK: f32 = 74.0;
/// How long a crew card takes to rise.
const RISE_MS: f32 = 140.0;
/// How long a wire glows after its function ran.
const PULSE_MS: f32 = 1200.0;

/// The glow's length (debug builds can slow it down to look at it:
/// `OBSESSION_DEV_PULSE_MS`).
fn pulse_ms() -> f32 {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("OBSESSION_DEV_PULSE_MS").ok().and_then(|v| v.parse().ok()) {
        return ms;
    }
    PULSE_MS
}
/// Dev: `OBSESSION_DEV_PAN=frames` pans the graph by itself, a few pixels a
/// frame, starting 2 s after it opens, then prints the frame intervals and
/// how long the cached layer took to build (stderr).
#[cfg(debug_assertions)]
mod pan_bench {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    use super::State;

    #[derive(Default)]
    struct Run {
        last: Option<Instant>,
        intervals: Vec<f32>,
        builds: Vec<f32>,
        done: bool,
    }

    thread_local! {
        static RUN: RefCell<Run> = RefCell::new(Run::default());
    }

    fn frames() -> Option<usize> {
        static FRAMES: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        *FRAMES.get_or_init(|| std::env::var("OBSESSION_DEV_PAN").ok().and_then(|v| v.parse().ok()))
    }

    pub fn step(state: &mut State) -> Option<iced::widget::canvas::Action<super::Message>> {
        let frames = frames()?;
        if state.started?.elapsed() < Duration::from_secs(2) {
            return Some(iced::widget::canvas::Action::request_redraw_at(Instant::now() + Duration::from_millis(100)));
        }
        RUN.with_borrow_mut(|run| {
            if run.done {
                return None;
            }
            let now = Instant::now();
            if let Some(last) = run.last {
                run.intervals.push((now - last).as_secs_f32() * 1000.0);
            }
            run.last = Some(now);
            let n = run.intervals.len();
            if n >= frames {
                run.done = true;
                let stats = |v: &mut Vec<f32>| {
                    v.sort_by(f32::total_cmp);
                    let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
                    let at = |q: f32| v.get(((v.len() as f32 - 1.0) * q) as usize).copied().unwrap_or(0.0);
                    format!("mean {mean:.2}, p50 {:.2}, p95 {:.2}, max {:.2}", at(0.5), at(0.95), at(1.0))
                };
                eprintln!("TIMING pan frames ({n}): {} ms", stats(&mut run.intervals));
                eprintln!("TIMING pan builds ({}): {} ms", run.builds.len(), stats(&mut run.builds));
                return None;
            }
            // Back and forth, so the graph stays in view.
            let dir = if (n / 150) % 2 == 0 { 1.0 } else { -1.0 };
            state.pan.x += 4.0 * dir;
            state.pan.y += 1.5 * dir;
            Some(iced::widget::canvas::Action::request_redraw())
        })
    }

    pub fn drawn(took: Duration) {
        if frames().is_none() || took < Duration::from_micros(50) {
            return;
        }
        RUN.with_borrow_mut(|run| {
            if run.last.is_some() && !run.done {
                run.builds.push(took.as_secs_f32() * 1000.0);
            }
        });
    }
}

/// How long a wire takes to go from dashed to solid.
const SOLID_MS: f32 = 450.0;

#[derive(Debug, Clone)]
pub enum Message {
    /// A card was dragged to a new (graph) position.
    Moved(usize, Point),
    /// Double-click: open the card's note.
    Open(usize),
    /// A wire was dragged from one card (or a function's output pin) to
    /// another card (or a function's input pin).
    Connect(PinRef, PinRef),
    /// Delete was pressed with this selected.
    Delete(Selected),
    /// The ƒ button on a wire was clicked.
    Function(usize),
    /// A crew card was dropped on a node card (and on one of its task rows,
    /// or on the card as a whole).
    Assign {
        member: usize,
        card: usize,
        task: Option<usize>,
    },
    /// Double-click on a crew card: open it.
    OpenCrew(usize),
    /// The view was panned or zoomed: where it is now, to remember.
    Viewed(SavedView),
}

/// One crew card in the hand.
#[derive(Debug, Clone)]
pub struct HandCard {
    pub key: String,
    pub name: String,
    pub role: String,
    pub initials: String,
    pub color: Color,
    pub bot: bool,
    /// Node cards with a task that mentions them.
    pub nodes: usize,
    /// Their open tasks in this graph.
    pub open: u32,
    pub badge: Option<Badge>,
    /// Their portrait (`portrait:` on the card, resolved in the vault).
    pub portrait: Option<Box<str>>,
}

/// A card, and on a function card, one of its pins (an output pin at the
/// start of a wire, an input pin at the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinRef {
    pub card: usize,
    pub pin: Option<usize>,
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
    /// `@names` on the task (lower case).
    pub people: Vec<String>,
    /// Its line in the note.
    pub line: usize,
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
    /// A function card's pins: its `in`s and `out`s.
    pub pins_in: Vec<(String, PortKind)>,
    pub pins_out: Vec<(String, PortKind)>,
    /// Why a function card isn't ready (shown with ⚠).
    pub problem: Option<String>,
    /// Its line in the graph file.
    pub line: Option<usize>,
    /// A function node that would change something if it ran now.
    pub stale: Option<String>,
    accent: Color,
}

impl Card {
    /// Where pin `k` sits (graph coordinates): inputs on the left edge,
    /// outputs on the right.
    fn pin_at(&self, pos: Point, output: bool, k: usize) -> Point {
        let x = if output { pos.x + FUNCTION_WIDTH } else { pos.x };
        Point::new(x, pos.y + HEADER + k as f32 * PIN_ROW + PIN_ROW / 2.0)
    }

    pub fn size(&self) -> Size {
        match self.kind {
            CardKind::Function => {
                let rows = self.pins_in.len().max(self.pins_out.len());
                Size::new(FUNCTION_WIDTH, HEADER + rows as f32 * PIN_ROW + 10.0)
            }
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
    /// The function card's output pin it starts at / input pin it ends at.
    pub from_pin: Option<usize>,
    pub to_pin: Option<usize>,
    /// The task row it leaves from / ends at, for a `[[a#^t2]]` or
    /// `[[a#Notes]]` end (only rows the card shows).
    pub from_row: Option<usize>,
    pub to_row: Option<usize>,
    pub state: WireState,
    /// When it turned solid (for the dashed-to-solid animation).
    pub solid_since: Option<Instant>,
    /// Its function would change something if it ran now: what.
    pub stale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireState {
    /// No function: a relation only.
    Plain,
    /// Its function checks out.
    Solid,
    /// Dashed with ⚠: why it isn't ready.
    Broken(String),
}

pub struct Graph {
    pub cards: Vec<Card>,
    pub wires: Vec<WireView>,
    pub show_links: bool,
    /// The crew, as a hand of cards along the bottom.
    pub hand: Vec<HandCard>,
    pub show_hand: bool,
    /// Wires (and function nodes) whose function just ran, by line in the
    /// graph file: they glow for a moment.
    pub pulses: Vec<(usize, Instant)>,
    /// Changed when a different graph is opened, so the view fits it.
    pub fit: u64,
    /// Where this graph was last looked at: used instead of fitting it.
    pub saved_view: Option<SavedView>,
    /// The cached layer: the grid and the wires' words, the wires (smooth
    /// meshes, `wire_mesh`), then the cards.
    cache: Cache,
    wire_mesh: RefCell<Option<WireMesh>>,
    cards_cache: Cache,
    /// The hand as the graph canvas last drew it, for `HandLayer`.
    hand_now: Cell<Option<HandNow>>,
    /// Decoded crew portraits, by file (the app keeps them up to date).
    faces: HashMap<Box<str>, Handle>,
    /// Crew card notes (lower case) and their portrait files, for the faces
    /// on node cards.
    card_faces: HashMap<String, Box<str>>,
    /// What the cache was drawn for.
    drawn: Cell<Option<DrawKey>>,
    version: u64,
}

/// The wires' mesh as last built, kept like a canvas cache.
type WireMesh = <iced_wgpu::Geometry as Cached>::Cache;

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawKey {
    pan: Vector,
    zoom: f32,
    /// The screen's scale factor (the soft edges are a screen pixel wide).
    scale: f32,
    size: Size,
    version: u64,
    lifted: Option<usize>,
    links: bool,
    /// How many wires are turning solid (drawn in the overlay meanwhile).
    growing: usize,
}

/// Screen <-> graph coordinates.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub pan: Vector,
    pub zoom: f32,
}

impl View {
    pub fn screen(&self, p: Point) -> Point {
        Point::new(p.x * self.zoom + self.pan.x, p.y * self.zoom + self.pan.y)
    }

    pub fn graph(&self, p: Point) -> Point {
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
    /// The canvas size, for the hand.
    size: Size,
    /// The crew card rising, and since when.
    rise: Option<(usize, Instant)>,
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
        pin: Option<usize>,
        to: Point,
    },
    /// A crew card being dragged (screen point).
    Dealing {
        member: usize,
        at: Point,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hover {
    Card(usize),
    Handle(usize),
    /// A function card's pin: (card, output, index).
    Pin(usize, bool, usize),
    Wire(usize),
    /// The ƒ button in the middle of a hovered wire.
    Button(usize),
    /// A crew card in the hand.
    Hand(usize),
}

impl Graph {
    /// Reads the graph in `text` (the file at `rel`) and every note it
    /// shows (through the store, so each is read from disk at most once).
    /// `previous` is the graph this one replaces, so wires that just became
    /// solid can animate.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        text: &str,
        rel: &str,
        vault: &Vault,
        store: &mut Store,
        team: Option<&Format>,
        links: &LinkIndex,
        library: &Library,
        previous: Option<&Graph>,
        show_links: bool,
    ) -> Self {
        let graph = nodes::parse(text);
        let mut cards: Vec<Card> = graph
            .nodes
            .iter()
            .map(|node| card(node, rel, vault, store, team, library))
            .collect();

        // Wires from `## Wires`.
        let find = |end: &End, cards: &[Card]| -> Option<usize> {
            match end {
                End::Note { target, .. } => {
                    let to = vault.resolve_from(target, Some(rel)).entry().map(|e| e.rel.clone());
                    cards.iter().position(|c| match (&c.rel, &to) {
                        (Some(a), Some(b)) => a == b,
                        _ => c.kind != CardKind::Function && c.key.eq_ignore_ascii_case(target),
                    })
                }
                End::Pin { function, .. } => cards
                    .iter()
                    .position(|c| c.kind == CardKind::Function && c.title.eq_ignore_ascii_case(function)),
            }
        };
        let pin_of = |end: &End, pins: &[(String, PortKind)]| match end {
            End::Pin { pin, .. } => pins.iter().position(|(n, _)| n.eq_ignore_ascii_case(pin)),
            End::Note { .. } => None,
        };
        // The task row a `#^block` / `#heading` end points at, if the card
        // shows it.
        let mut row_of = |end: &End, card: &Card| -> Option<usize> {
            let End::Note { sub: Some(sub), .. } = end else { return None };
            row_in(store, card, sub)
        };
        let mut wires: Vec<WireView> = Vec::new();
        for w in &graph.wires {
            let (Some(from), Some(to)) = (find(&w.from, &cards), find(&w.to, &cards)) else {
                continue;
            };
            let from_pin = pin_of(&w.from, &cards[from].pins_out);
            let to_pin = pin_of(&w.to, &cards[to].pins_in);
            let state = wire_state(w, &cards[from], &cards[to], from_pin, to_pin, library);
            let from_row = row_of(&w.from, &cards[from]);
            let to_row = row_of(&w.to, &cards[to]);
            wires.push(WireView {
                from,
                to,
                link: false,
                label: w.function.clone(),
                line: Some(w.line),
                from_pin,
                to_pin,
                from_row,
                to_row,
                state,
                solid_since: None,
                stale: None,
            });
        }

        // A function card is ready once every pin is wired; until then its
        // wires are dashed too.
        for (i, c) in cards.iter_mut().enumerate() {
            if c.kind != CardKind::Function || c.problem.is_some() {
                continue;
            }
            let unwired = c
                .pins_in
                .iter()
                .enumerate()
                .find(|(k, _)| !wires.iter().any(|w| w.to == i && w.to_pin == Some(*k)))
                .map(|(_, (n, _))| n.clone())
                .or_else(|| {
                    c.pins_out
                        .iter()
                        .enumerate()
                        .find(|(k, _)| !wires.iter().any(|w| w.from == i && w.from_pin == Some(*k)))
                        .map(|(_, (n, _))| n.clone())
                });
            if let Some(pin) = unwired {
                c.problem = Some(format!("`{pin}` isn't wired yet"));
            }
        }
        for w in &mut wires {
            for c in [w.from, w.to] {
                if w.state == WireState::Solid
                    && let Some(p) = &cards[c].problem
                {
                    w.state = WireState::Broken(p.clone());
                }
            }
        }

        // `[[links]]` between two cards: from the linked note to the note
        // with the link.
        for (i, c) in cards.iter().enumerate() {
            let Some(source) = &c.rel else { continue };
            // The links as written, for which row of the linked card each
            // one points at.
            let written = store.get(source).map(links::links).unwrap_or_default();
            for target in links.targets(source) {
                let Some(to) = vault.resolve_from(target, Some(source)).entry() else {
                    continue;
                };
                let Some(j) = cards.iter().position(|d| d.rel.as_deref() == Some(&*to.rel)) else {
                    continue;
                };
                let known = wires.iter().any(|w| w.from == j && w.to == i);
                if j != i && !known {
                    // Every link to that note must point at the same row,
                    // or the wire leaves from the card.
                    let mut rows = written
                        .iter()
                        .filter(|l| vault.resolve_from(&l.target, Some(source)).entry().is_some_and(|e| e.rel == to.rel))
                        .map(|l| l.sub.as_deref().and_then(|sub| row_in(store, &cards[j], sub)));
                    let first = rows.next().flatten();
                    let from_row = first.filter(|_| rows.all(|r| r == first));
                    wires.push(WireView {
                        from: j,
                        to: i,
                        link: true,
                        label: None,
                        line: None,
                        from_pin: None,
                        to_pin: None,
                        from_row,
                        to_row: None,
                        state: WireState::Plain,
                        solid_since: None,
                        stale: None,
                    });
                }
            }
        }

        // Wires that just became solid animate from dashed.
        if let Some(previous) = previous {
            let now = Instant::now();
            for w in wires.iter_mut().filter(|w| w.state == WireState::Solid) {
                let before = previous.wires.iter().find(|p| {
                    p.line == w.line
                        && p.label == w.label
                        && previous.cards.get(p.from).map(|c| &c.key) == cards.get(w.from).map(|c| &c.key)
                        && previous.cards.get(p.to).map(|c| &c.key) == cards.get(w.to).map(|c| &c.key)
                });
                w.solid_since = match before {
                    Some(p) if p.state == WireState::Solid => p.solid_since,
                    _ => Some(now),
                };
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
            hand: Vec::new(),
            show_hand: true,
            pulses: Vec::new(),
            saved_view: None,
            fit: 0,
            cache: Cache::new(),
            wire_mesh: RefCell::new(None),
            cards_cache: Cache::new(),
            hand_now: Cell::new(None),
            faces: HashMap::new(),
            card_faces: HashMap::new(),
            drawn: Cell::new(None),
            version: 0,
        }
    }

    /// Deals the crew: who appears where in this graph, and the badges
    /// from open mentions among its nodes.
    /// The crew, with each one's portrait file (or `None`).
    pub fn set_crew(&mut self, crew: &[Member], portraits: &[Option<Box<str>>]) {
        self.card_faces = crew
            .iter()
            .zip(portraits)
            .filter_map(|(m, p)| Some((m.rel.to_lowercase(), p.clone()?)))
            .collect();
        let mut hand: Vec<HandCard> = crew
            .iter()
            .zip(portraits)
            .map(|(m, portrait)| {
                let mentions = |c: &Card| c.tasks.iter().filter(|t| t.people.contains(&m.key)).count();
                HandCard {
                    key: m.key.clone(),
                    name: m.name.clone(),
                    role: m.role.clone(),
                    initials: crew::initials(&m.name),
                    color: m.color.map_or(accent(&m.key), |[r, g, b]| Color::from_rgb8(r, g, b)),
                    bot: m.bot,
                    nodes: self.cards.iter().filter(|c| mentions(c) > 0).count(),
                    open: self
                        .cards
                        .iter()
                        .flat_map(|c| &c.tasks)
                        .filter(|t| !t.done && t.people.contains(&m.key))
                        .count() as u32,
                    badge: None,
                    portrait: portrait.clone(),
                }
            })
            .collect();
        let counts: Vec<u32> = hand.iter().map(|h| h.open).collect();
        for (h, b) in hand.iter_mut().zip(crew::badges(&counts)) {
            h.badge = b;
        }
        self.hand = hand;
        self.touch();
    }

    fn hand_shown(&self) -> bool {
        self.show_hand && !self.hand.is_empty()
    }

    /// A crew card's place on screen; the hovered one rises.
    fn hand_rect(&self, i: usize, size: Size, raised: bool) -> Rectangle {
        self.hand_rect_at(i, size, if raised { 1.0 } else { 0.0 })
    }

    /// A crew card `lift` of the way (0 resting, 1 risen) up.
    fn hand_rect_at(&self, i: usize, size: Size, lift: f32) -> Rectangle {
        let n = self.hand.len() as f32;
        let step = self.hand_step(size);
        let total = HAND_W + step * (n - 1.0);
        let x = size.width / 2.0 - total / 2.0 + i as f32 * step;
        let rest = size.height - HAND_PEEK;
        let top = size.height - HAND_H - 10.0;
        Rectangle::new(Point::new(x, rest + (top - rest) * lift), Size::new(HAND_W, HAND_H))
    }

    /// From one card's left edge to the next: a gap while they fit,
    /// overlapping when they don't (20 px kept free at each side).
    fn hand_step(&self, size: Size) -> f32 {
        let n = self.hand.len() as f32;
        (HAND_W + 12.0).min((size.width - 40.0 - HAND_W) / (n - 1.0).max(1.0)).max(24.0)
    }

    /// How much of resting card `i` shows: the last one whole, the others
    /// up to where the next one covers them.
    fn hand_visible(&self, i: usize, size: Size) -> f32 {
        if i + 1 == self.hand.len() { HAND_W } else { self.hand_step(size).min(HAND_W) }
    }

    /// How far each glow has faded (0 just ran, 1 gone), by graph line.
    fn pulse(&self, line: Option<usize>) -> Option<f32> {
        let line = line?;
        let (_, at) = self.pulses.iter().find(|(l, _)| *l == line)?;
        let t = at.elapsed().as_secs_f32() * 1000.0 / pulse_ms();
        (t < 1.0).then_some(t)
    }

    fn pulsing(&self) -> bool {
        self.pulses.iter().any(|(_, at)| at.elapsed().as_secs_f32() * 1000.0 < pulse_ms())
    }

    /// The crew card under a screen point (the raised one first).
    fn hit_hand(&self, state: &State, p: Point) -> Option<usize> {
        if !self.hand_shown() {
            return None;
        }
        let raised = match state.hover {
            Some(Hover::Hand(h)) => Some(h),
            _ => None,
        };
        if let Some(h) = raised
            && self.hand_rect(h, state.size, true).contains(p)
        {
            return Some(h);
        }
        (0..self.hand.len()).rev().find(|&i| self.hand_rect(i, state.size, false).contains(p))
    }

    /// Where a crew card dropped at `p` would go: (node card, task row).
    fn drop_target(&self, state: &State, view: View, p: Point) -> Option<(usize, Option<usize>)> {
        let g = view.graph(p);
        let i = (0..self.cards.len()).rev().find(|&i| {
            self.cards[i].kind != CardKind::Function && self.card_rect(i, state).contains(g)
        })?;
        let card = &self.cards[i];
        if card.kind == CardKind::Missing {
            return Some((i, None));
        }
        let y = g.y - card.pos.y - HEADER;
        let row = (view.zoom >= DETAIL_ZOOM && y >= 0.0)
            .then(|| (y / ROW) as usize)
            .filter(|&r| r < card.tasks.len().min(MAX_ROWS));
        Some((i, row))
    }

    /// Whether a card mentions crew member `m` on any task.
    fn mentions(&self, card: usize, m: usize) -> bool {
        let key = &self.hand[m].key;
        self.cards[card].tasks.iter().any(|t| t.people.contains(key))
    }

    /// Whether a wire is still turning solid.
    fn animating(&self) -> bool {
        self.wires.iter().any(|w| w.solid_since.is_some_and(|t| t.elapsed().as_secs_f32() * 1000.0 < SOLID_MS))
    }

    /// Something about the cards changed: redraw them.
    pub fn touch(&mut self) {
        self.version += 1;
    }

    /// The portrait files the graph shows: the hand's, and those of crew
    /// cards that are nodes.
    pub fn portraits(&self) -> Vec<Box<str>> {
        let mut out: Vec<Box<str>> = self.hand.iter().filter_map(|h| h.portrait.clone()).collect();
        out.extend(self.card_faces.values().cloned());
        out.sort();
        out.dedup();
        out
    }

    /// The decoded portraits, by file. Redraws the cards when they change.
    pub fn set_faces(&mut self, faces: HashMap<Box<str>, Handle>) {
        if faces != self.faces {
            self.faces = faces;
            self.touch();
        }
    }

    /// The face to draw for a person in the hand.
    fn hand_face(&self, c: &HandCard) -> Option<&Handle> {
        self.faces.get(c.portrait.as_deref()?)
    }

    /// The face on a node card whose note is a crew card.
    fn card_face(&self, card: &Card) -> Option<&Handle> {
        let portrait = self.card_faces.get(&card.rel.as_deref()?.to_lowercase())?;
        self.faces.get(portrait)
    }

    /// Every card's position, for the layout comment.
    pub fn layout(&self) -> Vec<(&str, f32, f32)> {
        self.cards.iter().map(|c| (c.key.as_str(), c.pos.x, c.pos.y)).collect()
    }

    pub fn view(&self) -> Element<'_, Message> {
        // The hand in a canvas of its own above the graph: a canvas is one
        // layer, its text above all its shapes, so graph labels would show
        // through the cards. It only draws; the graph takes every event.
        iced::widget::stack![
            Canvas::new(self).width(Fill).height(Fill),
            Canvas::new(HandLayer(self)).width(Fill).height(Fill),
        ]
        .into()
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
        let start = match (wire.from_pin, wire.from_row) {
            (Some(k), _) => self.cards[wire.from].pin_at(a.position(), true, k),
            (None, Some(r)) => Point::new(a.x + a.width, row_y(a, r)),
            (None, None) => handle(a),
        };
        let end = match (wire.to_pin, wire.to_row) {
            (Some(k), _) => self.cards[wire.to].pin_at(b.position(), false, k),
            (None, Some(r)) => Point::new(b.x, row_y(b, r)),
            (None, None) => Point::new(b.x, b.y + b.height.min(HEADER + 40.0) / 2.0),
        };
        (start, end)
    }

    /// The middle of a wire, where its ƒ button sits (graph coordinates).
    fn middle(&self, wire: &WireView, state: &State) -> Point {
        let (a, b) = self.wire_ends(wire, state);
        curve_points(a, b)[12]
    }

    /// The input pin of card `j` nearest to `g` (function cards only).
    fn nearest_input(&self, j: usize, state: &State, g: Point) -> Option<usize> {
        let card = &self.cards[j];
        if card.kind != CardKind::Function {
            return None;
        }
        let pos = self.card_rect(j, state).position();
        (0..card.pins_in.len()).min_by(|&a, &b| {
            let da = (card.pin_at(pos, false, a).y - g.y).abs();
            let db = (card.pin_at(pos, false, b).y - g.y).abs();
            da.total_cmp(&db)
        })
    }

    /// Whether a function can go on this wire: not on a wire to or from a
    /// function node's pin, which only carries a file.
    fn takes_function(&self, w: &WireView) -> bool {
        self.cards[w.from].kind != CardKind::Function && self.cards[w.to].kind != CardKind::Function
    }

    fn hit(&self, state: &State, view: View, p: Point) -> Option<Hover> {
        if let Some(m) = self.hit_hand(state, p) {
            return Some(Hover::Hand(m));
        }
        let g = view.graph(p);
        // The ƒ button: on every wire with a function, and on the hovered
        // wire.
        let reach = 10.0 * view.zoom.max(0.6);
        for (i, w) in self.visible_wires() {
            let shown = w.label.is_some()
                || (self.takes_function(w) && matches!(state.hover, Some(Hover::Wire(h) | Hover::Button(h)) if h == i));
            if shown && view.screen(self.middle(w, state)).distance(p) <= reach {
                return Some(Hover::Button(i));
            }
        }
        // Cards on top of wires; later cards on top of earlier ones.
        for i in (0..self.cards.len()).rev() {
            let r = self.card_rect(i, state);
            let card = &self.cards[i];
            if card.kind == CardKind::Function {
                for k in 0..card.pins_out.len() {
                    if view.screen(card.pin_at(r.position(), true, k)).distance(p) <= 8.0 {
                        return Some(Hover::Pin(i, true, k));
                    }
                }
                for k in 0..card.pins_in.len() {
                    if view.screen(card.pin_at(r.position(), false, k)).distance(p) <= 8.0 {
                        return Some(Hover::Pin(i, false, k));
                    }
                }
            } else if view.screen(handle(r)).distance(p) <= 8.0 {
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
            match self.saved_view {
                Some(v) => restore(state, v, bounds.size()),
                None => self.fit(state, bounds.size()),
            }
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
        state.size = bounds.size();
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
                if let Some(Hover::Hand(m)) = hit {
                    let now = Instant::now();
                    let key = usize::MAX - m;
                    let double = state
                        .last_click
                        .is_some_and(|(c, at)| c == key && now.duration_since(at).as_millis() < 450);
                    state.last_click = Some((key, now));
                    if double {
                        state.last_click = None;
                        return Some(canvas::Action::publish(Message::OpenCrew(m)).and_capture());
                    }
                    state.interaction = Interaction::Dealing { member: m, at: p };
                    return Some(canvas::Action::request_redraw().and_capture());
                }
                match hit {
                    Some(Hover::Handle(i)) => {
                        state.interaction = Interaction::Wiring {
                            from: i,
                            pin: None,
                            to: view.graph(p),
                        };
                    }
                    Some(Hover::Pin(i, true, k)) => {
                        state.interaction = Interaction::Wiring {
                            from: i,
                            pin: Some(k),
                            to: view.graph(p),
                        };
                    }
                    Some(Hover::Button(i)) => {
                        state.selected = Some(Selected::Wire(i));
                        return Some(canvas::Action::publish(Message::Function(i)).and_capture());
                    }
                    Some(Hover::Card(i) | Hover::Pin(i, false, _)) => {
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
                    // Handled above.
                    Some(Hover::Hand(_)) => {}
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
                    Interaction::Dealing { at, .. } => {
                        *at = p;
                        Some(canvas::Action::request_redraw())
                    }
                    Interaction::Idle => {
                        let hover = point.and_then(|p| self.hit(state, view, p));
                        if hover != state.hover {
                            state.rise = match hover {
                                Some(Hover::Hand(h)) => Some((h, Instant::now())),
                                _ => None,
                            };
                            state.hover = hover;
                            return Some(canvas::Action::request_redraw());
                        }
                        None
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left | mouse::Button::Middle)) => {
                let done = std::mem::take(&mut state.interaction);
                // What's under the cursor now: a dropped crew card goes back
                // down at once instead of waiting for the next mouse move.
                if !matches!(done, Interaction::Idle) {
                    let hover = point.and_then(|p| self.hit(state, view, p));
                    if hover != state.hover {
                        state.rise = match hover {
                            Some(Hover::Hand(h)) => Some((h, Instant::now())),
                            _ => None,
                        };
                        state.hover = hover;
                    }
                }
                if let Interaction::Dealing { member, at } = done {
                    // Dropped back on the hand, or on nothing: put it back.
                    let on_hand = self.hit_hand(state, at).is_some();
                    return Some(match (on_hand, self.drop_target(state, view, at)) {
                        (false, Some((card, task))) => canvas::Action::publish(Message::Assign { member, card, task }),
                        _ => canvas::Action::request_redraw(),
                    });
                }
                match done {
                    Interaction::Dragging { card, at, moved: true, .. } => {
                        Some(canvas::Action::publish(Message::Moved(card, at)))
                    }
                    Interaction::Wiring { from, pin, .. } => {
                        let target = point.and_then(|p| match self.hit(state, view, p) {
                            Some(Hover::Pin(i, false, k)) if i != from => Some(PinRef { card: i, pin: Some(k) }),
                            Some(Hover::Card(i) | Hover::Handle(i) | Hover::Pin(i, true, _)) if i != from => Some(PinRef {
                                card: i,
                                pin: self.nearest_input(i, state, view.graph(p)),
                            }),
                            _ => None,
                        });
                        Some(match target {
                            Some(to) => canvas::Action::publish(Message::Connect(PinRef { card: from, pin }, to)),
                            None => canvas::Action::request_redraw(),
                        })
                    }
                    Interaction::Idle => None,
                    Interaction::Panning { pan, .. } if pan != state.pan => {
                        Some(canvas::Action::publish(Message::Viewed(saved_view(state))))
                    }
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
                Some(canvas::Action::publish(Message::Viewed(saved_view(state))).and_capture())
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
                #[cfg(debug_assertions)]
                if let Some(action) = pan_bench::step(state) {
                    return Some(action);
                }
                // Marching ants on the hovered or selected wire only.
                let rising = state.rise.is_some_and(|(_, at)| at.elapsed().as_secs_f32() * 1000.0 < RISE_MS);
                let animated = matches!(state.hover, Some(Hover::Wire(_) | Hover::Button(_)))
                    || matches!(state.selected, Some(Selected::Wire(_)))
                    || self.animating()
                    || self.pulsing()
                    || rising;
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
        let growing = self.wires.iter().filter(|w| progress(w) < 1.0).count();
        let key = DrawKey {
            pan: state.pan,
            zoom,
            scale: smooth::scale(),
            size: bounds.size(),
            version: self.version,
            lifted,
            links: self.show_links,
            growing,
        };
        if self.drawn.get() != Some(key) {
            self.cache.clear();
            self.cards_cache.clear();
            self.drawn.set(Some(key));
        }
        let screen = Rectangle::new(Point::ORIGIN, bounds.size());

        #[cfg(debug_assertions)]
        let built = std::time::Instant::now();
        let mut fresh = None;
        let still = self.cache.draw(renderer, bounds.size(), |frame| {
            draw_grid(frame, view);
            let mut shapes = Shapes::new();
            for (_, wire) in self.visible_wires() {
                if lifted.is_some_and(|c| wire.from == c || wire.to == c) || progress(wire) < 1.0 {
                    continue;
                }
                let (a, b) = self.wire_ends(wire, state);
                draw_wire(frame, &mut shapes, view, a, b, wire, Look::default(), screen);
            }
            fresh = Some(shapes);
        });
        let wires = {
            let mut kept = self.wire_mesh.borrow_mut();
            if let Some(shapes) = fresh {
                *kept = Some(shapes.into_geometry(bounds.size()).cache(Group::unique(), kept.take()));
            }
            kept.as_ref().map_or_else(|| Shapes::new().into_geometry(bounds.size()), iced_wgpu::Geometry::load)
        };
        let cards = self.cards_cache.draw(renderer, bounds.size(), |frame| {
            for (i, card) in self.cards.iter().enumerate() {
                if Some(i) != lifted {
                    draw_card(frame, view, card, card.pos, screen, self.card_face(card));
                }
            }
        });

        #[cfg(debug_assertions)]
        pan_bench::drawn(built.elapsed());
        // The overlay: what moves or reacts. Its wires go under the card
        // being dragged.
        let mut over = Shapes::new();
        let mut frame = Frame::new(renderer, bounds.size());
        let ants = state
            .started
            .map_or(0, |s| (s.elapsed().as_millis() / 50) as usize % 11);
        for (i, wire) in self.visible_wires() {
            let lit = matches!(state.hover, Some(Hover::Wire(h) | Hover::Button(h)) if h == i)
                || state.selected == Some(Selected::Wire(i));
            let moving = lifted.is_some_and(|c| wire.from == c || wire.to == c);
            if lit || moving || progress(wire) < 1.0 {
                let (a, b) = self.wire_ends(wire, state);
                let look = Look {
                    lit,
                    ants: if lit { 11 - ants } else { 0 },
                    button: lit && self.takes_function(wire),
                    pressed: state.hover == Some(Hover::Button(i)),
                };
                draw_wire(&mut frame, &mut over, view, a, b, wire, look, screen);
            }
        }
        if let Some(i) = lifted {
            let r = self.card_rect(i, state);
            draw_card(&mut frame, view, &self.cards[i], r.position(), screen, self.card_face(&self.cards[i]));
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
            let lit = state.hover == Some(Hover::Handle(i));
            frame.fill(
                &Path::circle(view.screen(handle(self.card_rect(i, state))), if lit { 6.0 } else { 4.5 }),
                if lit { theme::ACCENT } else { theme::MUTED },
            );
        }
        if let Some(Hover::Pin(i, output, k)) = state.hover {
            let pos = self.card_rect(i, state).position();
            frame.fill(&Path::circle(view.screen(self.cards[i].pin_at(pos, output, k)), 6.0), theme::ACCENT);
        }
        if let Interaction::Wiring { from, pin, to } = state.interaction {
            let r = self.card_rect(from, state);
            let a = match pin {
                Some(k) => self.cards[from].pin_at(r.position(), true, k),
                None => handle(r),
            };
            over.dashed(&curve_screen(view, a, to), 1.5, theme::ACCENT, 6.0 * zoom, 5.0 * zoom, 0.0);
        }
        // Wires whose function just ran glow, then fade.
        for (_, wire) in self.visible_wires() {
            if let Some(t) = self.pulse(wire.line) {
                let (a, b) = self.wire_ends(wire, state);
                let fade = 1.0 - t;
                over.line(&curve_screen(view, a, b), 2.0 + 6.0 * fade, Color { a: 0.55 * fade, ..theme::SUCCESS });
            }
        }
        for (i, card) in self.cards.iter().enumerate() {
            if card.kind == CardKind::Function
                && let Some(t) = self.pulse(card.line)
            {
                let r = self.card_rect(i, state);
                frame.stroke(
                    &Path::rounded_rectangle(view.screen(r.position()), r.size() * zoom, (8.0 * zoom).into()),
                    Stroke::default()
                        .with_color(Color { a: 0.7 * (1.0 - t), ..theme::SUCCESS })
                        .with_width(2.0 + 4.0 * (1.0 - t)),
                );
            }
        }
        self.draw_crew(&mut frame, state, view, bounds.size());
        vec![still, wires, cards, over.into_geometry(bounds.size()), frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.interaction {
            Interaction::Panning { .. } | Interaction::Dragging { .. } => return mouse::Interaction::Grabbing,
            Interaction::Wiring { .. } => return mouse::Interaction::Crosshair,
            Interaction::Dealing { .. } => return mouse::Interaction::Grabbing,
            Interaction::Idle => {}
        }
        if !cursor.is_over(bounds) {
            return mouse::Interaction::None;
        }
        match state.hover {
            Some(Hover::Card(_) | Hover::Hand(_)) => mouse::Interaction::Grab,
            Some(Hover::Handle(_) | Hover::Pin(..)) => mouse::Interaction::Crosshair,
            Some(Hover::Wire(_) | Hover::Button(_)) => mouse::Interaction::Pointer,
            None => mouse::Interaction::None,
        }
    }
}

impl Graph {
    /// The crew: highlights for the hovered (or dragged) person, and what
    /// the hand looks like now, for `HandLayer` to draw over the graph.
    fn draw_crew(&self, frame: &mut Frame, state: &State, view: View, size: Size) {
        self.hand_now.set(None);
        if !self.hand_shown() {
            return;
        }
        let zoom = view.zoom;
        let focus = match (state.interaction, state.hover) {
            (Interaction::Dealing { member, .. }, _) => Some(member),
            (Interaction::Idle, Some(Hover::Hand(m))) => Some(m),
            _ => None,
        };
        let screen = Rectangle::new(Point::ORIGIN, size);
        // Where this person appears.
        if let Some(m) = focus {
            let color = self.hand[m].color;
            let key = &self.hand[m].key;
            for (i, card) in self.cards.iter().enumerate() {
                if !self.mentions(i, m) {
                    continue;
                }
                let r = self.card_rect(i, state);
                let p = view.screen(r.position());
                if !Rectangle::new(p, r.size() * zoom).intersects(&screen) {
                    continue;
                }
                frame.stroke(
                    &Path::rounded_rectangle(p, r.size() * zoom, (8.0 * zoom).into()),
                    Stroke::default().with_color(color).with_width(2.5),
                );
                if zoom >= DETAIL_ZOOM {
                    for (k, t) in card.tasks.iter().take(MAX_ROWS).enumerate() {
                        if t.people.contains(key) {
                            let y = r.y + HEADER + k as f32 * ROW - 3.0;
                            frame.fill(
                                &Path::rounded_rectangle(
                                    view.screen(Point::new(r.x + 6.0, y)),
                                    Size::new((r.width - 12.0) * zoom, ROW * zoom),
                                    (4.0 * zoom).into(),
                                ),
                                Color { a: 0.18, ..color },
                            );
                        }
                    }
                }
            }
        }
        // The drop target while dragging.
        if let Interaction::Dealing { member, at } = state.interaction
            && self.hit_hand(state, at).is_none()
            && let Some((card, row)) = self.drop_target(state, view, at)
        {
            let color = self.hand[member].color;
            let r = self.card_rect(card, state);
            let (pos, sz) = match row {
                Some(k) => (Point::new(r.x + 4.0, r.y + HEADER + k as f32 * ROW - 4.0), Size::new(r.width - 8.0, ROW + 2.0)),
                None => (r.position(), r.size()),
            };
            frame.stroke(
                &Path::rounded_rectangle(view.screen(pos), sz * zoom, (6.0 * zoom).into()),
                Stroke::default().with_color(color).with_width(2.0),
            );
        }
        let raised = match (state.interaction, state.hover) {
            (Interaction::Idle, Some(Hover::Hand(h))) => {
                // Eases up over RISE_MS.
                let t = state
                    .rise
                    .filter(|(r, _)| *r == h)
                    .map_or(1.0, |(_, at)| (at.elapsed().as_secs_f32() * 1000.0 / RISE_MS).min(1.0));
                Some((h, 1.0 - (1.0 - t) * (1.0 - t)))
            }
            _ => None,
        };
        let dealing = match state.interaction {
            Interaction::Dealing { member, at } => Some((member, at)),
            _ => None,
        };
        self.hand_now.set(Some(HandNow { size, raised, dealing }));
    }

    /// The hand, drawn by `HandLayer` in a layer above the graph: resting
    /// cards, then the raised one, then the card being dragged.
    fn draw_hand(&self, frame: &mut Frame, now: HandNow) {
        let size = now.size;
        let raised = now.raised.map(|(h, lift)| (h, self.hand_rect_at(h, size, lift)));
        for i in (0..self.hand.len()).filter(|&i| Some(i) != raised.map(|(h, _)| h)) {
            let r = self.hand_rect(i, size, false);
            let mut left = r.x;
            let right = r.x + self.hand_visible(i, size);
            // A raised card to the left covers part of this one: lay its
            // words out in what's left.
            if let Some((h, top)) = raised
                && h < i
            {
                left = left.max(top.x + top.width);
            }
            self.draw_hand_card(frame, i, r, left - r.x, right - left);
        }
        if let Some((h, r)) = raised {
            self.draw_hand_card(frame, h, r, 0.0, HAND_W);
        }
        // The card being dragged: a small token under the cursor.
        if let Some((member, at)) = now.dealing {
            let c = &self.hand[member];
            frame.fill(&Path::circle(at, 18.0), c.color);
            frame.stroke(&Path::circle(at, 18.0), Stroke::default().with_color(theme::HEADING).with_width(1.5));
            let semibold = Font {
                weight: iced::font::Weight::Semibold,
                ..theme::UI_FONT
            };
            match self.hand_face(c) {
                Some(portrait) => draw_face(frame, portrait, at, 17.0, false),
                None => frame.fill_text(Text {
                    content: c.initials.clone(),
                    position: Point::new(at.x, at.y - 8.0),
                    color: theme::BG,
                    size: Pixels(13.0),
                    font: semibold,
                    align_x: iced::widget::text::Alignment::Center,
                    ..Text::default()
                }),
            }
            frame.fill_text(Text {
                content: format!("@{}", c.key),
                position: Point::new(at.x + 24.0, at.y - 8.0),
                color: theme::HEADING,
                size: Pixels(13.0),
                font: theme::UI_FONT,
                ..Text::default()
            });
        }
    }

    /// A crew card. `from` and `shown`: the part of its width no other card
    /// covers; when that's less than the whole card, its face, name and
    /// badge are laid out to fit that strip.
    fn draw_hand_card(&self, frame: &mut Frame, i: usize, r: Rectangle, from: f32, shown: f32) {
        let c = &self.hand[i];
        let body = Path::rounded_rectangle(r.position(), r.size(), 10.0.into());
        frame.fill(&body, theme::RAISED);
        frame.stroke(&body, Stroke::default().with_color(Color { a: 0.8, ..c.color }).with_width(1.5));
        frame.fill(&Path::rounded_rectangle(r.position(), Size::new(r.width, 5.0), 10.0.into()), c.color);
        let semibold = Font {
            weight: iced::font::Weight::Semibold,
            ..theme::UI_FONT
        };
        // Text in a canvas is drawn above every shape in it, so nothing is
        // written where another card covers this one.
        if from > 0.5 || shown < r.width - 0.5 {
            if shown >= 12.0 {
                let strip = Rectangle::new(Point::new(r.x + from, r.y), r.size());
                self.draw_hand_strip(frame, c, strip, shown, semibold);
            }
            return;
        }
        let text = |frame: &mut Frame, content: String, x: f32, y: f32, size: f32, color: Color, font: Font| {
            frame.fill_text(Text {
                content,
                position: Point::new(r.x + x, r.y + y),
                color,
                size: Pixels(size),
                font,
                ..Text::default()
            });
        };
        // The part that shows while resting: portrait, name, role, badge.
        let face = Point::new(r.x + 27.0, r.y + 29.0);
        if c.bot {
            frame.fill(&Path::rounded_rectangle(Point::new(face.x - 17.0, face.y - 17.0), Size::new(34.0, 34.0), 7.0.into()), c.color);
        } else {
            frame.fill(&Path::circle(face, 17.0), c.color);
        }
        match self.hand_face(c) {
            Some(portrait) => draw_face(frame, portrait, face, 17.0, c.bot),
            None => frame.fill_text(Text {
                content: c.initials.clone(),
                position: Point::new(face.x, face.y - 8.5),
                color: theme::BG,
                size: Pixels(14.0),
                font: semibold,
                align_x: iced::widget::text::Alignment::Center,
                ..Text::default()
            }),
        }
        text(frame, fit(&c.name, r.width - 60.0, 13.0), 52.0, 13.0, 13.0, theme::HEADING, semibold);
        let role = if c.role.is_empty() { format!("@{}", c.key) } else { c.role.clone() };
        text(frame, fit(&role, r.width - 60.0, 11.0), 52.0, 31.0, 11.0, theme::MUTED, theme::UI_FONT);
        if let Some(badge) = c.badge {
            let color = badge_color(badge);
            let pill = Rectangle::new(Point::new(r.x + 10.0, r.y + 51.0), Size::new(r.width - 20.0, 17.0));
            frame.fill(&Path::rounded_rectangle(pill.position(), pill.size(), 8.5.into()), Color { a: 0.22, ..color });
            frame.fill_text(Text {
                content: badge.label().to_owned(),
                position: Point::new(r.x + r.width / 2.0, r.y + 53.0),
                color,
                size: Pixels(10.5),
                font: semibold,
                align_x: iced::widget::text::Alignment::Center,
                ..Text::default()
            });
        }
        // Shown when it rises.
        let nodes = match c.nodes {
            0 => "Not in this graph".to_owned(),
            1 => "In 1 node".to_owned(),
            n => format!("In {n} nodes"),
        };
        text(frame, nodes, 12.0, 80.0, 11.5, theme::TEXT, theme::UI_FONT);
        text(frame, format!("{} open task{} here", c.open, if c.open == 1 { "" } else { "s" }), 12.0, 96.0, 11.5, theme::MUTED, theme::UI_FONT);
        text(frame, "Drag onto a task".to_owned(), 12.0, 112.0, 10.5, theme::FAINT, theme::UI_FONT);
    }

    /// A resting card partly covered by the next one: face, name and badge
    /// (or role) stacked and centred in the `shown` pixels that are visible.
    fn draw_hand_strip(&self, frame: &mut Frame, c: &HandCard, r: Rectangle, shown: f32, semibold: Font) {
        use iced::widget::text::Alignment;
        let cx = r.x + shown / 2.0;
        let inner = (shown - 8.0).max(0.0);
        let centred = |frame: &mut Frame, content: String, y: f32, size: f32, color: Color, font: Font| {
            frame.fill_text(Text {
                content,
                position: Point::new(cx, r.y + y),
                color,
                size: Pixels(size),
                font,
                align_x: Alignment::Center,
                ..Text::default()
            });
        };
        let radius = (shown / 2.0 - 5.0).clamp(6.0, 13.0);
        let face = Point::new(cx, r.y + 10.0 + radius);
        if c.bot {
            let side = radius * 2.0;
            frame.fill(&Path::rounded_rectangle(Point::new(face.x - radius, face.y - radius), Size::new(side, side), (radius * 0.4).into()), c.color);
        } else {
            frame.fill(&Path::circle(face, radius), c.color);
        }
        let initials = if radius >= 10.0 { 12.0 } else { 9.0 };
        match self.hand_face(c) {
            Some(portrait) => draw_face(frame, portrait, face, radius, c.bot),
            None => frame.fill_text(Text {
                content: c.initials.clone(),
                position: Point::new(face.x, face.y - initials * 0.62),
                color: theme::BG,
                size: Pixels(initials),
                font: semibold,
                align_x: Alignment::Center,
                ..Text::default()
            }),
        }
        if inner < 24.0 {
            // Too narrow for words: the badge as a coloured bar.
            if let Some(badge) = c.badge {
                let bar = Rectangle::new(Point::new(r.x + 4.0, r.y + 56.0), Size::new(inner, 6.0));
                frame.fill(&Path::rounded_rectangle(bar.position(), bar.size(), 3.0.into()), badge_color(badge));
            }
            return;
        }
        centred(frame, fit(&c.name, inner, 11.5), 40.0, 11.5, theme::HEADING, semibold);
        match c.badge {
            Some(badge) => {
                let color = badge_color(badge);
                let pill = Rectangle::new(Point::new(r.x + 4.0, r.y + 55.0), Size::new(inner, 15.0));
                // Capitals run wider than `fit` reckons: about 0.68 em.
                let fits = |s: &str| s.chars().count() as f32 * 9.5 * 0.68 <= inner - 6.0;
                let words = [badge.label(), badge_short(badge)].into_iter().find(|s| fits(s));
                // No room for its word: a solid pill in the badge's colour.
                let alpha = if words.is_some() { 0.22 } else { 0.85 };
                frame.fill(&Path::rounded_rectangle(pill.position(), pill.size(), 7.5.into()), Color { a: alpha, ..color });
                if let Some(words) = words {
                    centred(frame, words.to_owned(), 56.5, 9.5, color, semibold);
                }
            }
            None => {
                let role = if c.role.is_empty() { format!("@{}", c.key) } else { c.role.clone() };
                centred(frame, fit(&role, inner, 10.0), 56.0, 10.0, theme::MUTED, theme::UI_FONT);
            }
        }
    }
}

/// What the hand looks like this frame: the canvas size, the raised card
/// and how far it has risen (0 to 1), and the card being dragged.
#[derive(Debug, Clone, Copy)]
struct HandNow {
    size: Size,
    raised: Option<(usize, f32)>,
    dealing: Option<(usize, Point)>,
}

/// Draws the crew hand above the graph (see `Graph::view`). The graph
/// canvas is drawn first and leaves `hand_now` for it.
struct HandLayer<'a>(&'a Graph);

impl canvas::Program<Message> for HandLayer<'_> {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let Some(now) = self.0.hand_now.get() else {
            return Vec::new();
        };
        let mut frame = Frame::new(renderer, bounds.size());
        self.0.draw_hand(&mut frame, now);
        vec![frame.into_geometry()]
    }
}

fn badge_color(badge: Badge) -> Color {
    match badge {
        Badge::Overloaded => theme::DANGER,
        Badge::Powerhouse => theme::MENTION,
    }
}

/// A badge's word for a narrow card.
fn badge_short(badge: Badge) -> &'static str {
    match badge {
        Badge::Overloaded => "OVERLOAD",
        Badge::Powerhouse => "POWER",
    }
}

/// The view as it's remembered: the graph point in the middle of the
/// canvas, and the zoom.
fn saved_view(state: &State) -> SavedView {
    let middle = View { pan: state.pan, zoom: state.zoom }.graph(Point::new(state.size.width / 2.0, state.size.height / 2.0));
    SavedView { x: middle.x, y: middle.y, zoom: state.zoom }
}

/// Puts a remembered view back, centred in a canvas of this size.
fn restore(state: &mut State, v: SavedView, size: Size) {
    let zoom = v.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    state.zoom = zoom;
    state.pan = Vector::new(size.width / 2.0 - v.x * zoom, size.height / 2.0 - v.y * zoom);
}

/// The middle of task row `r` on a card at `card` (graph coordinates).
fn row_y(card: Rectangle, r: usize) -> f32 {
    card.y + HEADER + r as f32 * ROW + 8.5
}

/// The row of `card` that a `#^block` or `#heading` link points at, if the
/// card shows it.
fn row_in(store: &mut Store, card: &Card, sub: &str) -> Option<usize> {
    if card.kind == CardKind::Function || card.kind == CardKind::Missing {
        return None;
    }
    let text = store.get(card.rel.as_deref()?).ok()?;
    let lines: Vec<usize> = card.tasks.iter().map(|t| t.line).collect();
    links::task_at(text, sub, &lines).filter(|&r| r < MAX_ROWS)
}

/// Out of the right side of a card (mid-header), where its handle is.
fn handle(r: Rectangle) -> Point {
    Point::new(r.x + r.width, r.y + r.height.min(HEADER + 40.0) / 2.0)
}

/// How far a wire is from dashed (0) to solid (1).
fn progress(w: &WireView) -> f32 {
    w.solid_since
        .map_or(1.0, |t| (t.elapsed().as_secs_f32() * 1000.0 / SOLID_MS).min(1.0))
}

/// What a wire end takes: a note is a File, `#^id` a Task, `#Heading` a
/// Section.
fn end_kind(end: &End) -> Option<PortKind> {
    match end {
        End::Note { sub: None, .. } => Some(PortKind::File),
        End::Note { sub: Some(s), .. } if s.starts_with('^') => Some(PortKind::Task),
        End::Note { .. } => Some(PortKind::Section),
        End::Pin { .. } => None,
    }
}

/// Whether a `## Wires` wire is plain, solid or broken (and why).
fn wire_state(
    w: &nodes::Wire,
    a: &Card,
    b: &Card,
    from_pin: Option<usize>,
    to_pin: Option<usize>,
    library: &Library,
) -> WireState {
    use func::check::port_end;
    // A wire to or from a function card's pin.
    if a.kind == CardKind::Function || b.kind == CardKind::Function {
        if a.kind == b.kind {
            return WireState::Broken("Wire a function's pins to notes, not to another function".into());
        }
        if w.function.is_some() {
            return WireState::Broken("A wire to a function's pin can't have a function of its own".into());
        }
        let (card, pins, pin, note, end) = if a.kind == CardKind::Function {
            (a, &a.pins_out, from_pin, &w.to, &w.from)
        } else {
            (b, &b.pins_in, to_pin, &w.from, &w.to)
        };
        if let Some(problem) = &card.problem {
            return WireState::Broken(problem.clone());
        }
        let Some(k) = pin else {
            let name = match end {
                End::Pin { pin, .. } => pin.as_str(),
                End::Note { .. } => "",
            };
            return WireState::Broken(format!("{} has no pin `{name}`", card.title));
        };
        let (name, want) = &pins[k];
        if end_kind(note) != Some(*want) {
            return WireState::Broken(format!("`{name}` wants {}", port_end(*want)));
        }
        return WireState::Solid;
    }
    let Some(call) = &w.function else {
        return WireState::Plain;
    };
    let (name, args) = match func::parse_call(call) {
        Ok(parsed) => parsed,
        Err(e) => return WireState::Broken(e),
    };
    let Some((block, _, _)) = library.find(&name) else {
        return WireState::Broken(format!("No function called {name}"));
    };
    let f = &block.function;
    let Some((kin, kout)) = f.wire_shape() else {
        return WireState::Broken(format!(
            "{name} has {} in and {} out: list it in ## Nodes as `fn {name}` and wire its pins",
            f.inputs.len(),
            f.outputs.len()
        ));
    };
    if end_kind(&w.from) != Some(kin) {
        return WireState::Broken(format!("{name}'s `{}` wants {}", f.inputs[0].name, port_end(kin)));
    }
    if end_kind(&w.to) != Some(kout) {
        return WireState::Broken(format!("{name}'s `{}` wants {}", f.outputs[0].name, port_end(kout)));
    }
    let checked = func::check::check(f);
    if let Some(p) = checked.first_problem() {
        return WireState::Broken(format!("{name}: {p}"));
    }
    if let Some(p) = func::check::check_call(f, &args).into_iter().next() {
        return WireState::Broken(p);
    }
    WireState::Solid
}

/// A pin's colour by what it carries.
pub fn pin_color(kind: PortKind) -> Color {
    match kind {
        PortKind::File => theme::LINK,
        PortKind::Task => theme::SUCCESS,
        PortKind::Section => theme::TAG,
    }
}

/// One node's card: the note's name, folder and tasks.
fn card(
    node: &nodes::Node,
    rel: &str,
    vault: &Vault,
    store: &mut Store,
    team: Option<&Format>,
    library: &Library,
) -> Card {
    let blank = |kind, title: &str, folder: &str| Card {
        key: node.key.clone(),
        rel: None,
        title: title.to_owned(),
        folder: folder.to_owned(),
        kind,
        tasks: Vec::new(),
        done: 0,
        pos: Point::ORIGIN,
        pins_in: Vec::new(),
        pins_out: Vec::new(),
        problem: None,
        line: Some(node.line),
        stale: None,
        accent: accent(folder),
    };
    let target = match &node.kind {
        NodeKind::Function { name } => {
            let mut card = blank(CardKind::Function, name, "");
            match library.find(name) {
                Some((block, _, _)) => {
                    let f = &block.function;
                    card.pins_in = f.inputs.iter().map(|p| (p.name.clone(), p.kind)).collect();
                    card.pins_out = f.outputs.iter().map(|p| (p.name.clone(), p.kind)).collect();
                    card.problem = func::check::check(f).first_problem().map(str::to_owned);
                }
                None => card.problem = Some(format!("No function called {name}")),
            }
            return card;
        }
        NodeKind::File { target } => target,
    };
    let (entry, kind) = match vault.resolve_from(target, Some(rel)) {
        Resolved::One(entry) => (entry, CardKind::Note),
        Resolved::Ambiguous { picked, .. } => (picked, CardKind::Ambiguous),
        Resolved::NotFound | Resolved::Attachment(_) => {
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
            people: crew::mentions(raw).into_iter().map(|(_, n)| n).collect(),
            line,
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

pub fn draw_grid(frame: &mut Frame, view: View) {
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

/// A wire's curve on screen, as points close enough to look smooth.
pub fn curve_screen(view: View, a: Point, b: Point) -> Vec<Point> {
    let dx = ((b.x - a.x).abs() / 2.0).max(60.0);
    smooth::bezier(
        view.screen(a),
        view.screen(Point::new(a.x + dx, a.y)),
        view.screen(Point::new(b.x - dx, b.y)),
        view.screen(b),
    )
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

#[derive(Debug, Clone, Copy, Default)]
struct Look {
    lit: bool,
    ants: usize,
    /// Show the ƒ button (a hovered wire).
    button: bool,
    /// The mouse is on the button.
    pressed: bool,
}

/// A wire: its line, end dot, arrowhead and ƒ dot go in `shapes` (smooth),
/// its words in `frame`.
#[allow(clippy::too_many_arguments)]
fn draw_wire(
    frame: &mut Frame,
    shapes: &mut Shapes,
    view: View,
    a: Point,
    b: Point,
    wire: &WireView,
    look: Look,
    screen: Rectangle,
) {
    // Skip wires entirely off screen.
    let (sa, sb) = (view.screen(a), view.screen(b));
    let reach = ((sb.x - sa.x).abs() / 2.0).max(60.0 * view.zoom);
    let bbox = Rectangle::new(
        Point::new(sa.x.min(sb.x) - reach, sa.y.min(sb.y) - 30.0),
        Size::new((sa.x - sb.x).abs() + 2.0 * reach, (sa.y - sb.y).abs() + 60.0),
    );
    if !bbox.intersects(&screen) {
        return;
    }
    let lit = look.lit;
    let solid = wire.state == WireState::Solid;
    let color = if lit {
        theme::ACCENT
    } else if wire.link {
        Color { a: 0.55, ..theme::MUTED }
    } else if solid {
        theme::MENTION
    } else {
        theme::MUTED
    };
    let z = view.zoom.max(0.4);
    let grown = progress(wire);
    // A wire turning solid closes its gaps.
    let gap = if solid { 5.0 * z * (1.0 - grown) } else { 5.0 * z };
    let width = if lit {
        2.2
    } else if solid {
        1.8
    } else {
        1.5
    };
    let points = curve_screen(view, a, b);
    if solid && grown >= 1.0 && !lit {
        shapes.line(&points, width, color);
    } else {
        // The marching ants: `ants` counts down through 11 steps a dash.
        let (dash, gap) = (6.0 * z, gap.max(0.01));
        shapes.dashed(&points, width, color, dash, gap, look.ants as f32 / 11.0 * (dash + gap));
    }
    // A dot where it leaves a task row, so the row reads as its start.
    if wire.from_row.is_some() {
        shapes.circle(sa, 3.0 * z, color);
    }
    // Arrowhead.
    let s = 7.0 * z;
    shapes.fill(&[sb, Point::new(sb.x - s, sb.y - s * 0.6), Point::new(sb.x - s, sb.y + s * 0.6)], color);
    let mid = view.screen(curve_points(a, b)[12]);
    let broken = match &wire.state {
        WireState::Broken(why) => Some(why.as_str()),
        _ => None,
    };
    let stale = wire.stale.as_deref().filter(|_| broken.is_none());
    // The ƒ dot: on wires with a function, and on a hovered wire.
    if wire.label.is_some() || look.button {
        let k = z.min(1.2);
        let ring = if broken.is_some() || stale.is_some() {
            theme::WARNING
        } else if lit {
            theme::ACCENT
        } else {
            color
        };
        shapes.circle(mid, 8.0 * k, if look.pressed { theme::ACCENT } else { theme::RAISED });
        shapes.circle_ring(mid, 8.0 * k, 1.2, ring);
        frame.fill_text(Text {
            content: "ƒ".into(),
            position: Point::new(mid.x, mid.y - 7.0 * k),
            color: if look.pressed { theme::BG } else { ring },
            size: Pixels(11.0 * k),
            font: theme::UI_FONT,
            align_x: iced::widget::text::Alignment::Center,
            ..Text::default()
        });
    }
    if view.zoom < DETAIL_ZOOM {
        return;
    }
    let mut above = match &wire.label {
        Some(label) => format!("ƒ {}", label.split('(').next().unwrap_or(label).trim()),
        None if look.button => "Add a function".to_owned(),
        None => String::new(),
    };
    if broken.is_some() {
        above.push_str("  ⚠");
    }
    if stale.is_some() {
        above.push_str("  ⚠ out of date");
    }
    let caption = |frame: &mut Frame, content: String, y: f32, color: Color| {
        // A hovered wire is drawn over the cards: give its words a backing
        // so they stay readable where they cross a card.
        if look.lit {
            let size = 11.0 * view.zoom;
            let width = content.chars().count() as f32 * size * 0.56 + 10.0 * view.zoom;
            let height = size * 1.5;
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(mid.x - width / 2.0, mid.y + y * view.zoom - size * 0.2),
                    Size::new(width, height),
                    (4.0 * view.zoom).into(),
                ),
                Color { a: 0.92, ..theme::BG },
            );
        }
        frame.fill_text(Text {
            content,
            position: Point::new(mid.x, mid.y + y * view.zoom),
            color,
            size: Pixels(11.0 * view.zoom),
            font: theme::UI_FONT,
            align_x: iced::widget::text::Alignment::Center,
            ..Text::default()
        });
    };
    if !above.is_empty() {
        let tint = if broken.is_some() || stale.is_some() {
            theme::WARNING
        } else if lit {
            theme::ACCENT
        } else {
            theme::MUTED
        };
        caption(frame, above, -27.0, tint);
    }
    if let Some(why) = broken.or(stale)
        && lit
    {
        caption(frame, fit(why, 440.0, 11.0), 13.0, theme::WARNING);
    }
}

/// Cuts `text` to about `width` pixels at `size`, with an ellipsis.
pub fn fit(text: &str, width: f32, size: f32) -> String {
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

/// A portrait in a circle (a rounded square for a bot) centred at `at`.
/// Drawn in its own clip: the corner radius rounds the clip, not the image.
fn draw_face(frame: &mut Frame, portrait: &Handle, at: Point, radius: f32, bot: bool) {
    let rect = Rectangle::new(Point::new(at.x - radius, at.y - radius), Size::new(radius * 2.0, radius * 2.0));
    let image = iced::advanced::image::Image {
        handle: portrait.clone(),
        filter_method: iced::advanced::image::FilterMethod::Linear,
        rotation: Radians(0.0),
        border_radius: (if bot { radius * 0.4 } else { radius }).into(),
        opacity: 1.0,
        snap: false,
    };
    // The clipped frame keeps the outer frame's coordinates.
    frame.with_clip(rect, |f| f.draw_image(rect, image));
}

fn draw_card(frame: &mut Frame, view: View, card: &Card, pos: Point, screen: Rectangle, face: Option<&Handle>) {
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
        let ring = if card.problem.is_some() { theme::WARNING } else { theme::MENTION };
        frame.stroke(&body, Stroke::default().with_color(ring).with_width(1.2));
        label(
            frame,
            fit(&format!("ƒ  {}", card.title), size.width - 24.0, 14.0),
            Point::new(p.x + 12.0 * z, p.y + 11.0 * z),
            14.0 * z,
            theme::MENTION,
            semibold,
        );
        if z < DETAIL_ZOOM {
            return;
        }
        let (sub, color) = match (&card.problem, &card.stale) {
            (Some(why), _) => (format!("⚠ {why}"), theme::WARNING),
            (None, Some(what)) => (format!("⚠ out of date: {what}"), theme::WARNING),
            (None, None) => ("function · double-click to edit".to_owned(), theme::FAINT),
        };
        label(frame, fit(&sub, size.width - 24.0, 11.0), Point::new(p.x + 12.0 * z, p.y + 31.0 * z), 11.0 * z, color, theme::UI_FONT);
        for (output, pins) in [(false, &card.pins_in), (true, &card.pins_out)] {
            for (k, (name, kind)) in pins.iter().enumerate() {
                let at = view.screen(card.pin_at(pos, output, k));
                frame.fill(&Path::circle(at, 4.5 * z), pin_color(*kind));
                frame.fill_text(Text {
                    content: fit(&format!("{name}: {}", kind.name()), size.width / 2.0 - 16.0, 12.0),
                    position: Point::new(if output { at.x - 10.0 * z } else { at.x + 10.0 * z }, at.y - 7.5 * z),
                    color: theme::TEXT,
                    size: Pixels(12.0 * z),
                    font: theme::UI_FONT,
                    align_x: if output {
                        iced::widget::text::Alignment::Right
                    } else {
                        iced::widget::text::Alignment::Left
                    },
                    ..Text::default()
                });
            }
        }
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
    // A crew card's note: their face at the top right, the title beside it.
    let title_width = match face {
        Some(portrait) => {
            let r = 13.0 * z;
            draw_face(frame, portrait, Point::new(p.x + rect.width - pad - r, p.y + 8.0 * z + r), r, false);
            inner - 34.0
        }
        None => inner,
    };

    // Zoomed out: a block with the title.
    if z < DETAIL_ZOOM {
        label(
            frame,
            fit(&card.title, title_width, 15.0),
            Point::new(p.x + pad, p.y + 14.0 * z),
            15.0 * z,
            title_color,
            semibold,
        );
        return;
    }

    label(frame, fit(&card.title, title_width, 15.0), Point::new(p.x + pad, p.y + 11.0 * z), 15.0 * z, title_color, semibold);
    let subtitle = match card.kind {
        CardKind::Missing => "No such note yet · double-click to create".to_owned(),
        CardKind::Ambiguous => format!("⚠ several notes match · {}", card.folder),
        _ => card.folder.clone(),
    };
    let subtitle_color = if card.kind == CardKind::Ambiguous { theme::WARNING } else { theme::FAINT };
    label(frame, fit(&subtitle, title_width, 11.0), Point::new(p.x + pad, p.y + 31.0 * z), 11.0 * z, subtitle_color, theme::UI_FONT);

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
