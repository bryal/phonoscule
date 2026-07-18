//! The library's album grid: a scrolling, selectable grid of album cards, as one custom widget.
//!
//! One type owns every bespoke behavior the library view needs, so none of it leaks into the
//! application's model/update/view files:
//!
//! - Wheel/touchpad scrolling, scrollbar-less (matching how the stock scrollable was configured).
//! - Hovering a cover selects its album; the cursor leaving every cover deselects. The action
//!   bubbles floating over a cover capture their clicks first (children update before the grid
//!   and the grid respects capture), so they act without the grid reacting.
//! - Arrow-key navigation in two dimensions; when nothing is selected, an arrow key picks up the
//!   first album in view. The selection is always scrolled fully into view, minimally.
//! - Space queues the selected album and Ctrl+Space plays it, published via `on_queue`/`on_play`.
//! - Left-clicking a cover, or Enter on the selection, asks for the album's track menu
//!   (published via `on_menu`; the menu itself is the caller's).
//!
//! Selection and scroll position live in the widget tree ([`State`]), not the application model:
//! the widget publishes whole actions (`on_play(ix)`), so nothing outside needs to track them.
//! The exception is opt-in: [`selected`](AlbumGrid::selected) externalizes the selection so it
//! survives the state being dropped when the view is left. And the widget draws the card texts
//! itself from its own metric constants, so the geometry keyboard navigation relies on and the
//! rendered layout cannot drift apart. Both state and layout being in one place is the point --
//! the previous split (selection in the model, geometry mirrored out of the view, scroll offset
//! mirrored out of the scrollable) needed three files to cooperate.
//!
//! The caller supplies each card's cover as an [`Element`] (image or fallback, plus any floating
//! action bubbles), which the grid lays out to exactly the cover square; build it size-agnostic
//! (`Fill`). Titles and artists are passed as strings and drawn by the grid.

use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Text, Widget, layout, mouse, overlay, text, text::Renderer as _};
use iced::keyboard::{self, key::Named};
use iced::{Border, Color, Element, Event, Length, Pixels, Rectangle, Renderer, Size, Theme, Vector};

/// Horizontal padding around the grid, and spacing between cards within a row.
const GRID_PADDING: f32 = 16.0;
const GRID_SPACING: f32 = 16.0;
/// Vertical spacing between rows.
const ROW_SPACING: f32 = 24.0;
/// The base card width: as many columns as fit at this width, which then stretch to fill the row.
const CARD_SIDE: f32 = 168.0;
/// The pad between a card's edge and its content, framing the selection backdrop.
const CARD_PAD: f32 = 6.0;
/// The title block: exactly two lines at a fixed line height, so every card is the same height
/// (keyboard navigation computes row positions from [`Geom::card_h`]). Longer titles are clipped.
const TITLE_HEIGHT: f32 = 38.0;
const TITLE_SIZE: f32 = 15.0;
const TITLE_LINE_HEIGHT: f32 = 19.0;
/// The artist block: one line, clipping any wrap, for the same uniformity.
const ARTIST_HEIGHT: f32 = 17.0;
const ARTIST_SIZE: f32 = 13.0;
/// Spacing between a card's cover, title, and artist blocks.
const CARD_SPACING: f32 = 4.0;
/// How far one scroll-wheel line moves the grid, in pixels (the stock scrollable's factor).
const WHEEL_LINE: f32 = 60.0;

pub fn album_grid<'a, Message>(on_play: fn(usize) -> Message, on_queue: fn(usize) -> Message) -> AlbumGrid<'a, Message> {
    AlbumGrid {
        cards: Vec::new(),
        top_clearance: 0.0,
        bottom_clearance: 0.0,
        on_play,
        on_queue,
        on_menu: None,
        selection: None,
        interactive: true,
    }
}

