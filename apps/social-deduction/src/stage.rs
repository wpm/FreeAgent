//! What a viewer is drawn from: a game's log, folded an event at a time
//! into [`Cue`]s.
//!
//! A [`Stage`] reads the same events the console's narrator does and
//! keeps the narrator's words. The deal becomes the cast, each narrator
//! line a heading, a block of narration or an utterance, and the end of
//! the game its outcome. Votes and selections stay out of the cues: the
//! game's [`Board`] shows them, whole, after every event that moves it.

use crate::avalon::{self, Avalon};
use crate::config::Name;
use crate::game::{Entry, Game, Narrate};
use crate::log::{Ended, Header};
use crate::report::{Line, in_order};
use crate::seat::PlayerId;
use crate::werewolf::{self, Werewolf};
use anyhow::Context;
use free_agent::Event;
use serde::de::IgnoredAny;
use social_deduction_stage::{Beat, Cue, GameName, Millis, Outcome, Panel, Role, Seat, Side};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

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

    /// The last cues of a game that `ended` as it did at `at`, `won` being
    /// the outcome [`Stage::fold`] gave if a side won: a thought for every
    /// call still out, in seat order, then the outcome. A failure tells its
    /// error first and is a failure even after a win. Otherwise a game won
    /// is won, and one with no end is [`Outcome::Unfinished`].
    pub fn end(&mut self, at: Millis, won: Option<Cue>, ended: Option<Ended>) -> Vec<Cue> {
        let mut beats = self.hushed();
        let over = |outcome| Cue {
            at,
            beat: Beat::Over { outcome },
        };
        let last = match ended {
            Some(Ended::Failed(error)) => {
                beats.push(Beat::Told(vec![error.clone()]));
                over(Outcome::Failed(error))
            }
            Some(_) => won.unwrap_or_else(|| over(Outcome::Stopped)),
            None => won.unwrap_or_else(|| over(Outcome::Unfinished)),
        };
        let mut cues: Vec<Cue> = beats.into_iter().map(|beat| Cue { at, beat }).collect();
        cues.push(last);
        cues
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

/// A log's episodes, each read as its own game.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recordings {
    /// Every episode that reads, in the order of each one's first line.
    pub games: Vec<Recording>,
    /// Why each episode left out was left out, naming the line.
    pub problems: Vec<String>,
}

/// One episode of a log, folded into cues.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    /// The episode's ID, which every one of its lines carries.
    pub episode: Uuid,
    /// The game it plays.
    pub game: GameName,
    /// Its cues, the last of them its one [`Beat::Over`].
    pub cues: Vec<Cue>,
}

/// The episodes of a log written as JSON Lines by games of either kind,
/// each told apart by its ID, wherever its lines fall. Each episode is of
/// the game its header names. An episode with no header, written before
/// logs had one, is of the game whose event its first line is.
///
/// Each recording ends with one [`Beat::Over`], after a thought for every
/// call still out: as its episode ended, with the error told first when it
/// failed. An episode won and no more is won, and one cut off before that
/// is [`Outcome::Unfinished`].
///
/// A line that is no event at all belongs to the episode of the line
/// before it. An episode with a bad line is left out, and the problem
/// names the line by its number.
///
/// # Errors
///
/// Every problem, when no episode reads.
pub fn recordings(log: &str) -> anyhow::Result<Recordings> {
    let mut episodes: Vec<(Uuid, Vec<(usize, &str)>)> = Vec::new();
    let mut found = HashMap::new();
    let mut problems = Vec::new();
    let mut current = None;
    for (index, line) in log.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let number = index + 1;
        let episode = match (serde_json::from_str::<Event<IgnoredAny>>(line), current) {
            (Ok(event), _) => event.episode,
            (Err(_), Some(episode)) => episode,
            (Err(error), None) => {
                problems.push(format!("the log is bad at line {number}: {error}"));
                continue;
            }
        };
        current = Some(episode);
        let at = *found.entry(episode).or_insert_with(|| {
            episodes.push((episode, Vec::new()));
            episodes.len() - 1
        });
        episodes[at].1.push((number, line));
    }
    let mut games = Vec::new();
    for (episode, lines) in episodes {
        match recording(episode, &lines) {
            Ok(recording) => games.push(recording),
            Err(error) => problems.push(format!("{error:#}")),
        }
    }
    anyhow::ensure!(
        !games.is_empty() || problems.is_empty(),
        "{}",
        problems.join("\n")
    );
    Ok(Recordings { games, problems })
}

