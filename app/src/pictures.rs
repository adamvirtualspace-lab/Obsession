//! Pictures in notes: `![[shot.png]]` and `![](shot.png)` drawn in Live,
//! Split and Reading.
//!
//! Decoding happens off the UI thread (`decode`, run as a task), two pictures
//! at a time so a few huge files can't stack up in memory. What's kept is
//! the picture at the size it's drawn: at most the text column's width
//! (or the embed's `|300`) times the screen's scale factor, never larger
//! than the file. `Pictures` holds them by file and width, at most
//! `BUDGET` bytes; past that the least recently wanted pictures that aren't
//! near the visible part of the page go first. The GPU keeps only what's
//! drawn this frame.
//!
//! `PictureColumn` is the widget: a line's pictures stacked under it, each
//! fitted to the column, with its laid-out height written back so the page
//! knows how tall the line is when it's off screen.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use iced::advanced::image::{self as advanced_image, Renderer as _};
use iced::advanced::layout::{self, Layout};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::tree::Tree;
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::{Clipboard, Shell, Widget, mouse};
use iced::widget::image::Handle;
use iced::widget::text::{Alignment, LineHeight, Shaping, Wrapping};
use iced::{Border, Element, Event, Length, Pixels, Point, Radians, Rectangle, Size, alignment};

use crate::live_text::Press;
use crate::theme;

/// Decoded pictures kept in memory, at most.
pub const BUDGET: usize = 64 * 1024 * 1024;
/// The widest a picture is drawn: the text column.
pub const MAX_WIDTH: u32 = crate::live::TEXT_WIDTH as u32;
/// Space around and between a line's pictures.
const GAP: f32 = 6.0;
/// A picture that couldn't be shown: one line of text in a box.
const PLACEHOLDER: f32 = 34.0;
/// A picture still loading: a box about as tall as a picture, which is
/// what the page reckons for a line it hasn't laid out yet. (Were it only a
/// line tall, more pictures would seem near the screen and load too.)
pub const LOADING: f32 = 200.0;

/// One picture file at one drawn width (logical pixels). `square`: a crew
/// portrait, cropped to its middle square.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub rel: Box<str>,
    pub width: u32,
    pub square: bool,
}

/// Portraits are decoded this wide (logical pixels): enough for the card
/// note's header, and shrunk on the GPU for the hand's faces.
pub const PORTRAIT: u32 = 96;

impl Key {
    /// A crew portrait.
    pub fn portrait(rel: Box<str>) -> Self {
        Self { rel, width: PORTRAIT, square: true }
    }
}

/// A decoded picture, ready to draw.
#[derive(Debug, Clone)]
pub struct Decoded {
    pub handle: Handle,
    /// The file's own size in pixels (after EXIF rotation): its natural
    /// size on the page, like Obsidian.
    pub natural: (u32, u32),
    /// What the decoded pixels take.
    pub bytes: usize,
}

#[derive(Debug, Clone)]
enum Slot {
    Loading,
    Ready(Decoded),
    Failed(String),
}

#[derive(Debug)]
struct Entry {
    slot: Slot,
    /// The `want` round it was last wanted in.
    used: u64,
}

