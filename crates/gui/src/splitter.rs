//! The handle on the line between a pane at one edge of the window and
//! what shares the window with it: dragging it sets how wide the pane is,
//! the keys move it, and a double click puts it back.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The edge of the window a pane is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The pane is on the left, and what shares the window is right of
    /// it.
    Left,
    /// The pane is on the right.
    Right,
}

/// How wide a pane gets, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    /// How wide it starts.
    pub start: f64,
    /// The narrowest it gets.
    pub least: f64,
    /// The narrowest the rest of the main area gets.
    pub rest: f64,
    /// How far an arrow key moves the line.
    pub step: f64,
}

impl Widths {
    /// The widest the pane gets in a main area `main` wide: what leaves
    /// the rest its least, and never under the pane's least.
    pub(crate) fn widest(self, main: f64) -> f64 {
        (main - self.rest).max(self.least)
    }

    /// The pane's width `width` in a main area `main` wide, kept between
    /// the narrowest pane and the widest.
    pub(crate) fn clamped(self, width: f64, main: f64) -> f64 {
        width.clamp(self.least, self.widest(main))
    }

    /// The pane's width once the line on its `edge` is `key`ed from
    /// `width` in a main area `main` wide, or none when `key` doesn't
    /// move it. An arrow toward the middle of the window widens the pane.
    /// As in a window splitter, Home takes the pane to its narrowest and
    /// End to its widest.
    pub fn keyed(self, key: &str, edge: Edge, width: f64, main: f64) -> Option<f64> {
        let toward = match (key, edge) {
            ("ArrowRight", Edge::Left) | ("ArrowLeft", Edge::Right) => 1.0,
            ("ArrowLeft" | "ArrowRight", _) => -1.0,
            ("Home", _) => return Some(self.least),
            ("End", _) => return Some(self.widest(main)),
            _ => return None,
        };
        Some(self.clamped(width + toward * self.step, main))
    }
}

/// A drag of the handle, from where it was grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Grab {
    /// How wide the pane gets.
    pub widths: Widths,
    /// The edge the pane is on.
    pub edge: Edge,
    /// The main area's width.
    pub main: f64,
    /// Where the pointer would stand with the pane at no width: the
    /// pane's width at the grab, away from where it grabbed toward the
    /// pane's edge.
    pub zero: f64,
}

impl Grab {
    /// A grab at `x` of the handle on a pane `width` wide on `edge`, in a
    /// main area `main` wide.
    pub fn new(widths: Widths, edge: Edge, main: f64, width: f64, x: f64) -> Self {
        let zero = match edge {
            Edge::Left => x - width,
            Edge::Right => x + width,
        };
        Self {
            widths,
            edge,
            main,
            zero,
        }
    }

    /// The pane's width with the pointer at `x`: the line moves as far as
    /// the pointer does.
    pub(crate) fn dragged(self, x: f64) -> f64 {
        let width = match self.edge {
            Edge::Left => x - self.zero,
            Edge::Right => self.zero - x,
        };
        self.widths.clamped(width, self.main)
    }
}

/// A pane's width: the one the handle chose, kept within the main area it
/// shares.
#[derive(Debug, Clone, Copy)]
pub struct Split {
    /// How wide the pane gets.
    widths: Widths,
    /// The width the handle chose.
    chosen: RwSignal<f64>,
    /// The main area's width, once measured.
    main: RwSignal<Option<f64>>,
    /// How wide the pane shows.
    shown: Memo<f64>,
}

impl Split {
    /// A pane at its starting width.
    pub fn new(widths: Widths) -> Self {
        let chosen = RwSignal::new(widths.start);
        let main = RwSignal::new(None);
        let shown = Memo::new(move |_| {
            let chosen = chosen.get();
            main.get()
                .map_or(chosen, |main| widths.clamped(chosen, main))
        });
        Self {
            widths,
            chosen,
            main,
            shown,
        }
    }

    /// How wide the pane shows.
    pub fn width(self) -> f64 {
        self.shown.get()
    }
}