pub struct AlbumGrid<'a, Message> {
    cards: Vec<Card<'a, Message>>,
    /// Space above the first row (also what scrolling a selection up leaves clear -- the floating
    /// tabs live there) and below the last (so it can scroll out from under the player bar, which
    /// also bounds "in view" from below).
    top_clearance: f32,
    bottom_clearance: f32,
    /// Play the album (replacing the queue): Ctrl+Space on the selection.
    on_play: fn(usize) -> Message,
    /// Append the album to the queue: Space on the selection.
    on_queue: fn(usize) -> Message,
    /// Ask for an album's track menu: left-clicking a cover, or Enter on the selection. The menu
    /// itself is the caller's business; the grid only reports the ask.
    on_menu: Option<fn(usize) -> Message>,
    /// Externalized selection, when the caller opted in via [`selected`](Self::selected).
    selection: Option<Selection<Message>>,
    /// Whether the grid reacts to input at all. An `opaque` modal backdrop only blocks button
    /// presses -- cursor moves, wheel scrolls, and keyboard events still reach every widget in
    /// the tree -- so the caller disables this while a modal covers the grid.
    interactive: bool,
}

/// The two halves of an externalized selection: the caller's value, synced into the internal
/// selection on every render, and the message publishing every change back to the caller.
struct Selection<Message> {
    value: Option<usize>,
    notify: fn(Option<usize>) -> Message,
}

// Derived Clone/Copy would demand `Message: Clone/Copy`; the fields are copyable regardless.
impl<Message> Clone for Selection<Message> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Message> Copy for Selection<Message> {}

struct Card<'a, Message> {
    cover: Element<'a, Message>,
    title: &'a str,
    artist: &'a str,
}

impl<'a, Message> AlbumGrid<'a, Message> {
    pub fn push(mut self, cover: impl Into<Element<'a, Message>>, title: &'a str, artist: &'a str) -> Self {
        self.cards.push(Card { cover: cover.into(), title, artist });
        self
    }

    pub fn top_clearance(self, clearance: f32) -> Self {
        Self { top_clearance: clearance, ..self }
    }

    pub fn bottom_clearance(self, clearance: f32) -> Self {
        Self { bottom_clearance: clearance, ..self }
    }

    /// Externalizes the selection so it survives the widget's own state being dropped (leaving the
    /// view drops the whole subtree): the widget treats `selected` as the source of truth on every
    /// render and publishes `on_select` whenever the selection changes, so the caller's store and
    /// the internal state mirror each other. On a fresh mount the selection is also scrolled back
    /// into view (the scroll offset itself is not externalized -- restoring the selection's
    /// context is what matters).
    pub fn selected(self, selected: Option<usize>, on_select: fn(Option<usize>) -> Message) -> Self {
        Self { selection: Some(Selection { value: selected, notify: on_select }), ..self }
    }

    /// The message asking for an album's track menu: published on left-clicking a cover and on
    /// Enter with a selection.
    pub fn on_menu(self, on_menu: fn(usize) -> Message) -> Self {
        Self { on_menu: Some(on_menu), ..self }
    }

    /// Sets whether the grid reacts to input; disable it while a modal covers the grid (see the
    /// `interactive` field).
    pub fn interactive(self, enabled: bool) -> Self {
        Self { interactive: enabled, ..self }
    }

    fn geom(&self, width: f32) -> Geom {
        Geom::new(width, self.top_clearance, self.bottom_clearance)
    }
}

/// The grid's selection and scroll state, owned by the widget tree, which drops it when the
/// library view is left; an externalized selection (see [`AlbumGrid::selected`]) survives that.
#[derive(Default)]
struct State {
    selected: Option<usize>,
    /// Scroll offset in pixels; clamped against the content height on use, not on write, so a
    /// window resize can't strand it (the stock scrollable does the same).
    offset: f32,
    /// Whether the fresh-mount pass ran (see the top of `update`): a restored selection is
    /// scrolled back into view on the first event after the widget (re)mounts.
    restored: bool,
}

