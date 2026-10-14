//! What a viewer is drawn from: a game's log, folded an event at a time
//! into [`Cue`]s.
//!
//! A [`Stage`] reads the same events the console's narrator does and
//! keeps the narrator's words. The deal becomes the cast, each narrator
//! line a heading, a block of narration or an utterance, and the end of
//! the game its outcome. Votes and selections stay out of the cues: the
//! game's [`Board`] shows them, whole, after every event that moves it.

use crate::avalon::{self, Avalon};
use crate::game::{Entry, Game, Narrate};
use crate::report::{Line, in_order};
use crate::seat::PlayerId;
use crate::werewolf::{self, Werewolf};
use anyhow::Context;
use free_agent::Event;
use social_deduction_stage::{Beat, Cue, GameName, Millis, Outcome, Panel, Role, Seat, Side};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// What a game supplies for its log to be folded into cues: its name, how
/// its roles and sides are drawn, how it is won, and its board.
pub trait Staged: Game {
    /// The game's name, which picks the board the viewer draws.
    const GAME: GameName;
    /// The board this game draws.
    type Board: Board<Self>;

    /// `role` as the viewer colors it.
    fn role(role: Self::Role) -> Role;

    /// The side `role` wins with.
    fn side(role: Self::Role) -> Side;

    /// The side that won, if `event` ends the game.
    fn winner(event: &Self::Event) -> Option<Side>;
}

/// A game's board, moved by the log's events.
pub trait Board<G: Game>: Default + Send + 'static {
    /// The whole board after `event`, when `event` changes it.
    fn fold(&mut self, event: &Event<Entry<G>>) -> Option<Panel>;
}

/// No board at all: every event leaves it as it is.
impl<G: Game> Board<G> for () {
    fn fold(&mut self, _: &Event<Entry<G>>) -> Option<Panel> {
        None
    }
}

/// Folds one game's log into cues, an event at a time.
pub struct Stage<G: Staged> {
    /// The console's narrator, whose lines the cues carry.
    narrator: G::Narrator,
    /// The game's board.
    board: G::Board,
}

impl<G: Staged> Default for Stage<G> {
    fn default() -> Self {
        Self {
            narrator: G::Narrator::default(),
            board: G::Board::default(),
        }
    }
}

impl<G: Staged> Stage<G> {
    /// The cues `event` adds, each stamped with the event's time. The deal
    /// is the cast, then the clock at its live rate. Every other event is
    /// what its narrator lines tell: each heading a heading, each run of
    /// told lines one block, each utterance a bubble from its speaker,
    /// votes and blank lines left out. The board follows when the event
    /// moves it, and the end of the game is its outcome, last, won as the
    /// narrator tells it.
    pub fn fold(&mut self, event: &Event<Entry<G>>) -> Vec<Cue> {
        let at = millis(event.time);
        let lines = self.narrator.narrate(event);
        let mut beats = match &event.payload {
            Entry::Start {
                roles, strategies, ..
            } => vec![cast::<G>(roles, strategies), Beat::Clock { rate: 1.0 }],
            _ => beats(&lines),
        };
        beats.extend(self.board.fold(event).map(Beat::Panel));
        if let Entry::Event(own) = &event.payload
            && let Some(side) = G::winner(own)
        {
            let outcome = Outcome::Won {
                side,
                how: how(&lines),
            };
            beats.push(Beat::Over { outcome, log: None });
        }
        beats.into_iter().map(|beat| Cue { at, beat }).collect()
    }
}

/// The cues a game's log folds into, the log written as JSON Lines by a
/// game of either kind.
///
/// # Errors
///
/// A line that is an event of neither game.
pub fn cues(log: &str) -> anyhow::Result<Vec<Cue>> {
    read::<Avalon>(log)
        .or_else(|_| read::<Werewolf>(log))
        .context("the log is of neither Avalon nor Werewolf")
}