/// What a picture embed shows right now.
#[derive(Debug, Clone)]
pub enum Shown {
    /// Not looked for yet (it just scrolled into view).
    Pending,
    /// No such file in the vault.
    Missing,
    Loading,
    Ready(Decoded),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct Pictures {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    /// Bumped by each `want`.
    round: u64,
    /// Bumped when everything is thrown away (a new scale factor), so
    /// pictures still decoding for the old one are dropped when they land.
    generation: u64,
    scale: f32,
    /// The scale factor has been reported: until then nothing is decoded
    /// (it would only be decoded again).
    scale_known: bool,
    /// For the note on screen: each picture's target as written, and the
    /// file it resolves to (`None`: not in the vault).
    targets: HashMap<String, Option<Box<str>>>,
    /// The open note is a crew card with this portrait.
    header: Option<Box<str>>,
}

impl Pictures {
    pub fn new() -> Self {
        Self { scale: 1.0, ..Self::default() }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Whether pictures can be decoded yet.
    pub fn ready(&self) -> bool {
        self.scale_known
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The window's scale factor. A change drops every picture (they were
    /// decoded for the old one).
    pub fn set_scale(&mut self, scale: f32) -> bool {
        let first = !self.scale_known;
        self.scale_known = true;
        if (scale - self.scale).abs() < 0.01 || scale <= 0.0 {
            return first;
        }
        self.scale = scale;
        self.entries.clear();
        self.bytes = 0;
        self.generation += 1;
        true
    }

    /// The width a picture is decoded for: its `|300`, at most the column
    /// it's drawn in (`column`, 0 until laid out), rounded up to 100 so a
    /// small resize doesn't decode it again. `None` until the column is known.
    pub fn decode_width(spec: Option<u32>, column: f32) -> Option<u32> {
        if column <= 0.0 {
            return None;
        }
        let w = spec.map_or(column, |s| (s as f32).min(column));
        Some(((w / 100.0).ceil() as u32 * 100).clamp(100, MAX_WIDTH.div_ceil(100) * 100))
    }

    /// What `target` (as written in the note) resolves to, for `shown`.
    pub fn set_targets(&mut self, targets: HashMap<String, Option<Box<str>>>) {
        self.targets = targets;
    }

    /// What an embed shows now, decoded for `width` (`decode_width`).
    pub fn shown(&self, target: &str, width: Option<u32>) -> Shown {
        let Some(found) = self.targets.get(target) else {
            return Shown::Pending;
        };
        let Some(rel) = found else {
            return Shown::Missing;
        };
        let Some(width) = width else {
            return Shown::Pending;
        };
        let key = Key { rel: rel.clone(), width, square: false };
        match self.entries.get(&key).map(|e| &e.slot) {
            None => Shown::Pending,
            Some(Slot::Loading) => Shown::Loading,
            Some(Slot::Ready(d)) => Shown::Ready(d.clone()),
            Some(Slot::Failed(why)) => Shown::Failed(why.clone()),
        }
    }

    /// A crew portrait (`portrait:` on a card, resolved), if decoded.
    pub fn portrait(&self, rel: &str) -> Option<Handle> {
        match &self.entries.get(&Key::portrait(rel.into()))?.slot {
            Slot::Ready(d) => Some(d.handle.clone()),
            _ => None,
        }
    }

    /// The portrait at the top of the open note (a crew card), if any.
    pub fn set_header(&mut self, rel: Option<Box<str>>) {
        self.header = rel;
    }

    pub fn header(&self) -> Option<Handle> {
        self.portrait(self.header.as_deref()?)
    }

    /// The pictures near the visible part of the page: marks them as in
    /// use and returns the ones to start decoding.
    pub fn want(&mut self, keys: impl IntoIterator<Item = Key>) -> Vec<Key> {
        self.round += 1;
        let mut start = Vec::new();
        for key in keys {
            match self.entries.get_mut(&key) {
                Some(entry) => entry.used = self.round,
                None => {
                    self.entries.insert(key.clone(), Entry { slot: Slot::Loading, used: self.round });
                    start.push(key);
                }
            }
        }
        start
    }

    /// A decode finished.
    pub fn loaded(&mut self, key: Key, generation: u64, result: Result<Decoded, String>) {
        if generation != self.generation {
            return;
        }
        let Some(entry) = self.entries.get_mut(&key) else {
            return;
        };
        if !matches!(entry.slot, Slot::Loading) {
            return;
        }
        entry.slot = match result {
            Ok(decoded) => {
                self.bytes += decoded.bytes;
                Slot::Ready(decoded)
            }
            Err(why) => Slot::Failed(why),
        };
        self.trim();
    }

    /// Drops the least recently wanted pictures until they fit the budget.
    /// The ones wanted now stay, even over it: they're on screen.
    fn trim(&mut self) {
        while self.bytes > BUDGET {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, e)| e.used < self.round && matches!(e.slot, Slot::Ready(_)))
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            let Some(key) = oldest else {
                break;
            };
            if let Some(Entry { slot: Slot::Ready(d), .. }) = self.entries.remove(&key) {
                self.bytes -= d.bytes;
            }
        }
    }

    /// Another vault: none of these pictures (the scale factor stays).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.targets.clear();
        self.bytes = 0;
        self.generation += 1;
    }

    /// A file changed or went: decode it again when it's next wanted.
    pub fn forget(&mut self, rel: &str) {
        let gone: Vec<Key> = self.entries.keys().filter(|k| k.rel.eq_ignore_ascii_case(rel)).cloned().collect();
        for key in gone {
            if let Some(Entry { slot: Slot::Ready(d), .. }) = self.entries.remove(&key) {
                self.bytes -= d.bytes;
            }
        }
    }

