//! The function editor: one function's nodes on a canvas, Blueprint style,
//! with a panel beside it for values, the node library and problems.
//!
//! The function's text (an `obsession` block in the graph's `## Functions`
//! or in `_funct.md`) stays the saved form. Every change here is a small
//! text edit (core's `func::edit`) that the app applies to that file, then
//! the editor is refreshed from the new text: parsed, checked, and run
//! against the files it's wired to, which gives the live values shown when
//! hovering a pin.

use std::cell::Cell;
use std::time::Instant;

use iced::keyboard;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Canvas, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::{Space, button, column, container, opaque, row, rule, scrollable, text, text_input};
use iced::{Color, Element, Fill, Font, Length, Pixels, Point, Rectangle, Renderer, Size, Theme, Vector};

use obsession_core::func::check::{Checked, Source};
use obsession_core::func::eval::{self, Bind, Run};
use obsession_core::func::library::{self, Category, Default as Def, Spec};
use obsession_core::func::types::{Type, Want};
use obsession_core::func::{Arg, Block, Literal, Origin, PortKind, Value};

use crate::graph::{View, curve, draw_grid, fit, pin_color};
use crate::theme;

const NODE_W: f32 = 220.0;
const HEAD: f32 = 30.0;
const ROW: f32 = 22.0;
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.0;
pub const PANEL_W: f32 = 330.0;

/// Where the editor was opened from: this decides the live values.
#[derive(Debug, Clone, PartialEq)]
pub enum Context {
    /// The function on the wire at this line of `_nodes.md`.
    Wire { line: usize },
    /// A `fn Name` node in the graph.
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sel {
    /// The inputs node: `in`, `out` and `param` lines.
    Entry,
    Step(usize),
}

#[derive(Debug, Clone)]
pub enum Message {
    // From the canvas.
    Moved(usize, Point),
    /// A wire was dropped on input `input` of `step`, from `source`. If it
    /// was pulled off another input, that one is cleared.
    Connect {
        step: usize,
        input: usize,
        source: String,
        from: Option<(usize, usize)>,
    },
    Disconnect(usize, usize),
    Select(Option<Sel>),
    Delete(usize),
    OpenPalette(Option<Point>),
    // From the panel.
    Back,
    Draft(usize, String),
    Submit(usize),
    ParamDraft(usize, String),
    ParamSubmit,
    HeaderDraft(String),
    HeaderSubmit,
    RemoveLine(usize),
    PaletteQuery(String),
    AddNode(&'static str),
    ClosePalette,
    ChangeFunction,
    TakeOff,
    Duplicate,
}

/// What the app should do after a message.
pub enum Outcome {
    None,
    /// Change the file that holds the function.
    Edit(Box<dyn FnOnce(&str) -> String>),
    /// Rewrite the wire's call (`Name(task: "x")`).
    Wire(String),
    Back,
    /// Pick another function for the wire.
    Change,
    /// Take the function off the wire.
    TakeOff,
    /// Copy this saved function into the graph's `## Functions`.
    Duplicate,
    Status(String),
}

pub struct Editor {
    pub name: String,
    pub origin: Origin,
    pub context: Context,
    pub block: Option<Block>,
    pub checked: Checked,
    pub run: Run,
    pub binds: Vec<Option<Bind>>,
    /// The values the wire gives each param.
    pub wire_args: Vec<Option<Literal>>,
    /// The wire's two ends, for the panel ("[[a]] → [[b]]").
    pub wire_label: Option<String>,
    pub selected: Option<Sel>,
    drafts: Vec<String>,
    param_drafts: Vec<String>,
    header_draft: String,
    palette: Option<Palette>,
    /// Bumped when a different function opens, so the view fits it.
    pub fit: u64,
    cache: Cache,
    drawn: Cell<Option<DrawKey>>,
    version: u64,
    /// Node positions (graph coordinates): the entry node, then each step.
    entry_pos: Point,
    step_pos: Vec<Point>,
}

struct Palette {
    query: String,
    at: Option<Point>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawKey {
    pan: Vector,
    zoom: f32,
    size: Size,
    version: u64,
    lifted: Option<usize>,
}

impl Editor {
    pub fn new(name: String, origin: Origin, context: Context) -> Self {
        Self {
            name,
            origin,
            context,
            block: None,
            checked: Checked::default(),
            run: Run::default(),
            binds: Vec::new(),
            wire_args: Vec::new(),
            wire_label: None,
            selected: None,
            drafts: Vec::new(),
            param_drafts: Vec::new(),
            header_draft: String::new(),
            palette: None,
            fit: 0,
            cache: Cache::new(),
            drawn: Cell::new(None),
            version: 0,
            entry_pos: Point::ORIGIN,
            step_pos: Vec::new(),
        }
    }

    /// Takes the function's current text (`block`), the wire's values and
    /// the wired files; checks and runs it.
    pub fn refresh(
        &mut self,
        block: Option<Block>,
        wire_args: &[Arg],
        binds: Vec<Option<Bind>>,
        team: Option<&obsession_core::status::Format>,
    ) {
        self.binds = binds;
        self.block = block;
        let Some(f) = self.block.as_ref().map(|b| b.function.clone()) else {
            self.checked = Checked::default();
            self.run = Run::default();
            self.version += 1;
            return;
        };
        let f = &f;
        self.name = f.name.clone();
        self.checked = obsession_core::func::check::check(f);
        // The wire's values, by param.
        self.wire_args = vec![None; f.params.len()];
        for (k, arg) in wire_args.iter().enumerate() {
            let index = match &arg.name {
                Some(n) => f.params.iter().position(|p| p.name.eq_ignore_ascii_case(n)),
                None => (k < f.params.len()).then_some(k),
            };
            if let (Some(i), Value::Lit(l)) = (index, &arg.value) {
                self.wire_args[i] = Some(l.clone());
            }
        }
        self.run = eval::eval(f, &self.checked, &self.binds, &self.wire_args, team);
        self.layout();
        if let Some(Sel::Step(i)) = self.selected
            && i >= f.steps.len()
        {
            self.selected = None;
        }
        self.reset_drafts();
        self.version += 1;
    }

    fn reset_drafts(&mut self) {
        let Some(block) = &self.block else { return };
        let f = &block.function;
        self.drafts = match self.selected {
            Some(Sel::Step(i)) => {
                let bound = self.checked.bound.get(i).cloned().unwrap_or_default();
                let n = library::find(&f.steps[i].op).map_or(0, |s| s.inputs.len());
                (0..n).map(|k| bound.get(k).cloned().flatten().map_or(String::new(), |v| v.write())).collect()
            }
            _ => Vec::new(),
        };
        self.param_drafts = self
            .wire_args
            .iter()
            .map(|v| v.as_ref().map_or(String::new(), Literal::write))
            .collect();
    }