/// The cues the log of a game of `G` folds into.
fn read<G: Staged>(log: &str) -> serde_json::Result<Vec<Cue>> {
    let mut stage = Stage::<G>::default();
    let mut cues = Vec::new();
    for line in log.lines().filter(|line| !line.trim().is_empty()) {
        let event: Event<Entry<G>> = serde_json::from_str(line)?;
        cues.extend(stage.fold(&event));
    }
    Ok(cues)
}

/// `time` in milliseconds since the Unix epoch.
pub(crate) fn millis(time: SystemTime) -> Millis {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    Millis::try_from(since.as_millis()).unwrap_or(Millis::MAX)
}

/// The cast of a game dealt `roles` and `strategies`: every seat in seat
/// order, labeled `Player 3 · Seer`.
fn cast<G: Staged>(
    roles: &HashMap<PlayerId, G::Role>,
    strategies: &HashMap<PlayerId, String>,
) -> Beat {
    let seats = in_order(roles.keys())
        .into_iter()
        .map(|name| {
            let role = roles[name];
            Seat {
                name: name.clone(),
                label: format!("{name} · {}", capitalized(&role.to_string())),
                role: G::role(role),
                side: G::side(role),
                strategy: strategies[name].clone(),
            }
        })
        .collect();
    Beat::Cast {
        game: G::GAME,
        seats,
    }
}