/// A direction to move the selection.
#[derive(Clone, Copy)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl<Message> Widget<Message, Theme, Renderer> for AlbumGrid<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        // Seed from the externalized selection: a fresh state (`diff` never ran) must already
        // carry it, or the first frame after a view switch would briefly lose the selection.
        let selected = self.selection.and_then(|selection| selection.value);
        tree::State::new(State { selected, ..State::default() })
    }

    fn children(&self) -> Vec<Tree> {
        self.cards.iter().map(|card| Tree::new(&card.cover)).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.cards.iter().map(|card| &card.cover).collect::<Vec<_>>());
        let state = tree.state.downcast_mut::<State>();
        // An externalized selection is the source of truth: sync from it every render.
        if let Some(selection) = self.selection {
            state.selected = selection.value;
        }
        // A rescan can shrink the library under the selection: clamp to the last album.
        if state.selected.is_some_and(|s| s >= self.cards.len()) {
            state.selected = self.cards.len().checked_sub(1);
        }
    }

    fn size(&self) -> Size<Length> {
        Size { width: Length::Fill, height: Length::Fill }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits.max();
        let geom = self.geom(size.width);
        // Each cover is laid out to exactly its square (they're built size-agnostic), positioned
        // at its unscrolled content coordinates; drawing translates by the scroll offset.
        let covers = self
            .cards
            .iter_mut()
            .zip(&mut tree.children)
            .enumerate()
            .map(|(ix, (card, tree))| {
                let square = geom.cover(ix);
                let limits = layout::Limits::new(square.size(), square.size());
                card.cover.as_widget_mut().layout(tree, renderer, &limits).move_to(square.position())
            })
            .collect();
        layout::Node::with_children(size, covers)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((card, tree), layout) in self.cards.iter_mut().zip(&mut tree.children).zip(layout.children()) {
                card.cover.as_widget_mut().operate(tree, layout, renderer, operation);
            }
        });
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
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let n = self.cards.len();
        let geom = self.geom(bounds.width);
        let state = tree.state.downcast_mut::<State>();

        // Fresh mount: an externally persisted selection may sit anywhere in the grid while the
        // scroll offset starts at zero, so scroll it back into view before anything is shown (the
        // first event through here is the pre-paint redraw request).
        if !state.restored {
            state.restored = true;
            if let Some(selected) = state.selected
                && let Some(target) = geom.scroll_target(state.offset, bounds.height, selected / geom.cols)
            {
                state.offset = target.clamp(0.0, geom.max_offset(n, bounds.height));
            }
        }

        let offset = state.offset.clamp(0.0, geom.max_offset(n, bounds.height));
        let before = state.selected;

        // Children live at unscrolled content coordinates: hand them the cursor translated into
        // that space, and the visible region likewise. They react first and may capture (the
        // action bubbles' clicks and hover), in which case the grid stays out of it.
        let content_cursor = match cursor.position_over(bounds) {
            Some(position) => mouse::Cursor::Available(position + Vector::new(0.0, offset)),
            None => mouse::Cursor::Unavailable,
        };
        let content_viewport = Rectangle { y: bounds.y + offset, ..bounds };
        for ((card, tree), layout) in self.cards.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            card.cover.as_widget_mut().update(
                tree,
                event,
                layout,
                content_cursor,
                renderer,
                clipboard,
                shell,
                &content_viewport,
            );
        }
        if shell.is_event_captured() || !self.interactive {
            return;
        }

        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                // Hover drives the selection: the album whose cover is under the cursor, or
                // nothing when the cursor is over none. Reacting to actual cursor movement (not
                // per-frame hit tests) keeps keyboard navigation stable while it scrolls content
                // under a stationary mouse.
                let hovered = layout.children().position(|cover| content_cursor.is_over(cover.bounds()));
                if state.selected != hovered {
                    state.selected = hovered;
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * WHEEL_LINE,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                state.offset = (offset - dy).clamp(0.0, geom.max_offset(n, bounds.height));
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if cursor.is_over(bounds) => {
                // A click on a cover asks for its track menu. Hover has normally already selected
                // it, but select anyway: a click can land without a preceding move (e.g. through a
                // just-focused window).
                if let Some(on_menu) = self.on_menu
                    && let Some(ix) = layout.children().position(|cover| content_cursor.is_over(cover.bounds()))
                {
                    state.selected = Some(ix);
                    shell.publish(on_menu(ix));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(named), modifiers, repeat, .. })
                if n > 0 =>
            {
                let handled = match named {
                    Named::ArrowLeft if modifiers.is_empty() => self.step(state, geom, bounds.height, Dir::Left),
                    Named::ArrowRight if modifiers.is_empty() => self.step(state, geom, bounds.height, Dir::Right),
                    Named::ArrowUp if modifiers.is_empty() => self.step(state, geom, bounds.height, Dir::Up),
                    Named::ArrowDown if modifiers.is_empty() => self.step(state, geom, bounds.height, Dir::Down),
                    // One album per press: holding Space must not machine-gun the queue.
                    Named::Space if modifiers.is_empty() && !repeat => match state.selected {
                        Some(ix) => {
                            shell.publish((self.on_queue)(ix));
                            true
                        }
                        None => false,
                    },
                    Named::Space if *modifiers == keyboard::Modifiers::CTRL && !repeat => match state.selected {
                        Some(ix) => {
                            shell.publish((self.on_play)(ix));
                            true
                        }
                        None => false,
                    },
                    Named::Enter if modifiers.is_empty() && !repeat => match (self.on_menu, state.selected) {
                        (Some(on_menu), Some(ix)) => {
                            shell.publish(on_menu(ix));
                            true
                        }
                        _ => false,
                    },
                    _ => false,
                };
                if handled {
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            _ => {}
        }

        // Mirror every selection change back to the externalized store (see `selected`).
        if let Some(selection) = self.selection
            && state.selected != before
        {
            shell.publish((selection.notify)(state.selected));
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();
        let offset = state.offset.clamp(0.0, self.geom(bounds.width).max_offset(self.cards.len(), bounds.height));
        let content_cursor = match cursor.position_over(bounds) {
            Some(position) => mouse::Cursor::Available(position + Vector::new(0.0, offset)),
            None => mouse::Cursor::Unavailable,
        };
        let content_viewport = Rectangle { y: bounds.y + offset, ..bounds };

        let from_children = self
            .cards
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((card, tree), layout)| {
                card.cover.as_widget().mouse_interaction(tree, layout, content_cursor, &content_viewport, renderer)
            })
            .max()
            .unwrap_or_default();

        // A pointer over any cover, since clicking it selects.
        if from_children == mouse::Interaction::None && layout.children().any(|cover| content_cursor.is_over(cover.bounds())) {
            mouse::Interaction::Pointer
        } else {
            from_children
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let Some(visible) = bounds.intersection(viewport) else { return };
        let state = tree.state.downcast_ref::<State>();
        let geom = self.geom(bounds.width);
        let offset = state.offset.clamp(0.0, geom.max_offset(self.cards.len(), bounds.height));
        let content_viewport = Rectangle { y: visible.y + offset, ..visible };
        let content_cursor = match cursor.position_over(bounds) {
            Some(position) => mouse::Cursor::Available(position + Vector::new(0.0, offset)),
            None => mouse::Cursor::Unavailable,
        };
        // Hoisted out of the closure below: `fill_text` needs the renderer mutably.
        let font = renderer.default_font();

        renderer.with_layer(visible, |renderer| {
            renderer.with_translation(Vector::new(0.0, -offset), |renderer| {
                for (ix, ((card, tree), layout)) in self.cards.iter().zip(&tree.children).zip(layout.children()).enumerate() {
                    // Every card rect derives from its laid-out cover square, so the draw can't
                    // disagree with the layout.
                    let cover = layout.bounds();
                    let cell =
                        Rectangle { x: cover.x - CARD_PAD, y: cover.y - CARD_PAD, width: geom.side, height: geom.card_h() };
                    if cell.intersection(&content_viewport).is_none() {
                        continue;
                    }
                    // A translucent weak-primary backdrop marks the selection -- see-through
                    // enough that the black backdrop darkens it and the texts keep contrast.
                    if state.selected == Some(ix) {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: cell,
                                border: Border { radius: 6.0.into(), ..Border::default() },
                                ..renderer::Quad::default()
                            },
                            Color { a: 0.5, ..theme.extended_palette().primary.weak.color },
                        );
                    }
                    card.cover.as_widget().draw(tree, renderer, theme, defaults, layout, content_cursor, &content_viewport);

                    // Title and artist, in the fixed blocks the card height is computed from. The
                    // clip rects enforce the two-line/one-line limits.
                    let title_top = cover.y + cover.height + CARD_SPACING;
                    let title = Rectangle { x: cover.x, y: title_top, width: cover.width, height: TITLE_HEIGHT };
                    let artist_top = title_top + TITLE_HEIGHT + CARD_SPACING;
                    let artist = Rectangle { x: cover.x, y: artist_top, width: cover.width, height: ARTIST_HEIGHT };
                    let block = |content: &str, size: f32, line_height: text::LineHeight, rect: Rectangle| Text {
                        content: content.to_owned(),
                        bounds: rect.size(),
                        size: Pixels(size),
                        line_height,
                        font,
                        align_x: text::Alignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: text::Shaping::Advanced,
                        wrapping: text::Wrapping::default(),
                    };
                    // The title's line height fills its two-line block exactly.
                    let title_line = text::LineHeight::Absolute(Pixels(TITLE_LINE_HEIGHT));
                    renderer.fill_text(
                        block(card.title, TITLE_SIZE, title_line, title),
                        title.position(),
                        defaults.text_color,
                        title,
                    );
                    // The artist a notch dimmer than the title, but bright enough to stay
                    // readable over the selection backdrop.
                    renderer.fill_text(
                        block(card.artist, ARTIST_SIZE, text::LineHeight::default(), artist),
                        artist.position(),
                        Color { a: 0.8, ..defaults.text_color },
                        artist,
                    );
                }
            });
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();
        let offset = state.offset.clamp(0.0, self.geom(bounds.width).max_offset(self.cards.len(), bounds.height));
        let translation = translation - Vector::new(0.0, offset);
        let children = self
            .cards
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .filter_map(|((card, tree), layout)| {
                card.cover.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
            })
            .collect::<Vec<_>>();
        (!children.is_empty()).then(|| overlay::Group::with_children(children).overlay())
    }
}

impl<Message> AlbumGrid<'_, Message> {
    /// Moves the selection one step -- or, with nothing selected, picks up the first album in
    /// view -- then scrolls it fully into view. Returns true (it always handles the key).
    fn step(&self, state: &mut State, geom: Geom, view_h: f32, dir: Dir) -> bool {
        let n = self.cards.len();
        let offset = state.offset.clamp(0.0, geom.max_offset(n, view_h));
        let selected = match state.selected {
            Some(cur) => next_selection(cur.min(n - 1), n, geom.cols, dir),
            None => geom.first_visible(offset, n),
        };
        state.selected = Some(selected);
        if let Some(target) = geom.scroll_target(offset, view_h, selected / geom.cols) {
            state.offset = target.clamp(0.0, geom.max_offset(n, view_h));
        }
        true
    }
}