    /// Positions from `@x,y`; the rest placed in columns by how many steps
    /// feed them.
    fn layout(&mut self) {
        let Some(block) = &self.block else { return };
        let f = &block.function;
        let n = f.steps.len();
        let mut depth = vec![0usize; n];
        for &i in &self.checked.order {
            for v in self.checked.bound.get(i).into_iter().flatten().flatten() {
                if let Value::Ref(r) = v
                    && let Some(Source::Step(j)) = self.checked.source(r)
                {
                    depth[i] = depth[i].max(depth[j] + 1);
                }
            }
        }
        let mut column_y: Vec<f32> = Vec::new();
        self.step_pos = (0..n)
            .map(|i| match f.steps[i].pos {
                Some((x, y)) => Point::new(x, y),
                None => {
                    let d = depth[i];
                    if column_y.len() <= d {
                        column_y.resize(d + 1, 40.0);
                    }
                    let y = column_y[d];
                    column_y[d] += self.step_size(i).height + 30.0;
                    Point::new(40.0 + d as f32 * (NODE_W + 60.0), y)
                }
            })
            .collect();
        let min_x = self.step_pos.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        let min_y = self.step_pos.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        self.entry_pos = if n == 0 {
            Point::new(-NODE_W - 60.0, 40.0)
        } else {
            Point::new(min_x - NODE_W - 80.0, min_y)
        };
    }

    fn inputs_of(&self, i: usize) -> &'static [library::Input] {
        self.block
            .as_ref()
            .and_then(|b| b.function.steps.get(i))
            .and_then(|s| library::find(&s.op))
            .map_or(&[], |s| s.inputs)
    }

    fn step_size(&self, i: usize) -> Size {
        Size::new(NODE_W, HEAD + self.inputs_of(i).len() as f32 * ROW + 8.0)
    }

    /// The entry node's rows: (source name, label, type).
    fn entry_rows(&self) -> Vec<(String, String, Option<Type>)> {
        let Some(block) = &self.block else {
            return Vec::new();
        };
        let f = &block.function;
        let mut rows = Vec::new();
        for p in &f.inputs {
            rows.push((p.name.clone(), format!("in {}: {}", p.name, p.kind.name()), Some(Type::from(p.kind))));
        }
        for p in &f.outputs {
            rows.push((p.name.clone(), format!("out {}: {}", p.name, p.kind.name()), Some(Type::from(p.kind))));
        }
        for (k, p) in f.params.iter().enumerate() {
            let value = self.wire_args.get(k).cloned().flatten().unwrap_or_else(|| p.default.clone());
            rows.push((p.name.clone(), format!("param {} = {}", p.name, value.write()), Some(p.default.ty())));
        }
        rows
    }

    fn entry_size(&self) -> Size {
        Size::new(NODE_W, HEAD + self.entry_rows().len().max(1) as f32 * ROW + 8.0)
    }

    fn step_rect(&self, i: usize, state: &State) -> Rectangle {
        let pos = match state.interaction {
            Interaction::Dragging { step, at, .. } if step == i => at,
            _ => self.step_pos.get(i).copied().unwrap_or(Point::ORIGIN),
        };
        Rectangle::new(pos, self.step_size(i))
    }

    fn entry_rect(&self) -> Rectangle {
        Rectangle::new(self.entry_pos, self.entry_size())
    }

    fn in_pin(&self, i: usize, k: usize, state: &State) -> Point {
        let r = self.step_rect(i, state);
        Point::new(r.x, r.y + HEAD + k as f32 * ROW + ROW / 2.0)
    }

    fn out_pin(&self, i: usize, state: &State) -> Point {
        let r = self.step_rect(i, state);
        Point::new(r.x + r.width, r.y + HEAD / 2.0)
    }

    fn entry_pin(&self, row: usize) -> Point {
        let r = self.entry_rect();
        Point::new(r.x + r.width, r.y + HEAD + row as f32 * ROW + ROW / 2.0)
    }

    /// Where the wire for a source name starts.
    fn source_pin(&self, name: &str, state: &State) -> Option<Point> {
        match self.checked.source(name)? {
            Source::Step(j) => Some(self.out_pin(j, state)),
            _ => {
                let row = self.entry_rows().iter().position(|(n, _, _)| n == name)?;
                Some(self.entry_pin(row))
            }
        }
    }

    fn source_type(&self, name: &str) -> Option<Type> {
        let f = &self.block.as_ref()?.function;
        match self.checked.source(name)? {
            Source::In(p) => Some(Type::from(f.inputs[p].kind)),
            Source::Out(p) => Some(Type::from(f.outputs[p].kind)),
            Source::Param(p) => Some(f.params[p].default.ty()),
            Source::Step(j) => self.checked.types.get(j).copied().flatten(),
        }
    }

    /// The live value of a source, for hovering.
    fn source_value(&self, name: &str) -> String {
        let Some(f) = self.block.as_ref().map(|b| &b.function) else {
            return String::new();
        };
        let ins = f.inputs.len();
        let port = |p: usize| match self.binds.get(p).and_then(Option::as_ref) {
            Some(b) => match &b.sub {
                Some(sub) => format!("{}#{sub}", b.rel.trim_end_matches(".md")),
                None => b.rel.trim_end_matches(".md").to_owned(),
            },
            None => "not wired (no live value)".to_owned(),
        };
        match self.checked.source(name) {
            Some(Source::In(p)) => port(p),
            Some(Source::Out(p)) => port(ins + p),
            Some(Source::Param(p)) => self
                .wire_args
                .get(p)
                .cloned()
                .flatten()
                .unwrap_or_else(|| f.params[p].default.clone())
                .write(),
            Some(Source::Step(j)) => self.step_value(j),
            None => String::new(),
        }
    }

    fn step_value(&self, j: usize) -> String {
        match self.run.values.get(j) {
            Some(Ok(v)) => v.show(&self.binds),
            Some(Err(e)) => format!("no value: {e}"),
            None => String::new(),
        }
    }