/// The handle, on the inner edge of a pane on the window's `edge`,
/// setting its width in `split`. It goes between the pane and what shares
/// the main area with it, and measures that area whenever it changes
/// size. `label` names what it sets for a screen reader, and `tip` is its
/// help.
#[component]
pub fn Splitter(
    /// The pane's width.
    split: Split,
    /// The edge of the window the pane is on.
    edge: Edge,
    /// What the handle sets, as a screen reader names it.
    label: &'static str,
    /// The handle's help.
    tip: &'static str,
) -> impl IntoView {
    let widths = split.widths;
    let handle = NodeRef::<html::Div>::new();
    let observer = StoredValue::new_local(None::<web_sys::ResizeObserver>);
    handle.on_load(move |handle| {
        let Some(area) = handle.parent_element() else {
            return;
        };
        let measured = area.clone();
        let measure = Closure::<dyn FnMut()>::new(move || {
            let main = Some(measured.get_bounding_client_rect().width());
            if main != split.main.get_untracked() {
                split.main.set(main);
            }
        });
        if let Ok(watching) = web_sys::ResizeObserver::new(measure.into_js_value().unchecked_ref())
        {
            watching.observe(&area);
            observer.set_value(Some(watching));
        }
    });
    on_cleanup(move || {
        if let Some(Some(watching)) = observer.try_get_value() {
            watching.disconnect();
        }
    });
    // While a drag is on, the page selects no text.
    let dragging = |on: bool| {
        if let Some(body) = document().body() {
            let _ = body.class_list().toggle_with_force("fa-dragging", on);
        }
    };
    let held = StoredValue::new(None::<Grab>);
    let grab = move |event: web_sys::PointerEvent| {
        if event.button() == 0
            && let Some(handle) = handle.get_untracked()
            && let Some(main) = split.main.get_untracked()
        {
            event.prevent_default();
            let x = f64::from(event.client_x());
            let width = split.shown.get_untracked();
            held.set_value(Some(Grab::new(widths, edge, main, width, x)));
            dragging(true);
            let _ = handle.set_pointer_capture(event.pointer_id());
        }
    };
    let drag = move |event: web_sys::PointerEvent| {
        if let Some(grab) = held.get_value() {
            let to = grab.dragged(f64::from(event.client_x()));
            // Dragging on past a limit redraws nothing.
            if to != split.chosen.get_untracked() {
                split.chosen.set(to);
            }
        }
    };
    let release = move |_| {
        held.set_value(None);
        dragging(false);
    };
    let key = move |event: web_sys::KeyboardEvent| {
        let Some(main) = split.main.get_untracked() else {
            return;
        };
        if let Some(to) = widths.keyed(&event.key(), edge, split.shown.get_untracked(), main) {
            event.prevent_default();
            split.chosen.set(to);
        }
    };
    view! {
        <div
            class="fa-splitter"
            class:fa-splitter-left=edge == Edge::Left
            role="separator"
            aria-orientation="vertical"
            aria-label=label
            aria-valuenow=move || split.width().round()
            aria-valuemin=widths.least
            aria-valuemax=move || split.main.get().map(|main| widths.widest(main).round())
            tabindex="0"
            data-tip=tip
            data-tip-at=".fa-grip"
            node_ref=handle
            on:pointerdown=grab
            on:pointermove=drag
            on:lostpointercapture=release
            on:keydown=key
            on:dblclick=move |_| split.chosen.set(widths.start)
        >
            <span class="fa-grip"></span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane that starts at 400 px, narrows to 240, leaves the rest 320
    /// and steps 16.
    const WIDTHS: Widths = Widths {
        start: 400.0,
        least: 240.0,
        rest: 320.0,
        step: 16.0,
    };

    #[test]
    fn a_width_between_the_limits_stands() {
        assert_eq!(WIDTHS.clamped(400.0, 1280.0), 400.0);
    }

    #[test]
    fn the_pane_narrows_no_further_than_its_least() {
        assert_eq!(WIDTHS.clamped(100.0, 1280.0), WIDTHS.least);
    }

    #[test]
    fn the_rest_narrows_no_further_than_its_least() {
        assert_eq!(WIDTHS.clamped(1200.0, 1280.0), 1280.0 - WIDTHS.rest);
    }

    #[test]
    fn an_arrow_toward_the_middle_widens_the_pane_and_one_toward_its_edge_narrows_it() {
        let step = WIDTHS.step;
        assert_eq!(
            WIDTHS.keyed("ArrowLeft", Edge::Right, 400.0, 1280.0),
            Some(400.0 + step)
        );
        assert_eq!(
            WIDTHS.keyed("ArrowRight", Edge::Right, 400.0, 1280.0),
            Some(400.0 - step)
        );
        assert_eq!(
            WIDTHS.keyed("ArrowRight", Edge::Left, 400.0, 1280.0),
            Some(400.0 + step)
        );
        assert_eq!(
            WIDTHS.keyed("ArrowLeft", Edge::Left, 400.0, 1280.0),
            Some(400.0 - step)
        );
    }

    #[test]
    fn home_takes_the_pane_to_its_narrowest_and_end_to_its_widest() {
        for edge in [Edge::Left, Edge::Right] {
            assert_eq!(
                WIDTHS.keyed("Home", edge, 400.0, 1280.0),
                Some(WIDTHS.least)
            );
            assert_eq!(
                WIDTHS.keyed("End", edge, 400.0, 1280.0),
                Some(1280.0 - WIDTHS.rest)
            );
        }
    }

    #[test]
    fn the_widest_pane_leaves_the_rest_its_least() {
        assert_eq!(WIDTHS.widest(1280.0), 1280.0 - WIDTHS.rest);
        assert_eq!(WIDTHS.widest(400.0), WIDTHS.least);
    }

    #[test]
    fn a_grab_keeps_the_line_where_it_was_grabbed() {
        let right = Grab::new(WIDTHS, Edge::Right, 1280.0, 400.0, 904.0);
        assert_eq!(right.zero, 1304.0);
        assert_eq!(right.dragged(904.0), 400.0);
        let left = Grab::new(WIDTHS, Edge::Left, 1280.0, 400.0, 400.0);
        assert_eq!(left.zero, 0.0);
        assert_eq!(left.dragged(400.0), 400.0);
    }

    #[test]
    fn a_drag_moves_the_line_as_far_as_the_pointer() {
        let right = Grab::new(WIDTHS, Edge::Right, 1280.0, 400.0, 904.0);
        assert_eq!(right.dragged(854.0), 450.0);
        assert_eq!(right.dragged(2000.0), WIDTHS.least);
        let left = Grab::new(WIDTHS, Edge::Left, 1280.0, 400.0, 400.0);
        assert_eq!(left.dragged(450.0), 450.0);
        assert_eq!(left.dragged(0.0), WIDTHS.least);
    }

    #[test]
    fn a_key_keeps_the_pane_within_the_limits() {
        let least = WIDTHS.least;
        assert_eq!(
            WIDTHS.keyed("ArrowRight", Edge::Right, least, 1280.0),
            Some(least)
        );
    }

    #[test]
    fn other_keys_leave_the_line_alone() {
        assert_eq!(WIDTHS.keyed("Enter", Edge::Right, 400.0, 1280.0), None);
    }
}