impl<'a, Message: 'a> From<AlbumGrid<'a, Message>> for Element<'a, Message> {
    fn from(grid: AlbumGrid<'a, Message>) -> Self {
        Element::new(grid)
    }
}

/// The grid's layout geometry, derived from the widget width: everything keyboard navigation,
/// layout, and drawing need to agree on, computed in one place.
#[derive(Clone, Copy)]
struct Geom {
    cols: usize,
    /// The width of a card (cards stretch beyond [`CARD_SIDE`] to fill their row exactly).
    side: f32,
    top: f32,
    bottom: f32,
}

impl Geom {
    fn new(width: f32, top: f32, bottom: f32) -> Self {
        // As many columns as fit the width at the base card size, so albums wrap instead of
        // clipping -- then the cards stretch to use the row fully.
        let width = width - 2.0 * GRID_PADDING;
        let cols = (((width + GRID_SPACING) / (CARD_SIDE + GRID_SPACING)) as usize).max(1);
        let side = ((width - GRID_SPACING * (cols - 1) as f32) / cols as f32).floor().max(CARD_SIDE / 2.0);
        Geom { cols, side, top, bottom }
    }

    /// The height of every card: the cover square plus the fixed text blocks.
    fn card_h(&self) -> f32 {
        self.side + 2.0 * CARD_SPACING + TITLE_HEIGHT + ARTIST_HEIGHT
    }