    fn hit(&self, state: &State, view: View, p: Point) -> Option<Hit> {
        let g = view.graph(p);
        let near = |q: Point| view.screen(q).distance(p) <= 9.0;
        let Some(block) = &self.block else { return None };
        let f = &block.function;
        for i in (0..f.steps.len()).rev() {
            let spec = library::find(&f.steps[i].op);
            if spec.is_some_and(|s| !s.is_action()) && near(self.out_pin(i, state)) {
                return f.steps[i].id.clone().map(|id| Hit::Out(id, self.out_pin(i, state)));
            }
            for k in 0..self.inputs_of(i).len() {
                if near(self.in_pin(i, k, state)) {
                    return Some(Hit::In(i, k));
                }
            }
            if self.step_rect(i, state).contains(g) {
                return Some(Hit::Step(i));
            }
        }
        for (row, (name, _, _)) in self.entry_rows().iter().enumerate() {
            if near(self.entry_pin(row)) {
                return Some(Hit::Out(name.clone(), self.entry_pin(row)));
            }
        }
        if self.entry_rect().contains(g) {
            return Some(Hit::Entry);
        }
        None
    }

    fn fit_view(&self, state: &mut State, bounds: Size) {
        let mut min = self.entry_pos;
        let e = self.entry_size();
        let mut max = Point::new(self.entry_pos.x + e.width, self.entry_pos.y + e.height);
        for (i, p) in self.step_pos.iter().enumerate() {
            let s = self.step_size(i);
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x + s.width), max.y.max(p.y + s.height));
        }
        let margin = 50.0;
        let zoom = ((bounds.width - 2.0 * margin) / (max.x - min.x).max(1.0))
            .min((bounds.height - 2.0 * margin) / (max.y - min.y).max(1.0))
            .clamp(MIN_ZOOM, 1.0);
        let centre = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        state.zoom = zoom;
        state.pan = Vector::new(bounds.width / 2.0 - centre.x * zoom, bounds.height / 2.0 - centre.y * zoom);
    }

    /// Handles a message; returns what the app should do.
    pub fn update(&mut self, message: Message) -> Outcome {
        let Some(block) = self.block.clone() else {
            return match message {
                Message::Back => Outcome::Back,
                _ => Outcome::None,
            };
        };
        let f = &block.function;
        match message {
            Message::Moved(i, at) => {
                let Some(step) = f.steps.get(i).cloned() else {
                    return Outcome::None;
                };
                if let Some(p) = self.step_pos.get_mut(i) {
                    *p = at;
                }
                self.version += 1;
                Outcome::Edit(Box::new(move |text| obsession_core::func::edit::set_pos(text, &step, at.x, at.y)))
            }
            Message::Connect { step, input, source, from } => {
                let Some(target) = f.steps.get(step).cloned() else {
                    return Outcome::None;
                };
                let want = self.inputs_of(step).get(input).map(|i| (i.name, i.want));
                if let (Some((name, want)), Some(ty)) = (want, self.source_type(&source))
                    && !want.accepts(ty)
                {
                    return Outcome::Status(format!(
                        "⚠ {}'s `{name}` wants {}, and `{source}` is {}",
                        target.op,
                        want.describe(),
                        ty.name()
                    ));
                }
                if target.id.as_deref() == Some(source.as_str()) {
                    return Outcome::Status("⚠ A node can't feed itself".into());
                }
                let old = from.filter(|&(s, k)| (s, k) != (step, input)).and_then(|(s, k)| f.steps.get(s).cloned().map(|st| (st, k)));
                Outcome::Edit(Box::new(move |text| {
                    use obsession_core::func::edit::set_arg;
                    let text = set_arg(text, &target, input, Some(Value::Ref(source)));
                    match old {
                        // The old step's line is unchanged by the first edit
                        // unless it's the same step.
                        Some((old_step, k)) if old_step.line != target.line => set_arg(&text, &old_step, k, None),
                        Some((_, k)) => {
                            let reread = obsession_core::func::blocks(&text)
                                .into_iter()
                                .flat_map(|b| b.function.steps)
                                .find(|s| s.line == target.line);
                            match reread {
                                Some(s) => set_arg(&text, &s, k, None),
                                None => text,
                            }
                        }
                        None => text,
                    }
                }))
            }
            Message::Disconnect(i, k) => {
                let Some(step) = f.steps.get(i).cloned() else {
                    return Outcome::None;
                };
                Outcome::Edit(Box::new(move |text| obsession_core::func::edit::set_arg(text, &step, k, None)))
            }
            Message::Select(sel) => {
                self.selected = sel;
                self.palette = None;
                self.reset_drafts();
                self.version += 1;
                Outcome::None
            }
            Message::Delete(i) => {
                self.selected = None;
                Outcome::Edit(Box::new(move |text| obsession_core::func::edit::remove_step(text, &block, i)))
            }
            Message::OpenPalette(at) => {
                self.palette = Some(Palette {
                    query: String::new(),
                    at,
                });
                Outcome::None
            }
            Message::Back => Outcome::Back,
            Message::Draft(k, value) => {
                if let Some(d) = self.drafts.get_mut(k) {
                    *d = value;
                }
                Outcome::None
            }
            Message::Submit(k) => {
                let Some(Sel::Step(i)) = self.selected else {
                    return Outcome::None;
                };
                let Some(step) = f.steps.get(i).cloned() else {
                    return Outcome::None;
                };
                let draft = self.drafts.get(k).cloned().unwrap_or_default();
                let value = if draft.trim().is_empty() {
                    None
                } else {
                    match obsession_core::func::parse_value(&draft) {
                        Ok(v) => Some(v),
                        Err(e) => return Outcome::Status(format!("⚠ {e}")),
                    }
                };
                Outcome::Edit(Box::new(move |text| obsession_core::func::edit::set_arg(text, &step, k, value)))
            }
            Message::ParamDraft(k, value) => {
                if let Some(d) = self.param_drafts.get_mut(k) {
                    *d = value;
                }
                Outcome::None
            }
            Message::ParamSubmit => {
                // Every param field is written back together, in order.
                let mut args = Vec::new();
                for (k, p) in f.params.iter().enumerate() {
                    let draft = self.param_drafts.get(k).map_or("", |d| d.trim());
                    if draft.is_empty() {
                        continue;
                    }
                    match obsession_core::func::parse_value(draft) {
                        Ok(Value::Lit(l)) if l == p.default => {}
                        Ok(Value::Lit(l)) => args.push(Arg {
                            name: Some(p.name.clone()),
                            value: Value::Lit(l),
                        }),
                        Ok(Value::Ref(_)) => {
                            // Bare words are taken as text.
                            args.push(Arg {
                                name: Some(p.name.clone()),
                                value: Value::Lit(Literal::Text(draft.to_owned())),
                            });
                        }
                        Err(e) => return Outcome::Status(format!("⚠ {}: {e}", p.name)),
                    }
                }
                Outcome::Wire(obsession_core::func::edit::wire_call(&f.name, &args))
            }
            Message::HeaderDraft(value) => {
                self.header_draft = value;
                Outcome::None
            }
            Message::HeaderSubmit => {
                let line = self.header_draft.trim().to_owned();
                let parsed = obsession_core::func::parse(&format!("fn X\n{line}\n"));
                let ok = parsed.errors.is_empty()
                    && parsed.steps.is_empty()
                    && (parsed.inputs.len() + parsed.outputs.len() + parsed.params.len()) == 1;
                if !ok {
                    let why = parsed.errors.first().map_or("Write `in name: File`, `out name: File` or `param name = \"value\"`".to_owned(), |e| e.1.clone());
                    return Outcome::Status(format!("⚠ {why}"));
                }
                self.header_draft.clear();
                Outcome::Edit(Box::new(move |text| obsession_core::func::edit::add_header(text, &block, &line)))
            }
            Message::RemoveLine(line) => Outcome::Edit(Box::new(move |text| obsession_core::func::edit::remove_line(text, line))),
            Message::PaletteQuery(q) => {
                if let Some(p) = &mut self.palette {
                    p.query = q;
                }
                Outcome::None
            }
            Message::AddNode(op) => {
                let at = self.palette.take().and_then(|p| p.at).unwrap_or_else(|| {
                    // Right of the rightmost node.
                    let x = self.step_pos.iter().map(|p| p.x).fold(self.entry_pos.x, f32::max) + NODE_W + 60.0;
                    let y = self.step_pos.iter().map(|p| p.y).fold(self.entry_pos.y, f32::min);
                    Point::new(x, y)
                });
                self.selected = Some(Sel::Step(f.steps.len()));
                Outcome::Edit(Box::new(move |text| obsession_core::func::edit::add_step(text, &block, op, at.x, at.y).0))
            }
            Message::ClosePalette => {
                self.palette = None;
                Outcome::None
            }
            Message::ChangeFunction => Outcome::Change,
            Message::TakeOff => Outcome::TakeOff,
            Message::Duplicate => Outcome::Duplicate,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        row![
            Canvas::new(self).width(Fill).height(Fill),
            rule::vertical(1),
            container(scrollable(self.panel()).height(Fill))
                .width(Length::Fixed(PANEL_W))
                .height(Fill)
                .style(theme::panel),
        ]
        .into()
    }

    fn panel(&self) -> Element<'_, Message> {
        let small = |s: String| text(s).size(theme::SMALL).color(theme::MUTED);
        let heading = |s: &'static str| text(s).size(theme::SMALL).color(theme::FAINT);
        let tool = |label: &'static str, msg: Message| {
            button(text(label).size(theme::SMALL))
                .on_press(msg)
                .padding([4, 10])
                .style(theme::tool(false))
        };
        let mut col = column![
            row![
                tool("← Graph", Message::Back),
                text(format!("ƒ {}", self.name))
                    .size(16)
                    .font(Font {
                        weight: iced::font::Weight::Semibold,
                        ..theme::UI_FONT
                    })
                    .color(theme::MENTION),
            ]
            .spacing(10)
            .align_y(iced::Center),
        ]
        .spacing(10)
        .padding(14);