/// The episode `episode`, its `lines` numbered, read as the game its
/// first line names or is an event of.
fn recording(episode: Uuid, lines: &[(usize, &str)]) -> anyhow::Result<Recording> {
    let (number, first) = lines[0];
    let header = Header::read(first)
        .with_context(|| format!("episode {episode} is bad at line {number}"))?;
    match header.map(|header| header.game) {
        Some(Name::Avalon) => record::<Avalon>(episode, lines),
        Some(Name::Werewolf) => record::<Werewolf>(episode, lines),
        // An episode of format 0 has no header, so its game is the one
        // whose event its first line is.
        None if serde_json::from_str::<Event<Entry<Avalon>>>(first).is_ok() => {
            record::<Avalon>(episode, lines)
        }
        None if serde_json::from_str::<Event<Entry<Werewolf>>>(first).is_ok() => {
            record::<Werewolf>(episode, lines)
        }
        None => {
            anyhow::bail!("episode {episode} is of neither Avalon nor Werewolf at line {number}")
        }
    }
}

/// The episode `episode` of a game of `G`, its `lines` numbered, folded
/// into cues and ended as [`recordings`] ends it.
fn record<G: Staged>(episode: Uuid, lines: &[(usize, &str)]) -> anyhow::Result<Recording> {
    let mut stage = Stage::<G>::default();
    let mut cues = Vec::new();
    let mut won = None;
    let mut ended = None;
    let mut at = 0;
    for &(number, line) in lines {
        let event: Event<Entry<G>> = serde_json::from_str(line).with_context(|| {
            format!("the {} episode {episode} is bad at line {number}", G::NAME)
        })?;
        at = millis(event.time);
        if let Entry::Ended(how) = &event.payload {
            ended = Some(how.clone());
        }
        for cue in stage.fold(&event) {
            match cue.beat {
                Beat::Over { .. } => won = Some(cue),
                _ => cues.push(cue),
            }
        }
    }
    cues.extend(stage.end(at, won, ended));
    Ok(Recording {
        episode,
        game: G::GAME,
        cues,
    })
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
    use crate::log::Writer;
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

    /// The log of a random game of Avalon for five.
    async fn random_avalon() -> Vec<Event<Entry<Avalon>>> {
        random_game(avalon::tests::five(), avalon::Rules::default()).await
    }

    /// The log of a random game of Werewolf, one wolf against two.
    async fn random_werewolf() -> Vec<Event<Entry<Werewolf>>> {
        let roles = werewolf::tests::one_wolf_against(&["Player 1", "Player 2", "Player 3"]);
        random_game(roles, werewolf::Rules::default()).await
    }

    /// A log of random games of Avalon as `episodes`, each won, and the
    /// cues each folds into.
    async fn avalon_log(episodes: impl IntoIterator<Item = u128>) -> (String, Vec<Vec<Cue>>) {
        let mut log = String::new();
        let mut each = Vec::new();
        for episode in episodes {
            let events = random_avalon().await;
            log += &recorded_as(episode, &events, Ended::Won);
            each.push(folded(&events));
        }
        (log, each)
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

    /// The cues of the one episode `log` holds.
    fn only(log: &str) -> Vec<Cue> {
        let Recordings {
            mut games,
            problems,
        } = recordings(log).unwrap();
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(games.len(), 1);
        games.remove(0).cues
    }

    #[tokio::test]
    async fn a_game_folded_live_and_one_read_back_from_its_log_agree() {
        let events = random_avalon().await;
        assert_eq!(only(&written(&events)), folded(&events));
        let events = random_werewolf().await;
        let log = format!("{}\n", written(&events));
        assert_eq!(only(&log), folded(&events));
    }

    /// `events` of `G` written as the log's writer writes them, as the
    /// episode `episode`, between its header and how it `ended`.
    fn recorded_as<G: Game>(episode: u128, events: &[Event<Entry<G>>], ended: Ended) -> String {
        let episode = Uuid::from_u128(episode);
        let mut writer = Writer::<G, _>::start(Vec::new(), episode).unwrap();
        for event in events {
            writer
                .write(&Event {
                    episode,
                    ..event.clone()
                })
                .unwrap();
        }
        String::from_utf8(writer.end(ended).unwrap()).unwrap()
    }

    /// `events` of `G` written as the log's writer writes them, between
    /// the episode's header and its win.
    fn recorded<G: Game>(events: &[Event<Entry<G>>]) -> String {
        recorded_as(0, events, Ended::Won)
    }

    #[tokio::test]
    async fn a_log_with_a_header_is_read_as_the_game_it_names() {
        let events = random_avalon().await;
        let read = recordings(&recorded(&events)).unwrap();
        assert_eq!(read.games[0].game, GameName::Avalon);
        assert_eq!(read.games[0].cues, folded(&events));
        let events = random_werewolf().await;
        let read = recordings(&recorded(&events)).unwrap();
        assert_eq!(read.games[0].game, GameName::Werewolf);
        assert_eq!(read.games[0].cues, folded(&events));
    }

    /// The episodes `read` holds, in order.
    fn episodes(read: &Recordings) -> Vec<u128> {
        read.games
            .iter()
            .map(|game| game.episode.as_u128())
            .collect()
    }

    /// Check that `cues` start with the cast and end with their one
    /// outcome, `outcome`.
    fn check_whole(cues: &[Cue], outcome: impl Fn(&Outcome) -> bool) {
        assert!(matches!(cues[0].beat, Beat::Cast { .. }), "{:?}", cues[0]);
        let overs: Vec<&Outcome> = cues
            .iter()
            .filter_map(|cue| match &cue.beat {
                Beat::Over { outcome } => Some(outcome),
                _ => None,
            })
            .collect();
        assert_eq!(overs.len(), 1);
        assert!(matches!(cues.last().unwrap().beat, Beat::Over { .. }));
        assert!(outcome(overs[0]), "{:?}", overs[0]);
    }

    /// Whether `outcome` is a win.
    fn won(outcome: &Outcome) -> bool {
        matches!(outcome, Outcome::Won { .. })
    }

    #[tokio::test]
    async fn a_log_of_three_episodes_of_one_game_is_three_recordings_in_order() {
        let mut log = String::new();
        let mut each = Vec::new();
        for episode in [3, 1, 2] {
            let events = random_avalon().await;
            log += &recorded_as(episode, &events, Ended::Won);
            each.push(folded(&events));
        }
        let read = recordings(&log).unwrap();
        assert_eq!(episodes(&read), [3, 1, 2]);
        assert!(read.problems.is_empty());
        for (game, cues) in read.games.iter().zip(each) {
            check_whole(&game.cues, won);
            assert_eq!(game.cues, cues);
        }
    }

    /// A random game of Avalon and one of Werewolf, as episodes 1 and 2.
    async fn one_of_each() -> (String, String) {
        let avalon = random_avalon().await;
        let werewolf = random_werewolf().await;
        (
            recorded_as(1, &avalon, Ended::Won),
            recorded_as(2, &werewolf, Ended::Won),
        )
    }

    #[tokio::test]
    async fn a_log_of_an_episode_of_each_game_reads_both() {
        let (avalon, werewolf) = one_of_each().await;
        let read = recordings(&(avalon + &werewolf)).unwrap();
        let games: Vec<GameName> = read.games.iter().map(|game| game.game).collect();
        assert_eq!(games, [GameName::Avalon, GameName::Werewolf]);
        for game in &read.games {
            check_whole(&game.cues, won);
        }
    }

    #[tokio::test]
    async fn episodes_whose_lines_are_interleaved_read_as_they_do_one_after_the_other() {
        let (avalon, werewolf) = one_of_each().await;
        let mut first = avalon.lines();
        let mut second = werewolf.lines();
        let mut interleaved = String::new();
        loop {
            let (one, other) = (first.next(), second.next());
            if one.is_none() && other.is_none() {
                break;
            }
            for line in one.into_iter().chain(other) {
                interleaved += line;
                interleaved += "\n";
            }
        }
        let read = recordings(&interleaved).unwrap();
        assert_eq!(episodes(&read), [1, 2]);
        assert_eq!(read, recordings(&(avalon + &werewolf)).unwrap());
    }

    #[tokio::test]
    async fn an_episode_cut_off_ends_unfinished_with_its_calls_closed() {
        let (avalon, werewolf) = one_of_each().await;
        let thinking = logged(Entry::<Werewolf>::Thinking {
            player: "Player 2".to_string(),
            seq: 1,
        });
        let cut: String = written(&[Event {
            episode: Uuid::from_u128(2),
            ..thinking
        }]);
        let half = werewolf.lines().count() / 2;
        let cut = werewolf
            .lines()
            .take(half)
            .map(|line| format!("{line}\n"))
            .collect::<String>()
            + &cut;
        let read = recordings(&(avalon + &cut)).unwrap();
        assert_eq!(episodes(&read), [1, 2]);
        check_whole(&read.games[0].cues, won);
        let cues = &read.games[1].cues;
        check_whole(cues, |outcome| *outcome == Outcome::Unfinished);
        let thought = Beat::Thought {
            who: "Player 2".to_string(),
        };
        assert_eq!(cues[cues.len() - 2].beat, thought);
    }

    /// The beats of the one episode `log` holds.
    fn beats_of(log: &str) -> Vec<Beat> {
        only(log).into_iter().map(|cue| cue.beat).collect()
    }

    #[test]
    fn an_episode_stopped_ends_stopped_and_one_failed_tells_its_error_and_ends_failed() {
        let thinking = logged(Entry::<Avalon>::Thinking {
            player: "Player 2".to_string(),
            seq: 1,
        });
        let who = "Player 2".to_string();
        let calls = vec![Beat::Thinking { who: who.clone() }, Beat::Thought { who }];
        let stopped = recorded_as(1, std::slice::from_ref(&thinking), Ended::Stopped);
        let over = |outcome| Beat::Over { outcome };
        assert_eq!(
            beats_of(&stopped),
            [calls.clone(), vec![over(Outcome::Stopped)]].concat()
        );
        let error = "actor Player 1 failed: no model".to_string();
        let failed = recorded_as(1, &[thinking], Ended::Failed(error.clone()));
        assert_eq!(
            beats_of(&failed),
            [
                calls,
                vec![
                    Beat::Told(vec![error.clone()]),
                    over(Outcome::Failed(error))
                ]
            ]
            .concat()
        );
    }

    #[tokio::test]
    async fn an_episode_that_fails_after_a_win_ends_failed() {
        let events = random_avalon().await;
        let failed = recorded_as(1, &events, Ended::Failed("no model".to_string()));
        check_whole(&only(&failed), |outcome| {
            *outcome == Outcome::Failed("no model".to_string())
        });
    }

    #[tokio::test]
    async fn a_bad_line_leaves_out_its_episode_and_names_the_line() {
        let (log, _) = avalon_log(1..=3).await;
        let mut lines: Vec<&str> = log.lines().collect();
        let second = Uuid::from_u128(2).to_string();
        // The third line of the second episode, cut short.
        let bad = lines
            .iter()
            .position(|line| line.contains(&second))
            .unwrap()
            + 2;
        lines[bad] = &lines[bad][..20];
        let read = recordings(&lines.join("\n")).unwrap();
        assert_eq!(episodes(&read), [1, 3]);
        assert_eq!(read.problems.len(), 1);
        let number = bad + 1;
        let expected = format!("the Avalon episode {second} is bad at line {number}: ");
        assert!(
            read.problems[0].starts_with(&expected),
            "{:?}",
            read.problems
        );
    }

    #[test]
    fn a_log_whose_header_names_one_game_is_not_read_as_the_other() {
        let merlin = serde_json::json!({"Start": {"roles": {"Player 1": "Merlin"}}});
        let log = recorded::<Werewolf>(&[]) + &written(&[logged(merlin)]);
        let error = format!("{:#}", recordings(&log).unwrap_err());
        let episode = Uuid::default();
        assert!(
            error.starts_with(&format!(
                "the Werewolf episode {episode} is bad at line 3: "
            )),
            "{error}"
        );
    }

    #[test]
    fn a_log_of_a_newer_format_is_refused() {
        let header =
            serde_json::json!({"Header": {"format": 2, "version": "0.3.0", "game": "avalon"}});
        let error = recordings(&written(&[logged(header)])).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "episode {} is bad at line 1: \
                 This log was written by social-deduction 0.3.0 in log format 2. \
                 This version reads log formats up to 1.",
                Uuid::default()
            )
        );
    }

    #[test]
    fn a_log_of_neither_game_is_an_error() {
        let error = recordings(&written(&[logged(serde_json::json!({}))])).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "episode {} is of neither Avalon nor Werewolf at line 1",
                Uuid::default()
            )
        );
    }

    #[test]
    fn a_log_that_is_no_events_at_all_is_an_error_naming_each_line() {
        let error = recordings("{}\n\n[]").unwrap_err().to_string();
        let lines: Vec<&str> = error.lines().collect();
        assert_eq!(lines.len(), 2, "{error}");
        assert!(
            lines[0].starts_with("the log is bad at line 1: "),
            "{error}"
        );
        assert!(
            lines[1].starts_with("the log is bad at line 3: "),
            "{error}"
        );
    }

    #[test]
    fn an_empty_log_holds_no_recordings() {
        assert_eq!(recordings("\n").unwrap(), Recordings::default());
    }

    #[tokio::test]
    async fn a_bad_line_in_a_log_is_an_error_of_its_game_naming_the_line() {
        let events = random_avalon().await;
        let log = written(&events);
        let lines = log.lines().count();
        let truncated = &log[..log.len() - 10];
        let error = format!("{:#}", recordings(truncated).unwrap_err());
        let episode = events[0].episode;
        assert!(
            error.starts_with(&format!(
                "the Avalon episode {episode} is bad at line {lines}: "
            )),
            "{error}"
        );
        let events = random_werewolf().await;
        let log = format!("{}\n{{}}\n", written(&events));
        let error = format!("{:#}", recordings(&log).unwrap_err());
        let episode = events[0].episode;
        // The blank line counts as a line.
        let bad = events.len() + 2;
        assert!(
            error.starts_with(&format!(
                "the Werewolf episode {episode} is bad at line {bad}: "
            )),
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
        let beats = beats_of(&log);
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
                Beat::Over {
                    outcome: Outcome::Unfinished
                },
            ]
        );
        let mut stage = Stage::<Avalon>::default();
        let unfinished = Beat::Over {
            outcome: Outcome::Unfinished,
        };
        assert_eq!(
            stage.end(5, None, None),
            [Cue {
                at: 5,
                beat: unfinished
            }]
        );
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
