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
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What a game supplies for its log to be folded into cues: its name, how
/// its roles and sides are drawn, how it is won, and its board.
pub trait Staged: Game {
    /// The game's name, which picks the board the viewer draws.
    const GAME: GameName;
    /// The board this game draws.
    type Board: Board<Self>;

    /// `role` as the viewer colors it.
    fn role(role: Self::Role) -> Role;

    /// The side that won and how, as the narrator tells it, if `event`
    /// ends the game.
    fn winner(event: &Self::Event) -> Option<(Side, String)>;

    /// The player who dies by `event`, if anyone does.
    fn died(event: &Self::Event) -> Option<&PlayerId>;
}

/// A game's board, moved by the log's events.
pub trait Board<G: Game>: Default + Send + 'static {
    /// The whole board after `event`, when `event` changes it.
    fn fold(&mut self, event: &Event<Entry<G>>) -> Option<Panel>;
}

/// Folds one game's log into cues, an event at a time.
pub struct Stage<G: Staged> {
    /// The console's narrator, whose lines the cues carry.
    narrator: G::Narrator,
    /// The game's board.
    board: G::Board,
    /// The players whose model call is out.
    thinking: HashSet<PlayerId>,
}

impl<G: Staged> Default for Stage<G> {
    fn default() -> Self {
        Self {
            narrator: G::Narrator::default(),
            board: G::Board::default(),
            thinking: HashSet::new(),
        }
    }
}

impl<G: Staged> Stage<G> {
    /// The cues `event` adds, each stamped with the event's time. The deal
    /// is the cast, then the clock at its live rate. Every other event is
    /// what its narrator lines tell: each heading a heading, each run of
    /// told lines one block, each utterance a bubble from its speaker,
    /// votes and blank lines left out. A model call going out is its
    /// player thinking, and coming back their thought; a withheld choice
    /// is nothing. The board follows when the event moves it. A player
    /// who dies with a call out has thought, and so has everyone still
    /// thinking when the game ends. The end of the game is its outcome, last, won as the
    /// narrator tells it.
    pub fn fold(&mut self, event: &Event<Entry<G>>) -> Vec<Cue> {
        let at = millis(event.time);
        let lines = self.narrator.narrate(event);
        let mut beats = match &event.payload {
            Entry::Start {
                roles, strategies, ..
            } => vec![cast::<G>(roles, strategies), Beat::Clock { rate: 1.0 }],
            Entry::Thinking { player, .. } => {
                self.thinking.insert(player.clone());
                vec![Beat::Thinking {
                    who: player.clone(),
                }]
            }
            Entry::Thought { player, .. } => self.thought(player).into_iter().collect(),
            _ => beats(&lines),
        };
        beats.extend(self.board.fold(event).map(Beat::Panel));
        if let Entry::Event(own) = &event.payload {
            beats.extend(G::died(own).and_then(|dead| self.thought(dead)));
            if let Some((side, how)) = G::winner(own) {
                beats.extend(self.hushed());
                let outcome = Outcome::Won { side, how };
                beats.push(Beat::Over { outcome });
            }
        }
        beats.into_iter().map(|beat| Cue { at, beat }).collect()
    }

    /// The thoughts of every player still thinking, in seat order, stamped
    /// `at`: what closes every call still out when a game stops or fails,
    /// before its outcome.
    pub fn close(&mut self, at: Millis) -> Vec<Cue> {
        self.hushed()
            .into_iter()
            .map(|beat| Cue { at, beat })
            .collect()
    }

    /// `player`'s thought, if their call is out.
    fn thought(&mut self, player: &PlayerId) -> Option<Beat> {
        self.thinking.remove(player).then(|| Beat::Thought {
            who: player.clone(),
        })
    }

    /// The thoughts of every player still thinking, in seat order.
    fn hushed(&mut self) -> Vec<Beat> {
        let thinking = std::mem::take(&mut self.thinking);
        in_order(&thinking)
            .into_iter()
            .map(|who| Beat::Thought { who: who.clone() })
            .collect()
    }
}

/// The cues a game's log folds into, the log written as JSON Lines by a
/// game of either kind, which its first line says. Calls still out at the
/// end of the log are closed there.
///
/// # Errors
///
/// A first line that is an event of neither game, or a later line that
/// is no event of the first line's game, named by its number.
pub fn cues(log: &str) -> anyhow::Result<Vec<Cue>> {
    let Some(first) = log.lines().find(|line| !line.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    if serde_json::from_str::<Event<Entry<Avalon>>>(first).is_ok() {
        read::<Avalon>(log, "Avalon")
    } else if serde_json::from_str::<Event<Entry<Werewolf>>>(first).is_ok() {
        read::<Werewolf>(log, "Werewolf")
    } else {
        anyhow::bail!("the log is of neither Avalon nor Werewolf")
    }
}

/// The cues the log of a game of `G`, named `game`, folds into.
fn read<G: Staged>(log: &str, game: &str) -> anyhow::Result<Vec<Cue>> {
    let mut stage = Stage::<G>::default();
    let mut cues = Vec::new();
    for (index, line) in log.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: Event<Entry<G>> = serde_json::from_str(line)
            .with_context(|| format!("the {game} log is bad at line {}", index + 1))?;
        cues.extend(stage.fold(&event));
    }
    let at = cues.last().map_or(0, |cue| cue.at);
    cues.extend(stage.close(at));
    Ok(cues)
}

