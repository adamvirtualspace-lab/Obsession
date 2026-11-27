//! One drawn line of Live Preview: styled spans, like iced's rich text, but a
//! press reports *where* in the text it landed (a byte offset into the drawn
//! text), so the app can put the caret on the matching raw character.
//! Spans with a link (status chips, links) show a pointer. A hovered link
//! is underlined; a hovered status chip (`quiet`) grows instead (`POP`),
//! drawn over the line so nothing around it moves.

use std::cell::Cell;

use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{Paragraph, Renderer as _, Span};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::renderer::Renderer as _;
use iced::advanced::{Clipboard, Shell, Widget, mouse, renderer};
use iced::alignment;
use iced::widget::text::{Alignment, LineHeight, Shaping, Wrapping};
use iced::{Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Vector};

type P = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;

/// How much a hovered status chip grows.
const POP: f32 = 1.3;

/// What a press on the line hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press {
    /// Byte offset in the drawn text (`usize::MAX` past the end).
    pub offset: usize,
    /// The span index, if the press landed on a span with a link.
    pub link: Option<usize>,
    pub right: bool,
    /// Window coordinates, for menus.
    pub window: Point,
}

pub struct LiveText<'a, Message> {
    measure: Option<&'a Cell<f32>>,
    /// `measure` keeps the tallest of several (a table row's cells).
    tallest: bool,
    spans: Vec<Span<'a, usize, Font>>,
    size: Pixels,
    line_height: LineHeight,
    font: Font,
    color: Color,
    wrapping: Wrapping,
    /// While a selection is being dragged: called with the text offset
    /// under the cursor as it moves over this line.
    on_drag: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    on_press: Box<dyn Fn(Press) -> Message + 'a>,
    /// Links (by their value) that grow when hovered instead of getting an
    /// underline: status boxes, where a line would cross the symbol.
    quiet: Vec<usize>,
    /// What's behind the line, so a growing chip can cover its own symbol.
    /// Without it, a quiet link just shows the pointer.
    ground: Option<Color>,
}

impl<'a, Message> LiveText<'a, Message> {
    pub fn new(
        spans: Vec<Span<'a, usize, Font>>,
        on_press: impl Fn(Press) -> Message + 'a,
    ) -> Self {
        Self {
            measure: None,
            tallest: false,
            spans,
            size: Pixels(15.0),
            line_height: LineHeight::Relative(1.5),
            font: Font::DEFAULT,
            color: Color::WHITE,
            wrapping: Wrapping::WordOrGlyph,
            on_drag: None,
            on_press: Box::new(on_press),
            quiet: Vec::new(),
            ground: None,
        }
    }

    /// Links (by their value) that grow when hovered instead of getting an
    /// underline, drawn on `ground`.
    pub fn quiet(mut self, links: Vec<usize>, ground: Color) -> Self {
        self.quiet = links;
        self.ground = Some(ground);
        self
    }

    /// The hovered span `index` drawn `POP` times bigger, if it's a quiet link.
    fn pop(&self, index: usize) -> Option<(usize, P)> {
        self.ground?;
        let span = self.spans.get(index)?;
        if !span.link.is_some_and(|link| self.quiet.contains(&link)) {
            return None;
        }
        let size = Pixels(span.size.unwrap_or(self.size).0 * POP);
        // Laid out the way the line is, only bigger: the line's own line
        // height, and a space in the line's font after the symbol. A line
        // puts its baseline from every font on it (the text's and the
        // symbol's), so without the space the symbol alone would sit at
        // another height in its box. `draw` then centres the symbol's own
        // box on the small one, so it grows from its middle.
        let line_height = match span.line_height.unwrap_or(self.line_height) {
            LineHeight::Relative(r) => LineHeight::Relative(r),
            LineHeight::Absolute(p) => LineHeight::Absolute(Pixels(p.0 * POP)),
        };
        let spans: [Span<'_, usize, Font>; 2] = [
            Span::new(&*span.text).size(size).font(span.font.unwrap_or(self.font)),
            Span::new(" ").size(size).font(self.font),
        ];
        let paragraph = P::with_spans(iced::advanced::Text {
            content: spans.as_slice(),
            bounds: Size::INFINITE,
            size,
            line_height,
            font: self.font,
            align_x: Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::None,
        });
        Some((index, paragraph))
    }