    /// Vertical distance between consecutive rows' tops.
    fn pitch(&self) -> f32 {
        self.card_h() + ROW_SPACING
    }

    /// The cover square of the card at `ix`, in unscrolled content coordinates relative to the
    /// widget origin.
    fn cover(&self, ix: usize) -> Rectangle {
        let (row, col) = (ix / self.cols, ix % self.cols);
        let inner = self.side - 2.0 * CARD_PAD;
        Rectangle {
            x: GRID_PADDING + col as f32 * (self.side + GRID_SPACING) + CARD_PAD,
            y: self.top + row as f32 * self.pitch() + CARD_PAD,
            width: inner,
            height: inner,
        }
    }

    /// How far the grid can scroll: the content height (clearances included) beyond the viewport.
    fn max_offset(&self, n: usize, view_h: f32) -> f32 {
        let rows = n.div_ceil(self.cols.max(1));
        let content = self.top + rows as f32 * self.pitch() - if rows > 0 { ROW_SPACING } else { 0.0 } + self.bottom;
        (content - view_h).max(0.0)
    }

    /// The first album in view at the given scroll offset: the leftmost album of the topmost row
    /// extending below the viewport top, however slightly.
    fn first_visible(&self, offset: f32, n: usize) -> usize {
        // The first row whose bottom edge (top + row * pitch + card_h) lies strictly below the
        // offset.
        let row = (((offset - self.top - self.card_h()) / self.pitch()).floor() + 1.0).max(0.0) as usize;
        row.min((n - 1) / self.cols) * self.cols
    }