/// Set `player`'s entry in `entries` to `value`.
pub(crate) fn set<T>(entries: &mut [(PlayerId, T)], player: &PlayerId, value: T) {
    if let Some((_, entry)) = entries.iter_mut().find(|(name, _)| name == player) {
        *entry = value;
    }
}

/// `time` in milliseconds since the Unix epoch.
pub(crate) fn millis(time: SystemTime) -> Millis {
    millis_of(time.duration_since(UNIX_EPOCH).unwrap_or_default())
}

/// `duration` in whole milliseconds.
pub(crate) fn millis_of(duration: Duration) -> Millis {
    Millis::try_from(duration.as_millis()).unwrap_or(Millis::MAX)
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

    fn winner(event: &avalon::Outcome) -> Option<(Side, String)> {
        match event {
            avalon::Outcome::End { winner, by, .. } => {
                Some((side(*winner), avalon::report::won(*by).to_string()))
            }
            _ => None,
        }
    }

    /// Nobody dies in Avalon.
    fn died(_: &avalon::Outcome) -> Option<&PlayerId> {
        None
    }
}

/// The side an Avalon team is.
fn side(team: avalon::Team) -> Side {
    match team {
        avalon::Team::Good => Side::Good,
        avalon::Team::Evil => Side::Evil,
    }
}

impl Staged for Werewolf {
    const GAME: GameName = GameName::Werewolf;
    type Board = werewolf::board::Board;

    fn role(role: werewolf::Role) -> Role {
        match role {
            werewolf::Role::Werewolf => Role::Werewolf,
            werewolf::Role::Villager => Role::Villager,
            werewolf::Role::Doctor => Role::Doctor,
            werewolf::Role::Seer => Role::Seer,
        }
    }

    fn winner(event: &werewolf::Outcome) -> Option<(Side, String)> {
        match event {
            werewolf::Outcome::End { winner, days, .. } => {
                Some((team(*winner), werewolf::report::won(*winner, *days)))
            }
            _ => None,
        }
    }

