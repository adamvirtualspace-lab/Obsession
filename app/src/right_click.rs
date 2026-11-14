//! A wrapper for editors:
//!
//! - reports where a right-click landed, relative to the wrapped widget and in
//!   window coordinates (iced's `mouse_area` only says *that* one happened);
//! - optionally lets mouse-wheel events through to the page instead of the
//!   wrapped editor, so a page of Live Preview scrolls even over the line
//!   being edited;
//! - optionally asks for a scroll when the wrapped widget is out of view
//!   after a change (`reveal`), once per change.
//!
//! It does nothing until one of those events happens.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::Operation;
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, window};

/// Space kept between a revealed widget and the edge of the view.
const REVEAL_MARGIN: f32 = 48.0;

type OnScroll<'a, Message> = Box<dyn Fn(f32) -> Message + 'a>;

pub struct RightClick<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_right: Box<dyn Fn(Point, Point) -> Message + 'a>,
    ignore_wheel: bool,
    reveal: Option<(u64, OnScroll<'a, Message>)>,
}

/// Wraps `content`; `on_right(local, window)` is published on right-press.
pub fn right_click<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_right: impl Fn(Point, Point) -> Message + 'a,
) -> RightClick<'a, Message, Theme, Renderer> {
    RightClick {
        content: content.into(),
        on_right: Box::new(on_right),
        ignore_wheel: false,
        reveal: None,
    }
}

impl<'a, Message, Theme, Renderer> RightClick<'a, Message, Theme, Renderer> {
    pub fn ignore_wheel(mut self) -> Self {
        self.ignore_wheel = true;
        self
    }

    /// When `generation` changes, checks once whether the widget is in view
    /// and, if not, publishes `on_scroll(dy)` with the distance to scroll.
    pub fn reveal(mut self, generation: u64, on_scroll: impl Fn(f32) -> Message + 'a) -> Self {
        self.reveal = Some((generation, Box::new(on_scroll)));
        self
    }
}

#[derive(Default)]
struct State {
    revealed: u64,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for RightClick<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let bounds = layout.bounds();
                if let (Some(local), Some(window)) = (cursor.position_in(bounds), cursor.position())
                {
                    shell.publish((self.on_right)(local, window));
                    shell.capture_event();
                    return;
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { .. }) if self.ignore_wheel => return,
            Event::Window(window::Event::RedrawRequested(_)) => {
                if let Some((generation, on_scroll)) = &self.reveal {
                    let state = tree.state.downcast_mut::<State>();
                    if state.revealed != *generation {
                        state.revealed = *generation;
                        let bounds = layout.bounds();
                        let dy = if bounds.y < viewport.y {
                            bounds.y - viewport.y - REVEAL_MARGIN
                        } else if bounds.y + bounds.height > viewport.y + viewport.height {
                            // Down, but never past the widget's top.
                            let down = bounds.y + bounds.height + REVEAL_MARGIN
                                - (viewport.y + viewport.height);
                            down.min(bounds.y - viewport.y)
                        } else {
                            0.0
                        };
                        if dy.abs() > 0.5 {
                            shell.publish(on_scroll(dy));
                        }
                    }
                }
            }
            _ => {}
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<RightClick<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + renderer::Renderer,
{
    fn from(widget: RightClick<'a, Message, Theme, Renderer>) -> Self {
        Element::new(widget)
    }
}