    /// After a rescan: try the ones that failed again.
    pub fn forget_failed(&mut self) {
        self.entries.retain(|_, e| !matches!(e.slot, Slot::Failed(_)));
    }

    /// Decoded bytes held, and how many pictures.
    pub fn held(&self) -> (usize, usize) {
        let ready = self.entries.values().filter(|e| matches!(e.slot, Slot::Ready(_))).count();
        (self.bytes, ready)
    }
}

/// Reads and decodes a picture for drawing `width` logical pixels wide at
/// `scale`: shrunk to that (never enlarged), rotated as its EXIF says.
pub fn decode(path: PathBuf, width: u32, scale: f32, square: bool) -> Result<Decoded, String> {
    use image::ImageDecoder;
    // Two at a time: decoding needs the whole picture at full size for a
    // moment, and a page of big photos shouldn't need them all at once.
    static SLOTS: [Mutex<()>; 2] = [Mutex::new(()), Mutex::new(())];
    let _slot = SLOTS
        .iter()
        .find_map(|m| m.try_lock().ok())
        .unwrap_or_else(|| SLOTS[0].lock().unwrap_or_else(std::sync::PoisonError::into_inner));

    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let fail = |e: &dyn std::fmt::Display| format!("Can't show {name}: {e}");
    let mut reader = image::ImageReader::open(&path)
        .map_err(|e| fail(&e))?
        .with_guessed_format()
        .map_err(|e| fail(&e))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| fail(&e))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|e| fail(&e))?;
    picture.apply_orientation(orientation);

    if square {
        // A portrait: its middle square (a little above the middle on a
        // tall one, where faces usually are).
        let (w, h) = (picture.width(), picture.height());
        let side = w.min(h).max(1);
        let x = (w - side) / 2;
        let y = if h > w { (h - side) / 3 } else { 0 };
        picture = picture.crop_imm(x, y, side, side);
    }
    let (w, h) = (picture.width().max(1), picture.height().max(1));
    let target = ((width as f32 * scale).ceil() as u32).clamp(1, w);
    let rgba = if target < w {
        let th = ((u64::from(h) * u64::from(target)) / u64::from(w)).max(1) as u32;
        // A box filter: fast, and fine for shrinking.
        picture.thumbnail_exact(target, th).into_rgba8()
    } else {
        picture.into_rgba8()
    };
    let (pw, ph) = rgba.dimensions();
    let pixels = rgba.into_raw();
    Ok(Decoded {
        bytes: pixels.len(),
        handle: Handle::from_rgba(pw, ph, pixels),
        natural: (w, h),
    })
}

/// One picture in a `PictureColumn`.
pub struct Item {
    pub name: String,
    pub width: Option<u32>,
    pub shown: Shown,
}

/// A line's pictures, stacked, each fitted to the column.
pub struct PictureColumn<'a, Message> {
    items: Vec<Item>,
    measure: &'a Cell<f32>,
    /// The column's width, written at layout: pictures are decoded for it.
    column: &'a Cell<f32>,
    on_press: Box<dyn Fn(usize, bool, Point) -> Message + 'a>,
    /// Sent once, on the first frame a picture is shown that hasn't been
    /// asked for yet, so the app looks it up now the column is known.
    on_pending: Option<Message>,
}

impl<'a, Message> PictureColumn<'a, Message> {
    /// `on_press(item, right button, window point)`.
    pub fn new(
        items: Vec<Item>,
        measure: &'a Cell<f32>,
        column: &'a Cell<f32>,
        on_press: impl Fn(usize, bool, Point) -> Message + 'a,
    ) -> Self {
        Self { items, measure, column, on_press: Box::new(on_press), on_pending: None }
    }

    pub fn on_pending(mut self, message: Message) -> Self {
        if self.items.iter().any(|i| matches!(i.shown, Shown::Pending)) {
            self.on_pending = Some(message);
        }
        self
    }

    /// Where each picture goes in a column `width` wide (from its top).
    fn rects(&self, width: f32) -> Vec<Rectangle> {
        let mut y = GAP;
        self.items
            .iter()
            .map(|item| {
                let size = match &item.shown {
                    Shown::Ready(d) => {
                        let (nw, nh) = (d.natural.0 as f32, d.natural.1 as f32);
                        let w = item.width.map_or(nw, |w| w as f32).min(width).max(1.0);
                        Size::new(w, (w * nh / nw).max(1.0))
                    }
                    Shown::Pending | Shown::Loading => Size::new(width.min(item.width.map_or(f32::INFINITY, |w| w as f32).max(160.0)), LOADING),
                    Shown::Missing | Shown::Failed(_) => Size::new(width.min(item.width.map_or(f32::INFINITY, |w| w as f32).max(160.0)), PLACEHOLDER),
                };
                let rect = Rectangle::new(Point::new(0.0, y), size);
                y += size.height + GAP;
                rect
            })
            .collect()
    }