    fn died(event: &werewolf::Outcome) -> Option<&PlayerId> {
        match event {
            werewolf::Outcome::Hunted { hunt, .. } => hunt.dead(),
            werewolf::Outcome::Voted { out, .. } => out.as_ref(),
            werewolf::Outcome::End { .. } => None,
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
    use crate::game::Message;
    use crate::game::tests::{Fixture, drained, episode, logged, seated};
    use crate::strategy::RANDOM;
    use social_deduction_stage::{AvalonPanel, WerewolfPanel};

    /// The log of a game of `roles` under `rules`, every player random.
    async fn random_game<G: Fixture>(
        roles: HashMap<PlayerId, G::Role>,
        rules: G::Rules,
    ) -> Vec<Event<Entry<G>>> {
        let (episode, _, log) = episode(roles, rules, HashMap::new());
        episode.run(Duration::from_secs(60 * 60)).await.unwrap();
        drained(log).await
    }

    #[test]
    fn the_viewer_puts_every_role_on_the_side_it_wins_with() {
        for role in [
            avalon::Role::Merlin,
            avalon::Role::Percival,
            avalon::Role::Servant,
            avalon::Role::Assassin,
            avalon::Role::Morgana,
            avalon::Role::Mordred,
            avalon::Role::Oberon,
            avalon::Role::Minion,
        ] {
            assert_eq!(Avalon::role(role).side(), side(role.team()), "{role:?}");
        }
        for role in [
            werewolf::Role::Werewolf,
            werewolf::Role::Villager,
            werewolf::Role::Doctor,
            werewolf::Role::Seer,
        ] {
            assert_eq!(Werewolf::role(role).side(), team(role.team()), "{role:?}");
        }
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
        let seats = in_order(roles.keys())
            .into_iter()
            .map(|name| {
                let role = roles[name];
                Seat {
                    name: name.clone(),
                    label: format!("{name} · {}", capitalized(&role.to_string())),
                    role: G::role(role),
                    strategy: RANDOM.to_string(),
                }
            })
            .collect();
        let game = G::GAME;
        assert_eq!(cues[0].beat, Beat::Cast { game, seats });
        assert_eq!(cues[1].beat, Beat::Clock { rate: 1.0 });
        let (words, votes) = narrated(events);
        assert_eq!(shown(&cues), words);
        assert!(!votes.is_empty());
        assert!(shown(&cues).iter().all(|word| !votes.contains(word)));
        let (_, how) = events
            .iter()
            .find_map(|event| match &event.payload {
                Entry::Event(own) => G::winner(own),
                _ => None,
            })
            .unwrap();
        assert!(words.contains(&how), "{how}");
        let outcome = Outcome::Won { side, how };
        let over = Beat::Over { outcome };
        assert_eq!(cues.last().unwrap().beat, over);
        let overs = cues
            .iter()
            .filter(|cue| matches!(cue.beat, Beat::Over { .. }));
        assert_eq!(overs.count(), 1);
    }

    #[tokio::test]
    async fn a_random_game_of_avalon_is_its_cast_its_narration_and_its_outcome() {
        let roles = seated(&[
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
        check_random_game(&events, &roles, side(winner));
    }

    #[tokio::test]
    async fn a_random_game_of_werewolf_is_its_cast_its_narration_and_its_outcome() {
        let roles = seated(&[
            werewolf::Role::Werewolf,
            werewolf::Role::Villager,
            werewolf::Role::Doctor,
            werewolf::Role::Werewolf,
            werewolf::Role::Seer,
            werewolf::Role::Villager,
        ]);
        let events = random_game::<Werewolf>(roles.clone(), werewolf::Rules::default()).await;
        let winner = events
            .iter()
            .find_map(|event| match &event.payload {
                Entry::Event(werewolf::Outcome::End { winner, .. }) => Some(*winner),
                _ => None,
            })
            .unwrap();
        check_random_game(&events, &roles, team(winner));
    }

    #[tokio::test]
    async fn each_werewolf_board_is_a_change_and_the_last_ones_dead_are_the_narrators() {
        let roles = seated(&[
            werewolf::Role::Werewolf,
            werewolf::Role::Villager,
            werewolf::Role::Doctor,
            werewolf::Role::Werewolf,
            werewolf::Role::Seer,
            werewolf::Role::Villager,
        ]);
        let events = random_game::<Werewolf>(roles.clone(), werewolf::Rules::default()).await;
        let cues = folded(&events);
        let boards: Vec<&WerewolfPanel> = cues
            .iter()
            .filter_map(|cue| match &cue.beat {
                Beat::Panel(Panel::Werewolf(board)) => Some(board),
                _ => None,
            })
            .collect();
        assert!(boards.len() > 1);
        assert!(boards.windows(2).all(|pair| pair[0] != pair[1]));
        let survivors = events
            .iter()
            .find_map(|event| match &event.payload {
                Entry::Event(werewolf::Outcome::End { survivors, .. }) => Some(survivors),
                _ => None,
            })
            .unwrap();
        let dead: Vec<&PlayerId> = in_order(roles.keys())
            .into_iter()
            .filter(|player| !survivors.contains(*player))
            .collect();
        let last = boards.last().unwrap();
        let shown_dead: Vec<&PlayerId> = last
            .seats
            .iter()
            .filter(|seat| seat.died.is_some())
            .map(|seat| &seat.name)
            .collect();
        assert_eq!(shown_dead, dead);
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

    #[test]
    fn an_empty_log_folds_into_no_cues() {
        assert_eq!(cues("\n").unwrap(), []);
    }

    #[tokio::test]
    async fn a_bad_line_in_a_log_is_an_error_of_its_game_naming_the_line() {
        let events = random_game::<Avalon>(avalon::tests::five(), avalon::Rules::default()).await;
        let log = written(&events);
        let lines = log.lines().count();
        let truncated = &log[..log.len() - 10];
        let error = format!("{:#}", cues(truncated).unwrap_err());
        assert!(
            error.starts_with(&format!("the Avalon log is bad at line {lines}: ")),
            "{error}"
        );
        let roles = werewolf::tests::one_wolf_against(&["Player 1", "Player 2", "Player 3"]);
        let events = random_game::<Werewolf>(roles, werewolf::Rules::default()).await;
        let log = format!("{}\n{{}}\n", written(&events));
        let error = format!("{:#}", cues(&log).unwrap_err());
        // The blank line counts as a line.
        let bad = events.len() + 2;
        assert!(
            error.starts_with(&format!("the Werewolf log is bad at line {bad}: ")),
            "{error}"
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

    /// `entries`, logged in order, each event's beats apart.
    fn staged<G: Staged>(entries: &[Entry<G>]) -> Vec<Vec<Beat>> {
        let mut stage = Stage::<G>::default();
        entries
            .iter()
            .map(|entry| {
                let event = logged(entry.clone());
                stage.fold(&event).into_iter().map(|cue| cue.beat).collect()
            })
            .collect()
    }

    /// Check the calls `beats` show: every `Thinking` is closed by one
    /// `Thought` of its player before `Over`, and each utterance comes
    /// from its speaker's call that came back, after that call's `Thought`
    /// and before the speaker's next `Thinking`.
    fn check_calls(beats: &[Beat]) {
        let mut thinking = HashSet::new();
        let mut answered = HashSet::new();
        let mut calls = 0;
        for beat in beats {
            match beat {
                Beat::Thinking { who } => {
                    assert!(thinking.insert(who), "{who} thinks twice");
                    answered.remove(who);
                    calls += 1;
                }
                Beat::Thought { who } => {
                    assert!(thinking.remove(who), "{who} thought unasked");
                    answered.insert(who);
                }
                Beat::Said { who, .. } => assert!(answered.contains(who), "{who} spoke unasked"),
                Beat::Over { .. } => assert!(thinking.is_empty(), "{thinking:?}"),
                _ => {}
            }
        }
        assert!(calls > 0);
        assert!(beats.iter().any(|beat| matches!(beat, Beat::Over { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn each_call_in_a_prompted_game_of_avalon_is_closed_and_speaks_after_it_comes_back() {
        let events = avalon::prompted::tests::prompted_game().await;
        check_calls(
            &folded(&events)
                .into_iter()
                .map(|cue| cue.beat)
                .collect::<Vec<_>>(),
        );
    }

    /// `message`, withheld by `Player 1`.
    fn withheld<G: Game>(message: Message<G>) -> Entry<G> {
        Entry::Withheld {
            from: "Player 1".to_string(),
            message,
            seen: Some(3),
            forgot: None,
        }
    }

    #[test]
    fn a_withheld_choice_gives_no_cue_in_either_game() {
        let voted = withheld::<Avalon>(Message::Move(avalon::Move::Vote {
            seq: 1,
            from: "Player 1".to_string(),
            n: 0,
            ballot: avalon::Ballot::Approve,
        }));
        assert_eq!(staged(&[voted]), [vec![]]);
        let selected = withheld::<Werewolf>(Message::Move(werewolf::Move::Select {
            seq: 1,
            from: "Player 1".to_string(),
            n: 0,
            target: "Player 2".to_string(),
        }));
        assert_eq!(staged(&[selected]), [vec![]]);
    }

    #[test]
    fn a_thought_with_no_call_out_shows_nothing() {
        let thinking = Entry::<Avalon>::Thinking {
            player: "Player 1".to_string(),
            seq: 1,
        };
        let thought = Entry::<Avalon>::Thought {
            player: "Player 1".to_string(),
            seq: 1,
            ending: crate::game::Ending::Answered,
        };
        let who = "Player 1".to_string();
        assert_eq!(
            staged(&[thinking, thought.clone(), thought]),
            [
                vec![Beat::Thinking { who: who.clone() }],
                vec![Beat::Thought { who }],
                vec![]
            ]
        );
    }

    #[test]
    fn a_player_who_dies_with_a_call_out_has_thought() {
        let thinking = Entry::<Werewolf>::Thinking {
            player: "bob".to_string(),
            seq: 2,
        };
        let hunted = Entry::<Werewolf>::Event(werewolf::Outcome::Hunted {
            seq: 3,
            hunt: werewolf::Hunt {
                victim: "bob".to_string(),
                protectors: HashSet::new(),
            },
        });
        let staged = staged(&[thinking, hunted]);
        let thought = Beat::Thought {
            who: "bob".to_string(),
        };
        assert_eq!(staged[1].last(), Some(&thought));
    }

    #[test]
    fn calls_still_out_at_the_end_of_a_log_are_closed_there_in_seat_order() {
        let thinking = |player: &str| {
            logged(Entry::<Avalon>::Thinking {
                player: player.to_string(),
                seq: 1,
            })
        };
        let log = written(&[thinking("Player 10"), thinking("Player 2")]);
        let beats: Vec<Beat> = cues(&log)
            .unwrap()
            .into_iter()
            .map(|cue| cue.beat)
            .collect();
        let who = |name: &str| name.to_string();
        assert_eq!(
            beats,
            [
                Beat::Thinking {
                    who: who("Player 10")
                },
                Beat::Thinking {
                    who: who("Player 2")
                },
                Beat::Thought {
                    who: who("Player 2")
                },
                Beat::Thought {
                    who: who("Player 10")
                },
            ]
        );
        let mut stage = Stage::<Avalon>::default();
        assert!(stage.close(5).is_empty());
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
    fn a_role_is_capitalized_in_a_label() {
        assert_eq!(capitalized("seer"), "Seer");
        assert_eq!(capitalized("Merlin"), "Merlin");
        assert_eq!(capitalized(""), "");
    }
}
