//! The handle on the line between the chat and the board: dragging it
//! sets how wide the board is, the arrow keys step it, and a double click
//! puts it back.

use leptos::html;
use leptos::prelude::*;

/// How wide the board starts, in pixels.
pub const BOARD: f64 = 360.0;

/// The narrowest the board gets, in pixels.
pub const MIN_BOARD: f64 = 240.0;

/// The narrowest the chat gets, in pixels.
pub const MIN_CHAT: f64 = 320.0;

/// How far an arrow key moves the line, in pixels.
pub const KEY_STEP: f64 = 16.0;

/// The board's width `width` in a main area `main` wide, kept between the
/// narrowest board and the narrowest chat.
pub fn clamped(width: f64, main: f64) -> f64 {
    width.min(main - MIN_CHAT).max(MIN_BOARD)
}

/// The board's width once the line is `key`ed from `width` in a main area
/// `main` wide, or none when `key` doesn't move it.
pub fn keyed(key: &str, width: f64, main: f64) -> Option<f64> {
    let to = match key {
        "ArrowLeft" => width + KEY_STEP,
        "ArrowRight" => width - KEY_STEP,
        "Home" => f64::INFINITY,
        "End" => 0.0,
        _ => return None,
    };
    Some(clamped(to, main))
}

/// The handle, on the board's left edge, setting the board's `width`. It
/// goes between the chat and the board, in the area they share.
#[component]
pub fn Splitter(
    /// The board's width, in pixels.
    width: RwSignal<f64>,
) -> impl IntoView {
    let handle = NodeRef::<html::Div>::new();
    // The shared area's right edge and width, where the board's column ends.
    let area = move || {
        let area = handle.get_untracked()?.parent_element()?;
        let rect = area.get_bounding_client_rect();
        Some((rect.right(), rect.width()))
    };
    let grab = move |event: web_sys::PointerEvent| {
        if event.button() == 0
            && let Some(handle) = handle.get_untracked()
        {
            // Dragging selects no text.
            event.prevent_default();
            let _ = handle.set_pointer_capture(event.pointer_id());
        }
    };
    let drag = move |event: web_sys::PointerEvent| {
        let held = handle
            .get_untracked()
            .is_some_and(|handle| handle.has_pointer_capture(event.pointer_id()));
        if held && let Some((right, main)) = area() {
            let to = clamped(right - f64::from(event.client_x()), main);
            // Dragging on past a limit redraws nothing.
            if to != width.get_untracked() {
                width.set(to);
            }
        }
    };
    let key = move |event: web_sys::KeyboardEvent| {
        let Some((_, main)) = area() else {
            return;
        };
        if let Some(to) = keyed(&event.key(), width.get_untracked(), main) {
            event.prevent_default();
            width.set(to);
        }
    };
    view! {
        <div
            class="sd-splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Board width"
            aria-valuenow=move || width.get().round()
            aria-valuemin=MIN_BOARD
            tabindex="0"
            data-tip="Drag to make the board wider or narrower. Double-click to put it back."
            node_ref=handle
            on:pointerdown=grab
            on:pointermove=drag
            on:keydown=key
            on:dblclick=move |_| width.set(BOARD)
        ></div>
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
    fn home_and_end_take_the_line_to_its_limits() {
        assert_eq!(keyed("Home", 400.0, 1280.0), Some(1280.0 - MIN_CHAT));
        assert_eq!(keyed("End", 400.0, 1280.0), Some(MIN_BOARD));
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
