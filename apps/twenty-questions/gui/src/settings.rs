//! The desktop app's Settings window, Free Agent's: General, Models, and
//! the Game page.

use free_agent_gui::settings::{self as window, Page};
use leptos::prelude::*;
use twenty_questions_gui_types::settings::GAME;

mod game;

use game::GameTab;

/// The game's page of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GamePage;

impl Page for GamePage {
    fn all() -> Vec<GamePage> {
        vec![GamePage]
    }

    fn title(self) -> &'static str {
        "Game"
    }

    fn tip(self) -> String {
        "The players, the host and its secret object, and the strategy the players play."
            .to_string()
    }

    fn key(self) -> &'static str {
        GAME
    }
}

/// The Settings window.
#[component]
pub fn SettingsWindow() -> impl IntoView {
    view! {
        <window::SettingsWindow
            appearance="The game follows it too."
            plays="Twenty Questions"
            page=|(editor, GamePage)| view! { <GameTab editor=editor /> }.into_any()
        />
    }
}

#[cfg(test)]
mod tests {
    use free_agent_gui::settings::Tab;

    use super::*;

    #[test]
    fn the_game_is_the_window_s_own_page() {
        let titles: Vec<&str> = Tab::<GamePage>::all()
            .iter()
            .map(|tab| tab.title())
            .collect();
        assert_eq!(titles, ["General", "Models", "Game"]);
        assert_eq!(Tab::App(GamePage).id(), "tab-game");
        assert!(GamePage.tip().starts_with("The players"));
        assert_eq!(
            Tab::<GamePage>::of(&["game".to_string(), "players".to_string()]),
            Tab::App(GamePage)
        );
    }
}
