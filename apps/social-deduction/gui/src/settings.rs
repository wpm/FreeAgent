//! The desktop app's Settings window, Free Agent's: General, Models, and
//! under Games a page for each game, built in [`game`].

use free_agent_gui::settings::{self as window, Page};
use leptos::prelude::*;
use social_deduction_gui_types::GameName;
use social_deduction_gui_types::settings::{game_key, game_title};

mod game;

use game::GameTab;

/// A game's page of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GamePage(pub GameName);

impl Page for GamePage {
    fn all() -> Vec<GamePage> {
        GameName::ALL.map(GamePage).into()
    }

    fn title(self) -> &'static str {
        game_title(self.0)
    }

    fn tip(self) -> String {
        format!(
            "{}'s players, phase limits, strategies and prompt parts.",
            game_title(self.0)
        )
    }

    fn key(self) -> &'static str {
        game_key(self.0)
    }
}

/// The Settings window.
#[component]
pub fn SettingsWindow() -> impl IntoView {
    view! {
        <window::SettingsWindow
            label="Games"
            appearance="Werewolf is dark by night and light by day, whatever the appearance."
            plays="both games"
            page=|(editor, GamePage(game))| view! { <GameTab editor=editor game=game /> }.into_any()
        />
    }
}

#[cfg(test)]
mod tests {
    use free_agent_gui::settings::Tab;

    use super::*;

    #[test]
    fn the_games_are_the_window_s_own_pages() {
        let titles: Vec<&str> = Tab::<GamePage>::all()
            .iter()
            .map(|tab| tab.title())
            .collect();
        assert_eq!(titles, ["General", "Models", "Werewolf", "Avalon"]);
        let ids: Vec<String> = Tab::<GamePage>::all().iter().map(|tab| tab.id()).collect();
        assert_eq!(
            ids,
            ["tab-general", "tab-models", "tab-werewolf", "tab-avalon"]
        );
        assert_eq!(
            GamePage(GameName::Avalon).tip(),
            "Avalon's players, phase limits, strategies and prompt parts."
        );
        assert_eq!(
            Tab::<GamePage>::of(&["werewolf".to_string()]),
            Tab::App(GamePage(GameName::Werewolf))
        );
    }
}