    /// Writes the laid-out height here, so the page can skip drawing lines
    /// that are off screen and still know how tall they are.
    /// Like `measure`, but keeps the tallest height any widget reports into
    /// `cell` (one cell shared by a table row's cells).
    pub fn measure_tallest(mut self, cell: &'a Cell<f32>) -> Self {
        self.measure = Some(cell);
        self.tallest = true;
        self
    }

    pub fn measure(mut self, cell: &'a Cell<f32>) -> Self {
        self.measure = Some(cell);
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = Pixels(size);
        self
    }

    pub fn line_height(mut self, line_height: LineHeight) -> Self {
        self.line_height = line_height;
        self
    }

    /// Code keeps its lines whole (ASCII art, aligned columns).
    pub fn no_wrap(mut self) -> Self {
        self.wrapping = Wrapping::None;
        self
    }

    pub fn font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Reports the offset under the cursor as it moves, while `dragging`.
    pub fn on_drag(mut self, dragging: bool, on_drag: impl Fn(usize) -> Message + 'a) -> Self {
        if dragging {
            self.on_drag = Some(Box::new(on_drag));
        }
        self
    }
}

struct State {
    spans: Vec<Span<'static, usize, Font>>,
    paragraph: P,
    hovered_link: Option<usize>,
    /// The hovered status chip, drawn bigger: (span index, its paragraph).
    pop: Option<(usize, P)>,
    /// The last offset reported while dragging, so moves within one
    /// character don't repeat it.
    dragged: Option<usize>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for LiveText<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            spans: Vec::new(),
            paragraph: P::default(),
            hovered_link: None,
            pop: None,
            dragged: None,
        })
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let bounds = Size::new(limits.max().width, f32::INFINITY);
        let text = iced::advanced::Text {
            content: self.spans.as_slice(),
            bounds,
            size: self.size,
            line_height: self.line_height,
            font: self.font,
            align_x: Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: self.wrapping,
        };
        if state.spans != self.spans {
            state.paragraph = P::with_spans(text);
            state.spans = self.spans.iter().cloned().map(Span::to_static).collect();
            // A click changed the chip under the pointer: grow the new one.
            state.pop = state.pop.take().and_then(|(index, _)| self.pop(index));
        } else if state.paragraph.bounds() != bounds {
            state.paragraph.resize(bounds);
        }
        // `OBSESSION_DEV_POP=1`: every line's first status chip drawn as if
        // hovered, so a screenshot shows how they grow.
        #[cfg(debug_assertions)]
        if state.pop.is_none()
            && std::env::var_os("OBSESSION_DEV_POP").is_some()
            && let Some(index) = (0..self.spans.len()).find(|&i| self.pop(i).is_some())
        {
            state.pop = self.pop(index);
        }
        // An empty line still takes one line of height.
        let line = self.line_height.to_absolute(self.size).0;
        let min = state.paragraph.min_bounds();
        let height = min.height.max(line);
        if let Some(cell) = self.measure {
            cell.set(if self.tallest { cell.get().max(height) } else { height });
        }
        layout::Node::new(Size::new(limits.max().width, height))
    }

    fn draw(
        &self,
        tree: &Tree,
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
        let state = tree.state.downcast_ref::<State>();
        let translation = bounds.position() - Point::ORIGIN;

        for (index, span) in self.spans.iter().enumerate() {
            let hovered = state.hovered_link == Some(index);
            if span.highlight.is_none() && !span.underline && !span.strikethrough && !hovered {
                continue;
            }
            let regions = state.paragraph.span_bounds(index);
            if let Some(highlight) = span.highlight {
                for region in &regions {
                    let region = Rectangle::new(
                        region.position() - Vector::new(span.padding.left, span.padding.top),
                        region.size() + Size::new(span.padding.x(), span.padding.y()),
                    );
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: region + translation,
                            border: highlight.border,
                            ..Default::default()
                        },
                        highlight.background,
                    );
                }
            }
            let size = span.size.unwrap_or(self.size);
            let line_height = span
                .line_height
                .unwrap_or(self.line_height)
                .to_absolute(size);
            let color = span.color.unwrap_or(self.color);
            let baseline = translation + Vector::new(0.0, size.0 + (line_height.0 - size.0) / 2.0);
            let hover_line = hovered && span.link.is_some_and(|link| !self.quiet.contains(&link));
            if span.underline || hover_line {
                for region in &regions {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle::new(
                                region.position() + baseline - Vector::new(0.0, size.0 * 0.08),
                                Size::new(region.width, 1.0),
                            ),
                            ..Default::default()
                        },
                        color,
                    );
                }
            }
            if span.strikethrough {
                for region in &regions {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle::new(
                                region.position() + baseline - Vector::new(0.0, size.0 / 2.0),
                                Size::new(region.width, 1.0),
                            ),
                            ..Default::default()
                        },
                        color,
                    );
                }
            }
        }

        renderer.fill_paragraph(&state.paragraph, bounds.position(), self.color, *viewport);

        // The hovered status chip, bigger, over its own symbol: the symbol is
        // covered with what's behind it (its highlight, or the ground), and
        // the big one is centred on it.
        if let (Some((index, big)), Some(ground)) = (&state.pop, self.ground)
            && let Some(span) = self.spans.get(*index)
            && let Some(region) = state.paragraph.span_bounds(*index).first().copied()
        {
            let behind = match span.highlight.map(|h| h.background) {
                Some(iced::Background::Color(color)) => color,
                _ => ground,
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: region + translation,
                    ..Default::default()
                },
                behind,
            );
            // The big symbol's own box (span 0), centred on the small one's.
            let big_box = big.span_bounds(0).first().copied().unwrap_or(Rectangle::new(Point::ORIGIN, big.min_bounds()));
            // Glyphs land on whole pixels; half a pixel down evens out the
            // rounding (measured: otherwise the big one sits ~1 px high).
            let at = region.center() + translation - (big_box.center() - Point::ORIGIN) + Vector::new(0.0, 0.5);
            renderer.fill_paragraph(big, at, span.color.unwrap_or(self.color), *viewport);
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
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();

        let local = cursor.position_in(bounds);
        let hovered = local.and_then(|point| {
            let span = state.paragraph.hit_span(point)?;
            self.spans.get(span)?.link.is_some().then_some(span)
        });
        if hovered != state.hovered_link {
            state.hovered_link = hovered;
            state.pop = hovered.and_then(|index| self.pop(index));
            shell.request_redraw();
        }

        if let Event::Mouse(mouse::Event::CursorMoved { .. }) = event {
            match (&self.on_drag, local) {
                (Some(on_drag), Some(point)) => {
                    let offset = state
                        .paragraph
                        .hit_test(point)
                        .map_or(usize::MAX, |hit| hit.cursor());
                    if state.dragged != Some(offset) {
                        state.dragged = Some(offset);
                        shell.publish(on_drag(offset));
                    }
                }
                (None, _) => state.dragged = None,
                _ => {}
            }
            return;
        }

        let right = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => false,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => true,
            _ => return,
        };
        let (Some(point), Some(window)) = (local, cursor.position()) else {
            return;
        };
        let offset = state
            .paragraph
            .hit_test(point)
            .map_or(usize::MAX, |hit| hit.cursor());
        let link = hovered.and_then(|span| self.spans.get(span)?.link);
        state.dragged = Some(offset);
        shell.publish((self.on_press)(Press {
            offset,
            link,
            right,
            window,
        }));
        shell.capture_event();
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if !cursor.is_over(layout.bounds()) {
            return mouse::Interaction::None;
        }
        if tree.state.downcast_ref::<State>().hovered_link.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Text
        }
    }
}

impl<'a, Message: 'a> From<LiveText<'a, Message>> for Element<'a, Message> {
    fn from(widget: LiveText<'a, Message>) -> Self {
        Element::new(widget)
    }
}
