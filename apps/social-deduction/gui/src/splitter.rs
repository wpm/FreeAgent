//! The handle on the line between the chat and the board: dragging it
//! sets how wide the board is, the keys move it, and a double click puts
//! it back.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// How wide the board starts, in pixels.
pub const BOARD: f64 = 360.0;

/// The narrowest the board gets, in pixels.
pub const MIN_BOARD: f64 = 240.0;

/// The narrowest the chat gets, in pixels.
pub const MIN_CHAT: f64 = 320.0;

/// How far an arrow key moves the line, in pixels.
pub const KEY_STEP: f64 = 16.0;

/// The widest the board gets in a main area `main` wide: what leaves the
/// chat its least, and never under the board's least.
pub fn widest(main: f64) -> f64 {
    (main - MIN_CHAT).max(MIN_BOARD)
}

/// The board's width `width` in a main area `main` wide, kept between the
/// narrowest board and the widest.
pub fn clamped(width: f64, main: f64) -> f64 {
    width.clamp(MIN_BOARD, widest(main))
}

/// The board's width once the line is `key`ed from `width` in a main area
/// `main` wide, or none when `key` doesn't move it. As in a window
/// splitter, Home takes the board to its narrowest and End to its widest.
pub fn keyed(key: &str, width: f64, main: f64) -> Option<f64> {
    match key {
        "ArrowLeft" => Some(clamped(width + KEY_STEP, main)),
        "ArrowRight" => Some(clamped(width - KEY_STEP, main)),
        "Home" => Some(MIN_BOARD),
        "End" => Some(widest(main)),
        _ => None,
    }
}

/// A drag of the handle, from where it was grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grab {
    /// The main area's width.
    pub main: f64,
    /// Where the pointer would stand with the board at no width: the
    /// board's width at the grab, right of where it grabbed.
    pub edge: f64,
}

impl Grab {
    /// A grab at `x` of the handle on a board `width` wide, in a main area
    /// `main` wide.
    pub fn new(main: f64, width: f64, x: f64) -> Self {
        Self {
            main,
            edge: x + width,
        }
    }

    /// The board's width with the pointer at `x`: the line moves as far as
    /// the pointer does.
    pub fn dragged(self, x: f64) -> f64 {
        clamped(self.edge - x, self.main)
    }
}

/// The board's width: the one the handle chose, kept within the main area
/// it shares with the chat.
#[derive(Debug, Clone, Copy)]
pub struct Split {
    /// The width the handle chose.
    chosen: RwSignal<f64>,
    /// The main area's width, once measured.
    main: RwSignal<Option<f64>>,
    /// How wide the board shows.
    shown: Memo<f64>,
}

impl Default for Split {
    /// The board at its starting width.
    fn default() -> Self {
        let chosen = RwSignal::new(BOARD);
        let main = RwSignal::new(None);
        let shown = Memo::new(move |_| {
            let chosen = chosen.get();
            main.get().map_or(chosen, |main| clamped(chosen, main))
        });
        Self {
            chosen,
            main,
            shown,
        }
    }
}

impl Split {
    /// How wide the board shows.
    pub fn width(self) -> f64 {
        self.shown.get()
    }
}

/// The handle, on the board's left edge, setting its width in `split`. It
/// goes between the chat and the board, in the main area they share, and
/// measures that area whenever it changes size.
#[component]
pub fn Splitter(
    /// The board's width.
    split: Split,
) -> impl IntoView {
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
            let _ = body.class_list().toggle_with_force("sd-dragging", on);
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
            held.set_value(Some(Grab::new(main, split.shown.get_untracked(), x)));
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
        if let Some(to) = keyed(&event.key(), split.shown.get_untracked(), main) {
            event.prevent_default();
            split.chosen.set(to);
        }
    };
    view! {
        <div
            class="sd-splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Board width"
            aria-valuenow=move || split.width().round()
            aria-valuemin=MIN_BOARD
            aria-valuemax=move || split.main.get().map(|main| widest(main).round())
            tabindex="0"
            data-tip="Drag to make the board wider or narrower, or use the arrow keys, Home and End. Double-click to put it back."
            data-tip-at=".sd-grip"
            node_ref=handle
            on:pointerdown=grab
            on:pointermove=drag
            on:lostpointercapture=release
            on:keydown=key
            on:dblclick=move |_| split.chosen.set(BOARD)
        >
            <span class="sd-grip"></span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_width_between_the_limits_stands() {
        assert_eq!(clamped(400.0, 1280.0), 400.0);
    }

    #[test]
    fn the_board_narrows_no_further_than_its_least() {
        assert_eq!(clamped(100.0, 1280.0), MIN_BOARD);
    }

    #[test]
    fn the_chat_narrows_no_further_than_its_least() {
        assert_eq!(clamped(1200.0, 1280.0), 1280.0 - MIN_CHAT);
    }

    #[test]
    fn the_left_arrow_widens_the_board_and_the_right_narrows_it() {
        assert_eq!(keyed("ArrowLeft", 400.0, 1280.0), Some(400.0 + KEY_STEP));
        assert_eq!(keyed("ArrowRight", 400.0, 1280.0), Some(400.0 - KEY_STEP));
    }

    #[test]
    fn home_takes_the_board_to_its_narrowest_and_end_to_its_widest() {
        assert_eq!(keyed("Home", 400.0, 1280.0), Some(MIN_BOARD));
        assert_eq!(keyed("End", 400.0, 1280.0), Some(1280.0 - MIN_CHAT));
    }

    #[test]
    fn the_widest_board_leaves_the_chat_its_least() {
        assert_eq!(widest(1280.0), 1280.0 - MIN_CHAT);
        assert_eq!(widest(400.0), MIN_BOARD);
    }

    /// A grab at 904 of the handle on a 400 px board in a 1280 px area.
    const GRAB: Grab = Grab {
        main: 1280.0,
        edge: 1304.0,
    };

    #[test]
    fn a_grab_keeps_the_line_where_it_was_grabbed() {
        assert_eq!(Grab::new(1280.0, 400.0, 904.0), GRAB);
        assert_eq!(GRAB.dragged(904.0), 400.0);
    }

    #[test]
    fn a_drag_moves_the_line_as_far_as_the_pointer() {
        assert_eq!(GRAB.dragged(854.0), 450.0);
        assert_eq!(GRAB.dragged(2000.0), MIN_BOARD);
    }

    #[test]
    fn a_key_keeps_the_board_within_the_limits() {
        assert_eq!(keyed("ArrowRight", MIN_BOARD, 1280.0), Some(MIN_BOARD));
    }

    #[test]
    fn other_keys_leave_the_line_alone() {
        assert_eq!(keyed("Enter", 400.0, 1280.0), None);
    }
}
