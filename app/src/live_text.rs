//! One drawn line of Live Preview: styled spans, like iced's rich text, but a
//! press reports *where* in the text it landed (a byte offset into the drawn
//! text), so the app can put the caret on the matching raw character.
//! Spans with a link (status chips, links) show a pointer.

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
    spans: Vec<Span<'a, usize, Font>>,
    size: Pixels,
    line_height: LineHeight,
    font: Font,
    color: Color,
    wrapping: Wrapping,
    on_press: Box<dyn Fn(Press) -> Message + 'a>,
}

impl<'a, Message> LiveText<'a, Message> {
    pub fn new(
        spans: Vec<Span<'a, usize, Font>>,
        on_press: impl Fn(Press) -> Message + 'a,
    ) -> Self {
        Self {
            measure: None,
            spans,
            size: Pixels(15.0),
            line_height: LineHeight::Relative(1.5),
            font: Font::DEFAULT,
            color: Color::WHITE,
            wrapping: Wrapping::WordOrGlyph,
            on_press: Box::new(on_press),
        }
    }

    /// Writes the laid-out height here, so the page can skip drawing lines
    /// that are off screen and still know how tall they are.
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
}

struct State {
    spans: Vec<Span<'static, usize, Font>>,
    paragraph: P,
    hovered_link: Option<usize>,
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
        } else if state.paragraph.bounds() != bounds {
            state.paragraph.resize(bounds);
        }
        // An empty line still takes one line of height.
        let line = self.line_height.to_absolute(self.size).0;
        let min = state.paragraph.min_bounds();
        let height = min.height.max(line);
        if let Some(cell) = self.measure {
            cell.set(height);
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
            if span.underline || (hovered && span.link.is_some()) {
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
            shell.request_redraw();
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
        shell.publish((self.on_press)(Press {
            offset,
            link: hovered,
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
