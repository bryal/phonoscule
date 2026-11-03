//! A wrapper that keeps Alt chords from typing into the text field inside it.
//!
//! iced's text input inserts a key's text whatever modifier is held but Ctrl, so Alt+P pressed in the
//! album search appends a "p" rather than playing what the search found. The wrapper hands its
//! content every Alt chord with its text removed: the field ignores it, and it reaches the key
//! bindings as if no field had focus.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector, keyboard};

/// Wraps `content`, which is laid out, drawn and updated exactly as it would be on its own.
pub fn alt_passthrough<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> AltPassthrough<'a, Message, Theme, Renderer> {
    AltPassthrough { content: content.into() }
}

pub struct AltPassthrough<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

// Everything is the content's, its widget tree included, so ids and focus inside it are unchanged.
impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for AltPassthrough<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
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
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.content.as_widget_mut().operate(tree, layout, renderer, operation);
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
        let textless;
        let event = match event {
            Event::Keyboard(keyboard::Event::KeyPressed { modifiers, .. }) if modifiers.alt() => {
                let mut stripped = event.clone();
                if let Event::Keyboard(keyboard::Event::KeyPressed { text, .. }) = &mut stripped {
                    *text = None;
                }
                textless = stripped;
                &textless
            }
            _ => event,
        };
        self.content.as_widget_mut().update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a, Theme: 'a, Renderer> From<AltPassthrough<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(wrapper: AltPassthrough<'a, Message, Theme, Renderer>) -> Self {
        Element::new(wrapper)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use iced::advanced::widget::{self, operation};
    use iced::keyboard::key::{Code, Physical};
    use iced::keyboard::{Key, Location, Modifiers};
    use iced::widget::text_input;

    /// What a focused search field publishes for one press of `p` with `modifiers` held, and whether
    /// it kept the press from the key bindings.
    fn typed_into(wrapped: bool, modifiers: Modifiers) -> (Vec<String>, bool) {
        let input = text_input::<String, iced::Theme, ()>("", "").id("field").on_input(|value| value);
        let mut element: Element<'_, String, iced::Theme, ()> =
            if wrapped { alt_passthrough(input).into() } else { input.into() };
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, Size::new(200.0, 40.0)));
        let mut focus = operation::focusable::focus::<()>(widget::Id::new("field"));
        element.as_widget_mut().operate(&mut tree, Layout::new(&node), &(), &mut focus);

        let press = Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Character("p".into()),
            modified_key: Key::Character("p".into()),
            physical_key: Physical::Code(Code::KeyP),
            location: Location::Standard,
            modifiers,
            text: Some("p".into()),
            repeat: false,
        });
        let mut published = Vec::new();
        let mut shell = Shell::new(&mut published);
        let viewport = Rectangle::new(iced::Point::ORIGIN, Size::new(200.0, 40.0));
        element.as_widget_mut().update(
            &mut tree,
            &press,
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &(),
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &viewport,
        );
        let captured = shell.is_event_captured();
        (published, captured)
    }

    /// The field still types, and only an Alt chord is kept out of it.
    #[test]
    fn alt_chords_do_not_type() {
        assert_eq!(typed_into(false, Modifiers::ALT), (vec!["p".into()], true), "unwrapped, iced types the chord's letter");
        assert_eq!(typed_into(true, Modifiers::ALT), (vec![], false), "wrapped, the chord passes on untyped");
        assert_eq!(typed_into(true, Modifiers::empty()), (vec!["p".into()], true));
    }
}
