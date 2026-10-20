//! What the desktop app's main window is doing, and how each thing it
//! does leads to the next: from the list of games, a game is prepared and
//! plays, and once it ends it is in review, as is a log opened.

use social_deduction_cues::GameName;

/// What the main window shows and what its menus allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The list of games, each with its own **Play**.
    Start,
    /// The list of games, with a game of this one being prepared.
    Preparing(GameName),
    /// A game playing.
    Playing,
    /// The game just played, in review from its end.
    ReviewingGame,
    /// A log opened, in review.
    ReviewingLog,
}

impl Mode {
    /// The game being prepared.
    pub fn preparing(self) -> Option<GameName> {
        match self {
            Mode::Preparing(game) => Some(game),
            _ => None,
        }
    }

    /// Whether the window lists the games, which take the theme the
    /// settings hold.
    pub fn listing(self) -> bool {
        matches!(self, Mode::Start | Mode::Preparing(_))
    }

    /// Whether a game is in review.
    pub fn reviewing(self) -> bool {
        matches!(self, Mode::ReviewingGame | Mode::ReviewingLog)
    }

    /// Whether a game is being prepared or playing, which **Stop** stops
    /// and which leaves **Open Game Log…** off.
    pub fn busy(self) -> bool {
        matches!(self, Mode::Preparing(_) | Mode::Playing)
    }

    /// Whether the game just played is over, so its log saves.
    pub fn saveable(self) -> bool {
        self == Mode::ReviewingGame
    }

    /// After a cue of the game being prepared or playing, which is the
    /// game's last when `over`. A cue that comes once the window has left
    /// the game goes by.
    pub fn cued(self, over: bool) -> Option<Mode> {
        match self {
            Mode::Preparing(_) | Mode::Playing if over => Some(Mode::ReviewingGame),
            Mode::Preparing(_) | Mode::Playing => Some(Mode::Playing),
            _ => None,
        }
    }

    /// After the game being prepared started.
    pub fn started(self) -> Mode {
        match self {
            Mode::Preparing(_) => Mode::Playing,
            mode => mode,
        }
    }

    /// After the game being prepared could not start.
    pub fn failed(self) -> Mode {
        match self {
            Mode::Preparing(_) => Mode::Start,
            mode => mode,
        }
    }

    /// After **New Game**: the list of games, once the game being prepared
    /// has stopped.
    pub fn new_game(self) -> Mode {
        match self {
            Mode::Preparing(_) => self,
            _ => Mode::Start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Mode; 5] = [
        Mode::Start,
        Mode::Preparing(GameName::Werewolf),
        Mode::Playing,
        Mode::ReviewingGame,
        Mode::ReviewingLog,
    ];

    /// The modes for which `is` holds.
    fn which(is: fn(Mode) -> bool) -> Vec<Mode> {
        ALL.into_iter().filter(|&mode| is(mode)).collect()
    }

    #[test]
    fn a_game_is_prepared_plays_and_ends_in_review() {
        let preparing = Mode::Preparing(GameName::Avalon);
        assert_eq!(preparing.preparing(), Some(GameName::Avalon));
        assert_eq!(preparing.cued(false), Some(Mode::Playing));
        assert_eq!(Mode::Playing.cued(false), Some(Mode::Playing));
        assert_eq!(Mode::Playing.cued(true), Some(Mode::ReviewingGame));
        assert_eq!(preparing.cued(true), Some(Mode::ReviewingGame));
        assert_eq!(Mode::Playing.preparing(), None);
    }

    #[test]
    fn a_game_plays_once_started_and_goes_back_to_the_games_when_it_could_not_start() {
        let preparing = Mode::Preparing(GameName::Werewolf);
        assert_eq!(preparing.started(), Mode::Playing);
        assert_eq!(preparing.failed(), Mode::Start);
        assert_eq!(Mode::ReviewingGame.started(), Mode::ReviewingGame);
        assert_eq!(Mode::Start.failed(), Mode::Start);
    }

    #[test]
    fn a_cue_once_the_game_is_left_goes_by() {
        assert_eq!(
            which(|mode| mode.cued(true).is_none()),
            [Mode::Start, Mode::ReviewingGame, Mode::ReviewingLog]
        );
    }

    #[test]
    fn new_game_goes_back_to_the_games_once_preparing_stops() {
        for mode in ALL {
            let expected = if mode.preparing().is_some() {
                mode
            } else {
                Mode::Start
            };
            assert_eq!(mode.new_game(), expected, "{mode:?}");
        }
    }

    #[test]
    fn each_mode_shows_the_games_a_game_or_a_review() {
        let preparing = Mode::Preparing(GameName::Werewolf);
        assert_eq!(which(Mode::listing), [Mode::Start, preparing]);
        assert_eq!(
            which(Mode::reviewing),
            [Mode::ReviewingGame, Mode::ReviewingLog]
        );
    }

    #[test]
    fn a_log_opens_unless_a_game_is_busy_and_saves_once_the_game_is_over() {
        let preparing = Mode::Preparing(GameName::Werewolf);
        assert_eq!(which(Mode::busy), [preparing, Mode::Playing]);
        assert_eq!(which(Mode::saveable), [Mode::ReviewingGame]);
    }
}
