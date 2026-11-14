//! A wrapper that reports where a right-click landed, both relative to the
//! wrapped widget and in window coordinates. iced's `mouse_area` only says
//! *that* a right-click happened, and its move events would rebuild the view
//! on every mouse move. This one does nothing until the button goes down.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

pub struct RightClick<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_right: Box<dyn Fn(Point, Point) -> Message + 'a>,
}

/// Wraps `content`; `on_right(local, window)` is published on right-press.
pub fn right_click<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_right: impl Fn(Point, Point) -> Message + 'a,
) -> RightClick<'a, Message, Theme, Renderer> {
    RightClick {
        content: content.into(),
        on_right: Box::new(on_right),
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for RightClick<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
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
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event {
            let bounds = layout.bounds();
            if let (Some(local), Some(window)) = (cursor.position_in(bounds), cursor.position()) {
                shell.publish((self.on_right)(local, window));
                shell.capture_event();
                return;
            }
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