/// `word` with its first letter in upper case.
fn capitalized(word: &str) -> String {
    let mut letters = word.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// What narrator `lines` show: each heading, each run of told lines as one
/// block, and each utterance, in order.
fn beats(lines: &[Line]) -> Vec<Beat> {
    let mut beats = Vec::new();
    let mut told = Vec::new();
    for line in lines {
        if let Line::Told(text) = line {
            told.push(text.clone());
            continue;
        }
        if !told.is_empty() {
            beats.push(Beat::Told(std::mem::take(&mut told)));
        }
        match line {
            Line::Heading(text) => beats.push(Beat::Heading(text.clone())),
            Line::Said { who, text, .. } => beats.push(Beat::Said {
                who: who.clone(),
                text: text.clone(),
            }),
            _ => {}
        }
    }
    if !told.is_empty() {
        beats.push(Beat::Told(told));
    }
    beats
}

/// How the game was won, as the narrator's closing `lines` tell it: the
/// first told line after the last blank one.
fn how(lines: &[Line]) -> String {
    let last = lines.iter().rposition(|line| *line == Line::Blank);
    lines[last.map_or(0, |blank| blank + 1)..]
        .iter()
        .find_map(|line| match line {
            Line::Told(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

impl Staged for Avalon {
    const GAME: GameName = GameName::Avalon;
    type Board = avalon::board::Board;

    fn role(role: avalon::Role) -> Role {
        match role {
            avalon::Role::Merlin => Role::Merlin,
            avalon::Role::Percival => Role::Percival,
            avalon::Role::Servant => Role::Servant,
            avalon::Role::Assassin => Role::Assassin,
            avalon::Role::Morgana => Role::Morgana,
            avalon::Role::Mordred => Role::Mordred,
            avalon::Role::Oberon => Role::Oberon,
            avalon::Role::Minion => Role::Minion,
        }
    }

    fn side(role: avalon::Role) -> Side {
        match role.team() {
            avalon::Team::Good => Side::Good,
            avalon::Team::Evil => Side::Evil,
        }
    }

    fn winner(event: &avalon::Outcome) -> Option<Side> {
        match event {
            avalon::Outcome::End { winner, .. } => Some(match winner {
                avalon::Team::Good => Side::Good,
                avalon::Team::Evil => Side::Evil,
            }),
            _ => None,
        }
    }
}

impl Staged for Werewolf {
    const GAME: GameName = GameName::Werewolf;
    type Board = ();

    fn role(role: werewolf::Role) -> Role {
        match role {
            werewolf::Role::Werewolf => Role::Werewolf,
            werewolf::Role::Villager => Role::Villager,
            werewolf::Role::Doctor => Role::Doctor,
            werewolf::Role::Seer => Role::Seer,
        }
    }

    fn side(role: werewolf::Role) -> Side {
        team(role.team())
    }

    fn winner(event: &werewolf::Outcome) -> Option<Side> {
        match event {
            werewolf::Outcome::End { winner, .. } => Some(team(*winner)),
            _ => None,
        }
    }
}

/// The side a Werewolf team is.
fn team(team: werewolf::Team) -> Side {
    match team {
        werewolf::Team::Werewolves => Side::Evil,
        werewolf::Team::Villagers => Side::Good,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::tests::Fixture;
    use crate::game::{Message, game};
    use crate::random;
    use crate::strategy::RANDOM;
    use social_deduction_stage::AvalonPanel;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio::sync::oneshot;

    /// Every event `log` holds once the episode that writes it is over.
    async fn drained<L>(mut log: UnboundedReceiver<Event<L>>) -> Vec<Event<L>> {
        let mut events = Vec::new();
        while let Some(event) = log.recv().await {
            events.push(event);
        }
        events
    }

    /// The log of a game of `roles` under `rules`, every player random.
    async fn random_game<G: Fixture>(
        roles: HashMap<PlayerId, G::Role>,
        rules: G::Rules,
    ) -> Vec<Event<Entry<G>>> {
        let (winner, _won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let configuration = G::configured(&roles, &rules);
        let episode = game(
            crate::game::tests::dealt(&roles, RANDOM),
            rules,
            configuration,
            winner,
            |_| random::Player::init(),
            logger,
        );
        episode.run(Duration::from_secs(60 * 60)).await.unwrap();
        drained(log).await
    }

    /// The cues a fresh stage folds `events` into.
    fn folded<G: Staged>(events: &[Event<Entry<G>>]) -> Vec<Cue> {
        let mut stage = Stage::<G>::default();
        events.iter().flat_map(|event| stage.fold(event)).collect()
    }

    /// The words cues show in the chat, in order: each heading, told line
    /// and utterance.
    fn shown(cues: &[Cue]) -> Vec<String> {
        cues.iter()
            .flat_map(|cue| match &cue.beat {
                Beat::Heading(text) | Beat::Said { text, .. } => vec![text.clone()],
                Beat::Told(lines) => lines.clone(),
                _ => vec![],
            })
            .collect()
    }

    /// What the console's narrator tells of `events` after the deal, as
    /// words the chat shows, and its vote lines.
    fn narrated<G: Staged>(events: &[Event<Entry<G>>]) -> (Vec<String>, Vec<String>) {
        let mut narrator = G::Narrator::default();
        let lines: Vec<Line> = events
            .iter()
            .flat_map(|event| {
                let lines = narrator.narrate(event);
                match event.payload {
                    Entry::Start { .. } => vec![],
                    _ => lines,
                }
            })
            .collect();
        let words = lines
            .iter()
            .filter_map(|line| match line {
                Line::Heading(text) | Line::Told(text) | Line::Said { text, .. } => {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect();
        let votes = lines
            .iter()
            .filter_map(|line| match line {
                Line::Vote(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        (words, votes)
    }

    /// Check the cues of the random game `events` of `G`, dealt `roles`,
    /// won by `side`: the cast, then the clock, then the narrator's words
    /// with no vote among them, and the outcome last.
    fn check_random_game<G: Staged>(
        events: &[Event<Entry<G>>],
        roles: &HashMap<PlayerId, G::Role>,
        side: Side,
    ) {
        let cues = folded(events);
        let Beat::Cast { game, seats } = &cues[0].beat else {
            panic!("{:?}", cues[0]);
        };
        assert_eq!(*game, G::GAME);
        let names: Vec<&str> = seats.iter().map(|seat| seat.name.as_str()).collect();
        assert_eq!(names, in_order(roles.keys()));
        for seat in seats {
            let role = roles[&seat.name];
            assert_eq!(seat.role, G::role(role));
            assert_eq!(seat.side, G::side(role));
            assert_eq!(seat.strategy, RANDOM);
            let label = format!("{} · {}", seat.name, capitalized(&role.to_string()));
            assert_eq!(seat.label, label);
        }
        assert_eq!(cues[1].beat, Beat::Clock { rate: 1.0 });
        let (words, votes) = narrated(events);
        assert_eq!(shown(&cues), words);
        assert!(!votes.is_empty());
        assert!(shown(&cues).iter().all(|word| !votes.contains(word)));
        let Beat::Over { outcome, log } = &cues.last().unwrap().beat else {
            panic!("{:?}", cues.last());
        };
        let Outcome::Won { side: won, how } = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(*won, side);
        assert!(how.contains(" win"), "{how}");
        assert_eq!(*log, None);
        let overs = cues
            .iter()
            .filter(|cue| matches!(cue.beat, Beat::Over { .. }));
        assert_eq!(overs.count(), 1);
    }

    #[tokio::test]
    async fn a_random_game_of_avalon_is_its_cast_its_narration_and_its_outcome() {
        let roles = avalon::tests::seated(&[
            avalon::Role::Merlin,
            avalon::Role::Percival,
            avalon::Role::Servant,
            avalon::Role::Assassin,
            avalon::Role::Morgana,
            avalon::Role::Mordred,
            avalon::Role::Oberon,
        ]);
        let events = random_game::<Avalon>(roles.clone(), avalon::Rules::default()).await;
        let logged: Vec<_> = events.iter().map(|event| event.payload.clone()).collect();
        let (winner, _) = avalon::tests::ending(&logged);
        let side = match winner {
            avalon::Team::Good => Side::Good,
            avalon::Team::Evil => Side::Evil,
        };
        check_random_game(&events, &roles, side);
    }

    #[tokio::test]
    async fn a_random_game_of_werewolf_is_its_cast_its_narration_and_its_outcome() {
        let roles: HashMap<PlayerId, werewolf::Role> = [
            werewolf::Role::Werewolf,
            werewolf::Role::Villager,
            werewolf::Role::Doctor,
            werewolf::Role::Werewolf,
            werewolf::Role::Seer,
            werewolf::Role::Villager,
        ]
        .into_iter()
        .enumerate()
        .map(|(seat, role)| (format!("Player {}", seat + 1), role))
        .collect();
        let events = random_game::<Werewolf>(roles.clone(), werewolf::Rules::default()).await;
        let winner = events
            .iter()
            .find_map(|event| match &event.payload {
                Entry::Event(werewolf::Outcome::End { winner, .. }) => Some(*winner),
                _ => None,
            })
            .unwrap();
        check_random_game(&events, &roles, team(winner));
        let cues = folded(&events);
        assert!(!cues.iter().any(|cue| matches!(cue.beat, Beat::Panel(_))));
    }

    #[tokio::test]
    async fn the_last_avalon_board_agrees_with_the_narrator() {
        let roles = avalon::tests::five();
        let events = random_game::<Avalon>(roles, avalon::Rules::default()).await;
        let cues = folded(&events);
        let boards: Vec<&AvalonPanel> = cues
            .iter()
            .filter_map(|cue| match &cue.beat {
                Beat::Panel(Panel::Avalon(board)) => Some(board),
                _ => None,
            })
            .collect();
        let last = boards.last().unwrap();
        let logged: Vec<_> = events.iter().map(|event| event.payload.clone()).collect();
        let quested: Vec<(usize, bool)> = logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Event(avalon::Outcome::Quested { result, .. }) => {
                    Some((result.sabotages, result.succeeded))
                }
                _ => None,
            })
            .collect();
        let tokens: Vec<(usize, bool)> = last
            .quests
            .iter()
            .filter_map(|token| token.result.as_ref())
            .map(|result| (result.sabotages, result.succeeded))
            .collect();
        assert_eq!(tokens, quested);
        let tallied: Vec<bool> = logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Event(avalon::Outcome::Tallied { tally, .. }) => Some(tally.approved),
                _ => None,
            })
            .collect();
        let rows: Vec<bool> = last
            .proposals
            .iter()
            .rev()
            .filter_map(|row| row.approved)
            .collect();
        assert_eq!(rows, tallied);
        let proposed = logged
            .iter()
            .filter(|entry| matches!(entry, Entry::Event(avalon::Outcome::Proposed { .. })))
            .count();
        assert_eq!(last.proposals.len(), proposed);
    }

    /// `events` written as a log is, in JSON Lines.
    fn written<L: serde::Serialize>(events: &[Event<L>]) -> String {
        let mut written = Vec::new();
        for event in events {
            event.write(&mut written).unwrap();
        }
        String::from_utf8(written).unwrap()
    }

    #[tokio::test]
    async fn a_game_folded_live_and_one_read_back_from_its_log_agree() {
        let events = random_game::<Avalon>(avalon::tests::five(), avalon::Rules::default()).await;
        assert_eq!(cues(&written(&events)).unwrap(), folded(&events));
        let roles = werewolf::tests::one_wolf_against(&["Player 1", "Player 2", "Player 3"]);
        let events = random_game::<Werewolf>(roles, werewolf::Rules::default()).await;
        let log = format!("{}\n", written(&events));
        assert_eq!(cues(&log).unwrap(), folded(&events));
    }

    #[test]
    fn a_log_of_neither_game_is_an_error() {
        let error = cues("{}").unwrap_err();
        assert_eq!(
            error.to_string(),
            "the log is of neither Avalon nor Werewolf"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn each_utterance_in_a_prompted_game_is_said_by_its_speaker() {
        let events = avalon::prompted::tests::prompted_game().await;
        let said: Vec<Beat> = events
            .iter()
            .filter_map(|event| match &event.payload {
                Entry::Action {
                    message: Message::Say { from, text, .. },
                    ..
                } => Some(Beat::Said {
                    who: from.clone(),
                    text: text.clone(),
                }),
                _ => None,
            })
            .collect();
        assert!(!said.is_empty());
        let cues = folded(&events);
        let bubbles: Vec<Beat> = cues
            .into_iter()
            .filter(|cue| matches!(cue.beat, Beat::Said { .. }))
            .map(|cue| cue.beat)
            .collect();
        assert_eq!(bubbles, said);
    }

    #[test]
    fn a_cue_is_stamped_with_its_events_time_in_milliseconds() {
        assert_eq!(millis(UNIX_EPOCH + Duration::from_millis(1_234)), 1_234);
        assert_eq!(millis(UNIX_EPOCH - Duration::from_secs(1)), 0);
    }

    #[test]
    fn told_lines_run_together_until_something_else_comes_between() {
        let said = Line::Said {
            who: "Player 1".to_string(),
            speaker: "Player 1 (Merlin)".to_string(),
            text: "Hello.".to_string(),
        };
        let lines = [
            Line::Told("a".to_string()),
            Line::Told("b".to_string()),
            Line::Blank,
            Line::Heading("Day 1.".to_string()),
            Line::Vote("Player 1 votes.".to_string()),
            Line::Told("c".to_string()),
            said,
            Line::Told("d".to_string()),
        ];
        assert_eq!(
            beats(&lines),
            [
                Beat::Told(vec!["a".to_string(), "b".to_string()]),
                Beat::Heading("Day 1.".to_string()),
                Beat::Told(vec!["c".to_string()]),
                Beat::Said {
                    who: "Player 1".to_string(),
                    text: "Hello.".to_string()
                },
                Beat::Told(vec!["d".to_string()]),
            ]
        );
    }

    #[test]
    fn the_win_is_the_first_told_line_after_the_last_blank_one() {
        let told = |text: &str| Line::Told(text.to_string());
        let lines = [
            told("The quest took 3s."),
            Line::Blank,
            told("Good wins with three successful quests."),
            told("The game took 9s."),
        ];
        assert_eq!(how(&lines), "Good wins with three successful quests.");
        assert_eq!(how(&[told("Evil wins.")]), "Evil wins.");
        assert_eq!(how(&[]), "");
    }

    #[test]
    fn a_role_is_capitalized_in_a_label() {
        assert_eq!(capitalized("seer"), "Seer");
        assert_eq!(capitalized("Merlin"), "Merlin");
        assert_eq!(capitalized(""), "");
    }
}