        col = col.push(small(match self.origin {
            Origin::Local => "In this graph's ## Functions".to_owned(),
            Origin::Saved => "Saved in _funct.md: changes reach every wire that uses it".to_owned(),
        }));
        if self.origin == Origin::Saved {
            col = col.push(tool("Duplicate into this graph", Message::Duplicate));
        }

        let Some(block) = &self.block else {
            return col
                .push(text(format!("⚠ There's no function called {} any more", self.name)).size(theme::SMALL).color(theme::WARNING))
                .into();
        };
        let f = &block.function;

        // Problems, or ready.
        if self.checked.ok() {
            col = col.push(text("✓ Checks out: its wires are solid").size(theme::SMALL).color(theme::SUCCESS));
        } else {
            let mut problems = column![].spacing(4);
            for p in self.checked.problems.iter().take(8) {
                let line = text(format!("⚠ {}", p.message)).size(theme::SMALL).color(theme::WARNING);
                problems = problems.push(match p.step {
                    Some(i) => Element::from(
                        button(line)
                            .on_press(Message::Select(Some(Sel::Step(i))))
                            .padding(0)
                            .style(theme::row(false)),
                    ),
                    None => line.into(),
                });
            }
            col = col.push(problems);
        }

        // Where the live values come from.
        if let Some(label) = &self.wire_label {
            col = col.push(small(format!("Live values from {label}")));
        }
        if self.binds.iter().all(Option::is_none) {
            col = col.push(small("Not wired to any notes yet: no live values".into()));
        }

        // The wire's values.
        if matches!(self.context, Context::Wire { .. }) {
            col = col.push(rule::horizontal(1)).push(heading("ON THIS WIRE"));
            for (k, p) in f.params.iter().enumerate() {
                col = col.push(
                    column![
                        small(format!("{} · {} · {} by default", p.name, p.default.ty().name(), p.default.write())),
                        text_input(&p.default.write(), self.param_drafts.get(k).map_or("", String::as_str))
                            .on_input(move |v| Message::ParamDraft(k, v))
                            .on_submit(Message::ParamSubmit)
                            .padding(6)
                            .size(theme::SMALL)
                            .style(theme::search),
                    ]
                    .spacing(3),
                );
            }
            if !f.params.is_empty() {
                col = col.push(small("Press Enter to save the wire's values".into()));
            }
            col = col.push(row![tool("Change function", Message::ChangeFunction), tool("Take off wire", Message::TakeOff)].spacing(8));
        }

        col = col.push(rule::horizontal(1));

