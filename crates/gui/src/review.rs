//! Review: stepping through a game log bubble by bubble. The keys move the
//! cursor, and the review header names the file, picks the game and says
//! where the cursor is.

use free_agent_gui_types::go::Go;
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::chrome::SelectBox;

/// Where the key `key` takes the cursor, pressed with a modifier or not:
/// an unmodified arrow to the bubble before or after, and Home or End to
/// either end. A key pressed with a modifier is the menu's.
pub fn keyed(key: &str, modified: bool) -> Option<Go> {
    if modified {
        return None;
    }
    match key {
        "ArrowUp" | "ArrowLeft" => Some(Go::Previous),
        "ArrowDown" | "ArrowRight" => Some(Go::Next),
        "Home" => Some(Go::First),
        "End" => Some(Go::Last),
        _ => None,
    }
}

/// What the header shows of a log under review: the file's `name`, when
/// it came from one; the `game` shown of its `games`, with ‹ › and a list
/// of their `titles` when there are several; the cursor's `position`;
/// then the `children`, which control it. Unmodified arrows and Home and
/// End, pressed anywhere but in a field, go to `on_go`, as do ‹ and ›,
/// and a game picked from the list goes to `on_choose`.
#[component]
pub fn ReviewHeader(
    /// The file the log came from, if it came from one.
    #[prop(into)]
    name: Signal<Option<String>>,
    /// How many games the log holds.
    #[prop(into)]
    games: Signal<usize>,
    /// The game shown, from 0.
    #[prop(into)]
    game: Signal<usize>,
    /// Each game's title, for the list.
    #[prop(into)]
    titles: Signal<Vec<String>>,
    /// Where the cursor is, as `1 / 212`.
    #[prop(into)]
    position: Signal<Option<String>>,
    /// Moves the cursor.
    on_go: Callback<Go>,
    /// Shows a game, from its first bubble.
    on_choose: Callback<usize>,
    /// What controls the review.
    children: Children,
) -> impl IntoView {
    let keys = window_event_listener(ev::keydown, move |event| {
        let modified = event.alt_key() || event.ctrl_key() || event.meta_key() || event.shift_key();
        let typing = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|element| {
                matches!(element.tag_name().as_str(), "INPUT" | "SELECT" | "TEXTAREA")
            });
        if event.default_prevented() || typing {
            return;
        }
        if let Some(go) = keyed(&event.key(), modified) {
            event.prevent_default();
            on_go.run(go);
        }
    });
    on_cleanup(move || keys.remove());
    let chosen = move |event: ev::Event| {
        if let Ok(game) = event_target_value(&event).parse() {
            on_choose.run(game);
        }
    };
    let games_shown = move || {
        (games.get() > 1).then(|| {
            let titles = titles.get();
            view! {
                <span class="fa-review-games">
                    <button class="fa-small" on:click=move |_| on_go.run(Go::PreviousGame) disabled=move || game.get() == 0 aria-label="Previous game" data-tip="The game before, from its first bubble.">"‹"</button>
                    <span class="fa-review-game">{move || format!("Game {} of {}", game.get() + 1, games.get())}</span>
                    <button class="fa-small" on:click=move |_| on_go.run(Go::NextGame) disabled=move || { game.get() + 1 >= games.get() } aria-label="Next game" data-tip="The game after, from its first bubble.">"›"</button>
                    <SelectBox>
                        <select class="fa-select" on:change=chosen prop:value=move || game.get().to_string() aria-label="Game" data-tip="Each game in the log, by its name and how it ended.">
                            {titles
                                .into_iter()
                                .enumerate()
                                .map(|(index, title)| view! { <option value=index.to_string()>{title}</option> })
                                .collect_view()}
                        </select>
                    </SelectBox>
                </span>
            }
        })
    };
    view! {
        {move || name.get().map(|name| view! { <span class="fa-review-name">{name}</span> })}
        {games_shown}
        <span class="fa-review-position" data-tip="The bubble you're on. Step with the arrow keys, jump to either end with Home and End, or click a bubble. The Go menu moves by chapter and by game too.">{position}</span>
        {children()}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmodified_arrows_and_home_and_end_move_the_cursor_and_modified_keys_are_the_menu_s() {
        assert_eq!(keyed("ArrowUp", false), Some(Go::Previous));
        assert_eq!(keyed("ArrowLeft", false), Some(Go::Previous));
        assert_eq!(keyed("ArrowDown", false), Some(Go::Next));
        assert_eq!(keyed("ArrowRight", false), Some(Go::Next));
        assert_eq!(keyed("Home", false), Some(Go::First));
        assert_eq!(keyed("End", false), Some(Go::Last));
        assert_eq!(keyed("a", false), None);
        for key in [
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Home",
            "End",
        ] {
            assert_eq!(keyed(key, true), None);
        }
    }
}