    /// The scroll offset that brings the given row fully into view, or `None` if it already is.
    /// "In view" leaves the top clearance above the row (the floating tabs live there) and keeps
    /// its bottom above the bottom clearance (the player bar); the scroll is minimal -- up-moves
    /// align the row under the top clearance, down-moves align its bottom to the player bar, so
    /// the selection hugs whichever edge it left.
    fn scroll_target(&self, offset: f32, view_h: f32, row: usize) -> Option<f32> {
        let y_top = self.top + row as f32 * self.pitch();
        let y_bottom = y_top + self.card_h();
        if y_top < offset + self.top {
            Some((y_top - self.top).max(0.0))
        } else if y_bottom > offset + view_h - self.bottom {
            Some(y_bottom - (view_h - self.bottom))
        } else {
            None
        }
    }
}

/// The grid index one step from `cur` in `dir`, given `n` albums in `cols` columns. Up/Down move
/// by a row; Left/Right by one album, crossing row boundaries. Up from the top row and Down from
/// the last row stay put; Down into a shorter final row that has no cell in this column lands on
/// the last album. Assumes `n > 0`, `cols >= 1`, and `cur < n`.
fn next_selection(cur: usize, n: usize, cols: usize, dir: Dir) -> usize {
    match dir {
        Dir::Left => cur.saturating_sub(1),
        Dir::Right => (cur + 1).min(n - 1),
        Dir::Up => cur.checked_sub(cols).unwrap_or(cur),
        Dir::Down => {
            let below = cur + cols;
            if below < n {
                below
            } else if cur / cols < (n - 1) / cols {
                n - 1
            } else {
                cur
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // A 13-album grid laid out 5 columns wide: rows [0..4], [5..9], [10..12] (a short last row).
    const N: usize = 13;
    const COLS: usize = 5;

    // Cards at the base width (side 168 -> card_h 231, pitch 255) in an 800px viewport, with the
    // tab clearance above and the player bar below.
    const GEOM: Geom = Geom { cols: COLS, side: 168.0, top: 60.0, bottom: 152.0 };
    const VIEW_H: f32 = 800.0;

    #[test]
    fn horizontal_selection_is_linear_and_clamped() {
        assert_eq!(next_selection(0, N, COLS, Dir::Left), 0, "left saturates at the first album");
        assert_eq!(next_selection(5, N, COLS, Dir::Left), 4, "left crosses the row boundary");
        assert_eq!(next_selection(4, N, COLS, Dir::Right), 5, "right crosses the row boundary");
        assert_eq!(next_selection(N - 1, N, COLS, Dir::Right), N - 1, "right clamps at the last album");
    }

    #[test]
    fn vertical_selection_moves_by_a_row() {
        assert_eq!(next_selection(2, N, COLS, Dir::Up), 2, "up from the top row stays put");
        assert_eq!(next_selection(7, N, COLS, Dir::Up), 2, "up moves back one row, same column");
        assert_eq!(next_selection(2, N, COLS, Dir::Down), 7, "down moves forward one row, same column");
        assert_eq!(next_selection(7, N, COLS, Dir::Down), 12, "down into a full cell below");
    }

    #[test]
    fn down_into_a_short_last_row_lands_on_the_last_album() {
        // Album 9 (row 1, col 4) has no cell directly below -- the last row ends at 12 (col 2).
        assert_eq!(next_selection(9, N, COLS, Dir::Down), N - 1, "no cell below: land on the last album");
        // Album 11 is already in the last row: down stays put.
        assert_eq!(next_selection(11, N, COLS, Dir::Down), 11, "down from the last row stays put");
    }

    #[test]
    fn no_scroll_while_the_row_is_fully_visible() {
        assert_eq!(GEOM.card_h(), 231.0, "the test grid's card height");
        assert_eq!(GEOM.scroll_target(0.0, VIEW_H, 0), None, "row 0 starts in view");
        assert_eq!(GEOM.scroll_target(0.0, VIEW_H, 1), None, "row 1 ends at 546, above the bar at 648");
        assert_eq!(GEOM.scroll_target(408.0, VIEW_H, 2), None, "row 2 is in view once scrolled to it");
    }

    #[test]
    fn scrolls_minimally_to_either_edge() {
        // Row 3 ends at 60 + 3*255 + 231 = 1056; align its bottom to the bar: 1056 - 648.
        assert_eq!(GEOM.scroll_target(0.0, VIEW_H, 3), Some(408.0), "down: align the row bottom to the bar");
        assert_eq!(GEOM.scroll_target(408.0, VIEW_H, 3), None, "and it is then stably in view");
        // Back up from there: align row 0 under the top clearance, i.e. all the way to the top.
        assert_eq!(GEOM.scroll_target(408.0, VIEW_H, 0), Some(0.0), "up: align the row under the top clearance");
    }

    #[test]
    fn first_visible_is_the_topmost_row_below_the_viewport_top() {
        assert_eq!(GEOM.first_visible(0.0, N), 0, "unscrolled: the first album");
        // Row 0's bottom edge sits at 60 + 231 = 291: one visible pixel still counts...
        assert_eq!(GEOM.first_visible(290.0, N), 0, "a sliver of row 0 in view selects it");
        // ...but exactly at (or past) the edge it doesn't.
        assert_eq!(GEOM.first_visible(291.0, N), COLS, "row 0 fully above: row 1's first album");
        assert_eq!(GEOM.first_visible(1e4, N), 2 * COLS, "over-scrolled: clamps to the last row");
    }
}