        // The node library.
        if let Some(palette) = &self.palette {
            col = col.push(
                row![heading("ADD A NODE"), Space::new().width(Fill), tool("Close", Message::ClosePalette)].align_y(iced::Center),
            );
            col = col.push(
                text_input("Search nodes…", &palette.query)
                    .on_input(Message::PaletteQuery)
                    .padding(6)
                    .size(theme::SMALL)
                    .style(theme::search),
            );
            let q = palette.query.to_lowercase();
            for category in Category::ALL {
                let specs: Vec<&'static Spec> = library::NODES
                    .iter()
                    .filter(|s| s.category == category)
                    .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.help.to_lowercase().contains(&q))
                    .collect();
                if specs.is_empty() {
                    continue;
                }
                col = col.push(heading(category.name()));
                for spec in specs {
                    col = col.push(
                        button(
                            column![
                                text(spec.name).size(theme::SMALL).color(if spec.is_action() { theme::CODE } else { theme::TEXT }),
                                text(spec.help).size(11).color(theme::FAINT),
                            ]
                            .spacing(1),
                        )
                        .on_press(Message::AddNode(spec.name))
                        .padding([4, 6])
                        .width(Fill)
                        .style(theme::row(false)),
                    );
                }
            }
            return col.into();
        }
        col = col.push(row![tool("+ Node", Message::OpenPalette(None)), small("or double-click the canvas".into())].spacing(8).align_y(iced::Center));

        match self.selected {
            Some(Sel::Step(i)) if i < f.steps.len() => {
                let step = &f.steps[i];
                let Some(spec) = library::find(&step.op) else {
                    return col.push(text(format!("⚠ There's no node called {}", step.op)).size(theme::SMALL).color(theme::WARNING)).into();
                };
                let title = match &step.id {
                    Some(id) => format!("{id} = {}", spec.name),
                    None => spec.name.to_owned(),
                };
                col = col
                    .push(text(title).size(theme::TEXT_SIZE).color(theme::HEADING))
                    .push(text(spec.help).size(theme::SMALL).color(theme::MUTED));
                for (k, input) in spec.inputs.iter().enumerate() {
                    let placeholder = match input.default {
                        Def::Required => "required: a name, \"text\", a number…".to_owned(),
                        d => format!("{} by default", d.literal().map_or(String::new(), |l| l.write())),
                    };
                    let bound = self.checked.bound.get(i).and_then(|b| b.get(k)).cloned().flatten();
                    let live = match &bound {
                        Some(Value::Ref(r)) => Some(self.source_value(r)),
                        _ => None,
                    };
                    let mut field = column![
                        small(format!("{} · {}", input.name, input.want.describe())),
                        text_input(&placeholder, self.drafts.get(k).map_or("", String::as_str))
                            .on_input(move |v| Message::Draft(k, v))
                            .on_submit(Message::Submit(k))
                            .padding(6)
                            .size(theme::SMALL)
                            .style(theme::search),
                    ]
                    .spacing(3);
                    if let Some(live) = live {
                        field = field.push(text(format!("= {live}")).size(11).color(theme::FAINT));
                    }
                    col = col.push(field);
                }
                col = col.push(small("Press Enter to save a value; leave it empty for the default".into()));
                if !spec.is_action() {
                    col = col.push(text(format!("Gives: {}", self.step_value(i))).size(theme::SMALL).color(theme::TEXT));
                } else {
                    col = col.push(text(self.step_value(i)).size(theme::SMALL).color(theme::TEXT));
                }
                for p in self.checked.step_problems(i) {
                    col = col.push(text(format!("⚠ {}", p.message)).size(theme::SMALL).color(theme::WARNING));
                }
                col = col.push(tool("Delete node", Message::Delete(i)));
            }
            Some(Sel::Entry) => {
                col = col.push(text("Inputs, outputs and params").size(theme::TEXT_SIZE).color(theme::HEADING));
                let lines = f
                    .inputs
                    .iter()
                    .map(|p| (p.line, format!("in {}: {}", p.name, p.kind.name())))
                    .chain(f.outputs.iter().map(|p| (p.line, format!("out {}: {}", p.name, p.kind.name()))))
                    .chain(f.params.iter().map(|p| (p.line, format!("param {} = {}", p.name, p.default.write()))));
                for (line, label) in lines {
                    col = col.push(
                        row![
                            text(label).size(theme::SMALL).color(theme::TEXT).width(Fill),
                            button(text("×").size(theme::SMALL)).on_press(Message::RemoveLine(line)).padding([0, 8]).style(theme::tool(false)),
                        ]
                        .align_y(iced::Center),
                    );
                }
                col = col
                    .push(
                        text_input("in name: File   ·   out name: Task   ·   param name = \"x\"", &self.header_draft)
                            .on_input(Message::HeaderDraft)
                            .on_submit(Message::HeaderSubmit)
                            .padding(6)
                            .size(theme::SMALL)
                            .style(theme::search),
                    )
                    .push(small("One in and one out: it sits on a wire. More: it's a node in the graph (fn Name) wired by its pins.".into()));
            }
            _ => {
                col = col.push(small("Click a node to set its values. Drag from a pin on the right of a node to a pin on the left of another to connect them; drag a connection off to remove it. Hover a pin for its live value.".into()));
            }
        }
        col.into()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Hit {
    Entry,
    Step(usize),
    /// An output pin: the source name and where it is.
    Out(String, Point),
    /// Input `k` of step `i`.
    In(usize, usize),
}

#[derive(Debug, Default)]
pub struct State {
    pan: Vector,
    zoom: f32,
    interaction: Interaction,
    hover: Option<Hit>,
    cursor: Point,
    fitted: Option<u64>,
    last_click: Option<Instant>,
}

#[derive(Debug, Default, Clone)]
enum Interaction {
    #[default]
    Idle,
    Panning {
        from: Point,
        pan: Vector,
    },
    Dragging {
        step: usize,
        grab: Vector,
        at: Point,
        moved: bool,
    },
    Wiring {
        source: String,
        from: Point,
        to: Point,
        /// The input it was pulled off.
        detached: Option<(usize, usize)>,
    },
}

impl canvas::Program<Message> for Editor {
    type State = State;

