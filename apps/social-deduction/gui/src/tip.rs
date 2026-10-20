//! Hover and focus help: an element's `data-tip` attribute says what it
//! is and what changing it does, and [`Tips`] shows it beside the element
//! while the pointer is over it or it has the keyboard's focus, and not
//! while it is pressed. An element's `data-tip-at` names a part of it
//! that the tip shows beside instead.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// How far a tip keeps from its element and from the window's edges, in
/// pixels.
const GAP: f64 = 8.0;

/// A box on the screen, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// Its left edge.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
    /// Its bottom edge.
    pub bottom: f64,
}

/// Where a tip `width` by `height` goes for an element at `element`, in a
/// window `across` by `down`: under the element, from its left edge, kept
/// inside the window, or above the element when it would run off the
/// bottom.
pub fn placed(
    element: Bounds,
    (width, height): (f64, f64),
    (across, down): (f64, f64),
) -> (f64, f64) {
    let left = element.left.min(across - width - GAP).max(GAP);
    let under = element.bottom + GAP;
    let top = if under + height > down - GAP {
        element.top - height - GAP
    } else {
        under
    };
    (left, top)
}

/// The tip of the element `target` is in, and that element, if it has
/// one that isn't empty.
fn tipped(target: Option<web_sys::EventTarget>) -> Option<(web_sys::Element, String)> {
    let element = target?.dyn_into::<web_sys::Element>().ok()?;
    let tipped = element.closest("[data-tip]:not([data-tip=''])").ok()??;
    let tip = tipped.get_attribute("data-tip")?;
    Some((tipped, tip))
}

/// The tip that shows each element's `data-tip`, for as long as the page
/// is open.
#[component]
pub fn Tips() -> impl IntoView {
    let shown = NodeRef::<html::Div>::new();
    // The element whose tip shows, if one does.
    let current = StoredValue::new_local(None::<web_sys::Element>);
    // Pressing hides the tip until the press ends, as when dragging.
    let pressed = StoredValue::new(false);
    let hide = move || {
        if current.get_value().is_some()
            && let Some(tip) = shown.get_untracked()
        {
            tip.set_hidden(true);
            current.set_value(None);
        }
    };
    let show = move |target: Option<web_sys::EventTarget>| {
        let Some(tip) = shown.get_untracked().filter(|_| !pressed.get_value()) else {
            return;
        };
        let Some((element, text)) = tipped(target) else {
            hide();
            return;
        };
        // Moving within the element leaves its tip where it is.
        if current.with_value(|current| current.as_ref() == Some(&element)) {
            return;
        }
        current.set_value(Some(element.clone()));
        tip.set_text_content(Some(&text));
        tip.set_hidden(false);
        // The part `data-tip-at` names, if any, places the tip.
        let at = element.get_attribute("data-tip-at");
        let placer = at
            .and_then(|at| element.query_selector(&at).ok().flatten())
            .unwrap_or_else(|| element.clone());
        let rect = placer.get_bounding_client_rect();
        let window = window();
        let size = |length: Result<JsValue, JsValue>| {
            length.ok().and_then(|l| l.as_f64()).unwrap_or_default()
        };
        let (left, top) = placed(
            Bounds {
                left: rect.left(),
                top: rect.top(),
                bottom: rect.bottom(),
            },
            (
                f64::from(tip.offset_width()),
                f64::from(tip.offset_height()),
            ),
            (size(window.inner_width()), size(window.inner_height())),
        );
        let style = web_sys::HtmlElement::style(&tip);
        let _ = style.set_property("left", &format!("{left}px"));
        let _ = style.set_property("top", &format!("{top}px"));
    };
    // Kept for as long as the page is open.
    let _ = window_event_listener(ev::mouseover, move |event| show(event.target()));
    let _ = window_event_listener(ev::focusin, move |event| show(event.target()));
    let _ = window_event_listener(ev::focusout, move |_| hide());
    let _ = window_event_listener(ev::pointerdown, move |_| {
        pressed.set_value(true);
        hide();
    });
    let released = move |_| pressed.set_value(false);
    let _ = window_event_listener(ev::pointerup, released);
    let _ = window_event_listener(ev::pointercancel, released);
    let _ = window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" {
            hide();
        }
    });
    // Scrolling anything moves the element away from its tip, and a
    // scroll does not bubble, so it is heard on the way down.
    let scrolled = Closure::<dyn FnMut()>::new(hide).into_js_value();
    let _ = document().add_event_listener_with_callback_and_bool(
        "scroll",
        scrolled.unchecked_ref(),
        true,
    );
    view! { <div class="sd-tip" role="tooltip" node_ref=shown hidden></div> }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: (f64, f64) = (800.0, 600.0);

    #[test]
    fn a_tip_goes_under_its_element_from_its_left_edge() {
        let element = Bounds {
            left: 100.0,
            top: 50.0,
            bottom: 70.0,
        };
        assert_eq!(placed(element, (200.0, 40.0), WINDOW), (100.0, 78.0));
    }

    #[test]
    fn a_tip_stays_inside_the_window() {
        let right = Bounds {
            left: 700.0,
            top: 50.0,
            bottom: 70.0,
        };
        assert_eq!(placed(right, (200.0, 40.0), WINDOW), (592.0, 78.0));
        let left = Bounds {
            left: -20.0,
            top: 50.0,
            bottom: 70.0,
        };
        assert_eq!(placed(left, (200.0, 40.0), WINDOW), (8.0, 78.0));
    }

    #[test]
    fn a_tip_goes_above_an_element_near_the_bottom() {
        let element = Bounds {
            left: 100.0,
            top: 560.0,
            bottom: 580.0,
        };
        assert_eq!(placed(element, (200.0, 40.0), WINDOW), (100.0, 512.0));
    }
}