    /// How tall the column is at `width`.
    pub fn height(&self, width: f32) -> f32 {
        self.rects(width).last().map_or(0.0, |r| r.y + r.height + GAP)
    }
}

/// A line's pictures before they're laid out: each as tall as `LOADING`.
pub fn estimate(count: usize) -> f32 {
    GAP + count as f32 * (LOADING + GAP)
}

/// Whether this column has asked for its pending pictures already.
#[derive(Default)]
struct Asked(bool);

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for PictureColumn<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<Asked>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(Asked::default())
    }

    fn size(&self) -> Size<Length> {
        Size { width: Length::Fill, height: Length::Shrink }
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let width = limits.max().width;
        let height = self.height(width);
        self.measure.set(height);
        self.column.set(width);
        layout::Node::new(Size::new(width, height))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if !bounds.intersects(viewport) {
            return;
        }
        for (item, rect) in self.items.iter().zip(self.rects(bounds.width)) {
            let rect = Rectangle::new(rect.position() + (bounds.position() - Point::ORIGIN), rect.size());
            if !rect.intersects(viewport) {
                continue;
            }
            if let Shown::Ready(d) = &item.shown {
                let image = advanced_image::Image {
                    handle: d.handle.clone(),
                    filter_method: advanced_image::FilterMethod::Linear,
                    rotation: Radians(0.0),
                    border_radius: 4.0.into(),
                    opacity: 1.0,
                    snap: true,
                };
                renderer.draw_image(image, rect, rect);
                continue;
            }
            let (words, color) = match &item.shown {
                Shown::Missing => (format!("⚠ {} isn't in the vault", item.name), theme::WARNING),
                Shown::Failed(why) => (format!("⚠ {why}"), theme::WARNING),
                _ => (format!("🖼 {}", item.name), theme::FAINT),
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: rect,
                    border: Border { color: theme::LINE, width: 1.0, radius: 4.0.into() },
                    ..Default::default()
                },
                theme::PANEL,
            );
            let text = iced::advanced::Text {
                content: words,
                bounds: Size::new(rect.width - 20.0, rect.height),
                size: Pixels(theme::SMALL),
                line_height: LineHeight::Relative(1.3),
                font: theme::UI_FONT,
                align_x: Alignment::Default,
                align_y: alignment::Vertical::Center,
                shaping: Shaping::Advanced,
                wrapping: Wrapping::None,
            };
            renderer.fill_text(text, Point::new(rect.x + 10.0, rect.center_y()), color, rect);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let asked = tree.state.downcast_mut::<Asked>();
        if let Event::Window(iced::window::Event::RedrawRequested(_)) = event {
            match &self.on_pending {
                Some(message) if !asked.0 => {
                    asked.0 = true;
                    shell.publish(message.clone());
                }
                Some(_) => {}
                None => asked.0 = false,
            }
            return;
        }
        let right = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => false,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => true,
            _ => return,
        };
        let bounds = layout.bounds();
        let (Some(local), Some(window)) = (cursor.position_in(bounds), cursor.position()) else {
            return;
        };
        if let Some(i) = self.rects(bounds.width).iter().position(|r| r.contains(local)) {
            shell.publish((self.on_press)(i, right, window));
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        match cursor.position_in(bounds) {
            Some(p) if self.rects(bounds.width).iter().any(|r| r.contains(p)) => mouse::Interaction::Pointer,
            _ => mouse::Interaction::None,
        }
    }
}

impl<'a, Message: Clone + 'a> From<PictureColumn<'a, Message>> for Element<'a, Message> {
    fn from(widget: PictureColumn<'a, Message>) -> Self {
        Element::new(widget)
    }
}

/// A press on a picture, as a press on its line's text: in Reading (and
/// Split's preview) it opens the file like its link; in Live it starts
/// editing at the embed.
pub fn press(seg: usize, seg_offset: usize, read_only: bool, right: bool, window: Point) -> Press {
    Press {
        offset: seg_offset,
        link: read_only.then_some(seg),
        right,
        window,
    }
}