    fn update(&self, state: &mut State, event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Message>> {
        if state.fitted != Some(self.fit) {
            self.fit_view(state, bounds.size());
            state.fitted = Some(self.fit);
            state.interaction = Interaction::Idle;
            state.hover = None;
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
                match self.hit(state, view, p) {
                    Some(Hit::Out(source, at)) => {
                        state.interaction = Interaction::Wiring {
                            source,
                            from: at,
                            to: view.graph(p),
                            detached: None,
                        };
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    Some(Hit::In(i, k)) => {
                        let bound = self.checked.bound.get(i).and_then(|b| b.get(k)).cloned().flatten();
                        if let Some(Value::Ref(source)) = bound
                            && let Some(from) = self.source_pin(&source, state)
                        {
                            state.interaction = Interaction::Wiring {
                                source,
                                from,
                                to: view.graph(p),
                                detached: Some((i, k)),
                            };
                            return Some(canvas::Action::request_redraw().and_capture());
                        }
                        Some(canvas::Action::publish(Message::Select(Some(Sel::Step(i)))).and_capture())
                    }
                    Some(Hit::Step(i)) => {
                        let pos = self.step_pos.get(i).copied().unwrap_or(Point::ORIGIN);
                        state.interaction = Interaction::Dragging {
                            step: i,
                            grab: view.graph(p) - pos,
                            at: pos,
                            moved: false,
                        };
                        Some(canvas::Action::publish(Message::Select(Some(Sel::Step(i)))).and_capture())
                    }
                    Some(Hit::Entry) => Some(canvas::Action::publish(Message::Select(Some(Sel::Entry))).and_capture()),
                    None => {
                        let now = Instant::now();
                        let double = state.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 450);
                        state.last_click = Some(now);
                        if double {
                            state.last_click = None;
                            return Some(canvas::Action::publish(Message::OpenPalette(Some(view.graph(p)))).and_capture());
                        }
                        state.interaction = Interaction::Panning { from: p, pan: state.pan };
                        let deselect = self.selected.is_some().then_some(Message::Select(None));
                        Some(match deselect {
                            Some(m) => canvas::Action::publish(m).and_capture(),
                            None => canvas::Action::capture(),
                        })
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = cursor
                    .position_in(bounds)
                    .or_else(|| cursor.position().map(|c| Point::new(c.x - bounds.x, c.y - bounds.y)))?;
                state.cursor = p;
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
                        let pin = matches!(hover, Some(Hit::Out(..) | Hit::In(..)));
                        if hover != state.hover || pin {
                            state.hover = hover;
                            return Some(canvas::Action::request_redraw());
                        }
                        None
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left | mouse::Button::Middle)) => {
                let done = std::mem::take(&mut state.interaction);
                // A pin's value box follows the cursor, not the pin a drag
                // started from.
                if !matches!(done, Interaction::Idle) {
                    state.hover = point.and_then(|p| self.hit(state, view, p));
                }
                match done {
                    Interaction::Dragging { step, at, moved: true, .. } => Some(canvas::Action::publish(Message::Moved(step, at))),
                    Interaction::Wiring { source, detached, .. } => {
                        let target = point.and_then(|p| self.hit(state, view, p));
                        Some(match target {
                            Some(Hit::In(i, k)) => canvas::Action::publish(Message::Connect {
                                step: i,
                                input: k,
                                source,
                                from: detached,
                            }),
                            _ => match detached {
                                Some((i, k)) => canvas::Action::publish(Message::Disconnect(i, k)),
                                None => canvas::Action::request_redraw(),
                            },
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
                let g = view.graph(p);
                state.zoom = zoom;
                state.pan = Vector::new(p.x - g.x * zoom, p.y - g.y * zoom);
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                use keyboard::key::Named;
                match (key.as_ref(), self.selected) {
                    (keyboard::Key::Named(Named::Delete), Some(Sel::Step(i))) => Some(canvas::Action::publish(Message::Delete(i))),
                    (keyboard::Key::Named(Named::Escape), Some(_)) => Some(canvas::Action::publish(Message::Select(None))),
                    (keyboard::Key::Named(Named::Escape), None) if cursor.is_over(bounds) => Some(canvas::Action::publish(Message::Back)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn draw(&self, state: &State, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let zoom = if state.zoom == 0.0 { 1.0 } else { state.zoom };
        let view = View { pan: state.pan, zoom };
        let lifted = match state.interaction {
            Interaction::Dragging { step, .. } => Some(step),
            _ => None,
        };
        let key = DrawKey {
            pan: state.pan,
            zoom,
            size: bounds.size(),
            version: self.version,
            lifted,
        };
        if self.drawn.get() != Some(key) {
            self.cache.clear();
            self.drawn.set(Some(key));
        }
        let Some(block) = &self.block else {
            let mut frame = Frame::new(renderer, bounds.size());
            draw_grid(&mut frame, view);
            return vec![frame.into_geometry()];
        };
        let f = &block.function;
        let touches = |i: usize| -> bool {
            Some(i) == lifted
                || lifted.is_some_and(|l| {
                    f.steps[l].id.as_deref().is_some_and(|id| {
                        self.checked
                            .bound
                            .get(i)
                            .is_some_and(|b| b.iter().flatten().any(|v| matches!(v, Value::Ref(r) if r == id)))
                    })
                })
        };

        let still = self.cache.draw(renderer, bounds.size(), |frame| {
            draw_grid(frame, view);
            self.draw_entry(frame, view);
            for i in 0..f.steps.len() {
                if !touches(i) {
                    self.draw_wires_into(frame, view, state, i);
                }
            }
            for i in 0..f.steps.len() {
                if Some(i) != lifted {
                    self.draw_step(frame, view, state, i);
                }
            }
        });

        let mut frame = Frame::new(renderer, bounds.size());
        for i in 0..f.steps.len() {
            if touches(i) {
                self.draw_wires_into(&mut frame, view, state, i);
            }
        }
        if let Some(i) = lifted {
            self.draw_step(&mut frame, view, state, i);
        }
        if let Some(Sel::Step(i)) = self.selected
            && i < f.steps.len()
        {
            let r = self.step_rect(i, state);
            frame.stroke(
                &Path::rounded_rectangle(view.screen(r.position()), r.size() * zoom, (7.0 * zoom).into()),
                Stroke::default().with_color(theme::ACCENT).with_width(1.5),
            );
        }
        if self.selected == Some(Sel::Entry) {
            let r = self.entry_rect();
            frame.stroke(
                &Path::rounded_rectangle(view.screen(r.position()), r.size() * zoom, (7.0 * zoom).into()),
                Stroke::default().with_color(theme::ACCENT).with_width(1.5),
            );
        }
        if let Interaction::Wiring { from, to, source, .. } = &state.interaction {
            let color = self.source_type(source).map_or(theme::ACCENT, type_color);
            frame.stroke(&curve(view, *from, *to), Stroke::default().with_color(color).with_width(2.0));
        }
        // The hovered pin's live value.
        let tip = match &state.hover {
            Some(Hit::Out(source, at)) => Some((*at, self.source_type(source), self.source_value(source))),
            Some(Hit::In(i, k)) => {
                let input = self.inputs_of(*i).get(*k);
                let bound = self.checked.bound.get(*i).and_then(|b| b.get(*k)).cloned().flatten();
                let value = match &bound {
                    Some(Value::Ref(r)) => self.source_value(r),
                    Some(Value::Lit(l)) => l.write(),
                    None => input
                        .and_then(|i| i.default.literal())
                        .map_or("not set".to_owned(), |l| format!("{} (default)", l.write())),
                };
                let want = input.map_or(String::new(), |i| format!("{} · wants {}", i.name, i.want.describe()));
                Some((self.in_pin(*i, *k, state), None, format!("{want}\n{value}")))
            }
            _ => None,
        };
        if let Some((at, ty, value)) = tip
            && matches!(state.interaction, Interaction::Idle)
        {
            let label = match ty {
                Some(t) => format!("{}\n{value}", t.name()),
                None => value,
            };
            let lines: Vec<&str> = label.lines().collect();
            let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).min(64) as f32 * 6.6 + 16.0;
            let height = lines.len() as f32 * 16.0 + 10.0;
            let p = view.screen(at);
            let origin = Point::new(
                (p.x + 12.0).min(bounds.width - width - 4.0).max(4.0),
                (p.y + 10.0).min(bounds.height - height - 4.0).max(4.0),
            );
            let rect = Path::rounded_rectangle(origin, Size::new(width, height), 5.0.into());
            frame.fill(&rect, theme::SELECTED);
            frame.stroke(&rect, Stroke::default().with_color(theme::LINE).with_width(1.0));
            for (n, line) in lines.iter().enumerate() {
                frame.fill_text(Text {
                    content: fit(line, width - 12.0, 12.0),
                    position: Point::new(origin.x + 8.0, origin.y + 5.0 + n as f32 * 16.0),
                    color: if n == 0 { theme::MUTED } else { theme::TEXT },
                    size: Pixels(12.0),
                    font: theme::UI_FONT,
                    ..Text::default()
                });
            }
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
            Some(Hit::Out(..) | Hit::In(..)) => mouse::Interaction::Crosshair,
            Some(Hit::Step(_)) => mouse::Interaction::Grab,
            Some(Hit::Entry) => mouse::Interaction::Pointer,
            None => mouse::Interaction::None,
        }
    }
}

/// A wire's colour by the type it carries.
pub fn type_color(t: Type) -> Color {
    match t {
        Type::File => pin_color(PortKind::File),
        Type::Section => pin_color(PortKind::Section),
        Type::Task | Type::TaskList => pin_color(PortKind::Task),
        Type::Text | Type::TextList => theme::TEXT,
        Type::Number => theme::CODE,
        Type::Bool => theme::DANGER,
    }
}

fn want_color(w: Want) -> Color {
    match w {
        Want::Text => theme::TEXT,
        Want::Number => theme::CODE,
        Want::Bool => theme::DANGER,
        Want::Task => pin_color(PortKind::Task),
        Want::File => pin_color(PortKind::File),
        Want::TaskList | Want::TextOrTasks => pin_color(PortKind::Task),
        Want::List | Want::Any => theme::MUTED,
        Want::Target(_) => theme::CODE,
    }
}

fn say(frame: &mut Frame, content: String, at: Point, size: f32, color: Color, font: Font, right: bool) {
    frame.fill_text(Text {
        content,
        position: at,
        color,
        size: Pixels(size),
        font,
        align_x: if right {
            iced::widget::text::Alignment::Right
        } else {
            iced::widget::text::Alignment::Left
        },
        ..Text::default()
    });
}

impl Editor {
    fn draw_entry(&self, frame: &mut Frame, view: View) {
        let z = view.zoom;
        let r = self.entry_rect();
        let p = view.screen(r.position());
        let body = Path::rounded_rectangle(p, r.size() * z, (7.0 * z).into());
        frame.fill(&body, theme::RAISED);
        frame.stroke(&body, Stroke::default().with_color(theme::MENTION).with_width(1.0));
        let semibold = Font {
            weight: iced::font::Weight::Semibold,
            ..theme::UI_FONT
        };
        say(frame, "Inputs".into(), Point::new(p.x + 10.0 * z, p.y + 7.0 * z), 13.0 * z, theme::MENTION, semibold, false);
        for (row, (_, label, ty)) in self.entry_rows().iter().enumerate() {
            let pin = view.screen(self.entry_pin(row));
            frame.fill(&Path::circle(pin, 4.5 * z), ty.map_or(theme::MUTED, type_color));
            say(frame, fit(label, NODE_W - 30.0, 12.0), Point::new(p.x + 10.0 * z, pin.y - 7.5 * z), 12.0 * z, theme::TEXT, theme::UI_FONT, false);
        }
    }

    fn draw_wires_into(&self, frame: &mut Frame, view: View, state: &State, i: usize) {
        let Some(bound) = self.checked.bound.get(i) else { return };
        for (k, v) in bound.iter().enumerate() {
            let Some(Value::Ref(source)) = v else { continue };
            let Some(from) = self.source_pin(source, state) else { continue };
            if let Interaction::Wiring { detached: Some(d), .. } = state.interaction
                && d == (i, k)
            {
                continue;
            }
            let color = self.source_type(source).map_or(theme::MUTED, type_color);
            frame.stroke(
                &curve(view, from, self.in_pin(i, k, state)),
                Stroke::default().with_color(Color { a: 0.85, ..color }).with_width(1.8),
            );
        }
    }

    fn draw_step(&self, frame: &mut Frame, view: View, state: &State, i: usize) {
        let Some(block) = &self.block else { return };
        let step = &block.function.steps[i];
        let z = view.zoom;
        let r = self.step_rect(i, state);
        let p = view.screen(r.position());
        let size = r.size() * z;
        let spec = library::find(&step.op);
        let broken = self.checked.step_problems(i).next().is_some();
        let failed = matches!(self.run.values.get(i), Some(Err(_))) && self.binds.iter().any(Option::is_some);
        let body = Path::rounded_rectangle(p, size, (7.0 * z).into());
        frame.fill(&body, theme::RAISED);
        let ring = if broken {
            theme::WARNING
        } else if spec.is_some_and(Spec::is_action) {
            theme::CODE
        } else {
            theme::LINE
        };
        frame.stroke(&body, Stroke::default().with_color(ring).with_width(if broken { 1.4 } else { 1.0 }));
        // Header.
        let head_color = match spec.map(|s| s.category) {
            Some(Category::Actions) => theme::CODE,
            Some(Category::Logic) => theme::DANGER,
            Some(Category::Markdown) => theme::SUCCESS,
            Some(Category::Lists) => theme::TAG,
            _ => theme::LINK,
        };
        frame.fill(
            &Path::rounded_rectangle(p, Size::new(size.width, 3.0 * z), (7.0 * z).into()),
            head_color,
        );
        let title = match &step.id {
            Some(id) => format!("{id} = {}", spec.map_or(step.op.as_str(), |s| s.name)),
            None => spec.map_or(step.op.clone(), |s| s.name.to_owned()),
        };
        let semibold = Font {
            weight: iced::font::Weight::Semibold,
            ..theme::UI_FONT
        };
        let warn = if broken || failed { "⚠ " } else { "" };
        say(
            frame,
            fit(&format!("{warn}{title}"), NODE_W - 70.0, 13.0),
            Point::new(p.x + 10.0 * z, p.y + 8.0 * z),
            13.0 * z,
            if broken { theme::WARNING } else { theme::HEADING },
            semibold,
            false,
        );
        if let Some(spec) = spec
            && !spec.is_action()
        {
            let ty = self.checked.types.get(i).copied().flatten();
            let out = view.screen(self.out_pin(i, state));
            frame.fill(&Path::circle(out, 5.0 * z), ty.map_or(theme::MUTED, type_color));
            if let Some(t) = ty {
                say(frame, t.name().into(), Point::new(out.x - 10.0 * z, out.y - 7.0 * z), 11.0 * z, theme::FAINT, theme::UI_FONT, true);
            }
        }
        let Some(spec) = spec else { return };
        let bound = self.checked.bound.get(i);
        for (k, input) in spec.inputs.iter().enumerate() {
            let pin = view.screen(self.in_pin(i, k, state));
            let value = bound.and_then(|b| b.get(k)).cloned().flatten();
            let wired = matches!(value, Some(Value::Ref(_)));
            let fill = if wired { want_color(input.want) } else { theme::RAISED };
            frame.fill(&Path::circle(pin, 4.5 * z), fill);
            frame.stroke(&Path::circle(pin, 4.5 * z), Stroke::default().with_color(want_color(input.want)).with_width(1.2));
            say(frame, input.name.into(), Point::new(p.x + 12.0 * z, pin.y - 7.5 * z), 12.0 * z, theme::TEXT, theme::UI_FONT, false);
            let (shown, color) = match &value {
                Some(Value::Lit(l)) => (l.write(), theme::CODE),
                Some(Value::Ref(r)) => (r.clone(), theme::MUTED),
                None => match input.default {
                    Def::Required => ("needs a value".to_owned(), theme::WARNING),
                    d => (d.literal().map_or(String::new(), |l| l.write()), theme::FAINT),
                },
            };
            say(
                frame,
                fit(&shown, NODE_W * 0.55, 12.0),
                Point::new(p.x + size.width - 10.0 * z, pin.y - 7.5 * z),
                12.0 * z,
                color,
                theme::MONO_FONT,
                true,
            );
        }
    }
}

/// Picking a function for a wire, or a function node for the graph.
pub struct Picker {
    /// The functions to show: (name, where, fits, shape).
    pub choices: Vec<(String, Origin, bool, String)>,
    pub name: String,
    pub for_wire: bool,
    pub current: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PickerMessage {
    Pick(String),
    Name(String),
    Create,
    Remove,
    Close,
}

impl Picker {
    pub fn view(&self) -> Element<'_, PickerMessage> {
        let title = if self.for_wire { "Function on this wire" } else { "Add a function node" };
        let mut list = column![].spacing(2);
        for (name, origin, fits, shape) in &self.choices {
            let place = match origin {
                Origin::Local => "this graph",
                Origin::Saved => "_funct.md",
            };
            let current = self.current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name));
            let item = row![
                text(format!("ƒ {name}")).size(theme::TEXT_SIZE).color(if *fits { theme::TEXT } else { theme::FAINT }),
                text(shape.clone()).size(theme::SMALL).color(theme::MUTED),
                Space::new().width(Fill),
                text(place).size(theme::SMALL).color(theme::FAINT),
            ]
            .spacing(10)
            .align_y(iced::Center);
            let b = button(item).padding([6, 10]).width(Fill).style(theme::row(current));
            list = list.push(if *fits { b.on_press(PickerMessage::Pick(name.clone())) } else { b });
        }
        if self.choices.is_empty() {
            list = list.push(text("No functions yet").size(theme::SMALL).color(theme::MUTED));
        }
        let mut panel = column![
            text(title).size(theme::TEXT_SIZE).color(theme::HEADING),
            scrollable(list).height(Length::Shrink),
            row![
                text_input("New function's name…", &self.name)
                    .on_input(PickerMessage::Name)
                    .on_submit(PickerMessage::Create)
                    .padding(8)
                    .size(theme::SMALL)
                    .style(theme::search),
                button(text("Create").size(theme::SMALL))
                    .on_press(PickerMessage::Create)
                    .padding([6, 12])
                    .style(theme::tool(false)),
            ]
            .spacing(8)
            .align_y(iced::Center),
        ]
        .spacing(10);
        if self.for_wire && self.current.is_some() {
            panel = panel.push(
                button(text("Take the function off this wire").size(theme::SMALL))
                    .on_press(PickerMessage::Remove)
                    .padding([4, 10])
                    .style(theme::tool(false)),
            );
        }
        let popup = container(panel).padding(14).width(Length::Fixed(560.0)).style(theme::popup);
        opaque(
            iced::widget::mouse_area(
                container(opaque(popup))
                    .padding(iced::Padding::new(0.0).top(90.0))
                    .center_x(Fill)
                    .height(Fill)
                    .style(theme::backdrop),
            )
            .on_press(PickerMessage::Close),
        )
    }
}

/// What the cards and live values need from a wire end.
pub fn kind_of(sub: Option<&str>) -> PortKind {
    match sub {
        None => PortKind::File,
        Some(s) if s.starts_with('^') => PortKind::Task,
        Some(_) => PortKind::Section,
    }
}

/// A short description of a function's shape: "File → File".
pub fn shape(f: &obsession_core::func::Function) -> String {
    let side = |ports: &[obsession_core::func::Port]| -> String {
        match ports.len() {
            0 => "nothing".into(),
            1 => ports[0].kind.name().into(),
            n => format!("{n} pins"),
        }
    };
    format!("{} → {}", side(&f.inputs), side(&f.outputs))
}
