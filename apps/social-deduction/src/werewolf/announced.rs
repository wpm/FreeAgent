//! The announcing environment, shared by every variant: it opens each
//! phase with a message to every living player, awake in it or not, takes
//! the awake players' selections as they come, and ends the phase. A
//! player announced a phase it sleeps through learns the phase has begun,
//! and its selections do not count. Players who take seconds to decide
//! hold nothing up.
//!
//! A night, and a day without a ratify limit, take each player's first
//! selection and end once everyone awake has selected or at the limit,
//! whichever comes first. A day with a ratify limit takes each player's
//! latest selection as its vote and keeps the votes: whenever they change
//! the tentative decision, the player they select or nobody, every living
//! player is sent a [`Message::Tentative`], and while a player is selected
//! every selection restarts the ratify timer. The day ends when the timer
//! fires, and the selected player is voted out, or at the day limit,
//! whatever is standing, and nobody is.
//!
//! The environment logs the deal, then the configuration if there is one;
//! an [`Entry::Action`] for each announcement and tentative decision it
//! sends, numbered in one series from 1; an [`Entry::Observation`] for each
//! selection it receives, stale or not, with the phase it was in when the
//! selection arrived; each night's hunt as [`Entry::Hunted`]; each day's
//! vote as [`Entry::Voted`]; and the result. The reminders it sends itself
//! are not logged.

use super::{
    Choices, Ending, Entry, Hunt, Message, Phase, PlayerId, Role, Rules, State, Team, act, observe,
};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Context, Think};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// Everything the episode needs to run the environment for a game of
/// `roles` under `rules`, as the actor `wrap` makes of it: it may send to
/// and stop every player, holds the logger, and thinks. It logs the deal,
/// then `configuration` if there is one, before it opens the first night.
/// The winning team is sent on `winner` when the game ends.
pub fn init<A>(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    configuration: Option<Entry>,
    winner: oneshot::Sender<Team>,
    wrap: impl FnOnce(Environment) -> A + Send + 'static,
) -> ActorInit<A>
where
    A: Behavior<Message = Message, Log = Entry>,
{
    let players: HashSet<PlayerId> = roles.keys().cloned().collect();
    let game = Arc::new(Mutex::new(Game::new(roles, rules, winner)));
    let shared = Arc::clone(&game);
    ActorInit {
        behavior: Box::new(move |context| {
            wrap(Environment {
                context,
                game,
                configuration: configuration.map(Box::new),
            })
        }),
        think: Some(Box::new(move |context| {
            Box::new(Environment {
                context,
                game: shared,
                configuration: None,
            })
        })),
        can_send_to: players.clone(),
        can_shut_down: players,
        has_logger: true,
    }
}

/// What the environment's two loops share: the game, the phase it is at,
/// and where the winner goes.
struct Game {
    state: State,
    rules: Rules,
    /// The number of the current phase. The first night is 1.
    seq: u64,
    /// The number of the last message the environment sent to a player.
    /// Each announcement is a message of its own.
    n: u64,
    /// The phase underway, while one is: taken when it ends, so that
    /// nothing more counts until the next one opens.
    underway: Option<Underway>,
    /// Where the winning team goes when the game ends. Taken when sent.
    winner: Option<oneshot::Sender<Team>>,
    /// When the game was made, which is as good as when it was dealt.
    started: Instant,
}

/// The phase underway: whom the environment waits on and what they have
/// said.
#[derive(Default)]
struct Underway {
    /// The players awake in the phase.
    awake: HashSet<PlayerId>,
    /// Each awake player's selection: its latest in a day with a ratify
    /// limit, its first otherwise.
    selections: Choices,
    /// In a day with a ratify limit, the tentative decision standing: the
    /// player the votes select, if they select anyone.
    tentative: Option<PlayerId>,
    /// How many selections have counted, which numbers the ratify timers so
    /// that a stale one is known.
    count: u64,
}

/// What the environment does about a selection that counted.
#[derive(Debug, Default, PartialEq)]
struct Counted {
    /// The selection was the last the phase was waiting for: end it.
    last: bool,
    /// The selection changed the tentative decision: tell every living
    /// player.
    tentative: Option<(Message, HashSet<PlayerId>)>,
    /// A player is selected: restart the ratify timer, to fire this
    /// reminder after the ratify limit.
    ratify: Option<(Duration, Message)>,
}

impl Game {
    /// A game of `roles` under `rules`, before its first night. The
    /// winning team is sent on `winner` when the game ends.
    fn new(roles: HashMap<PlayerId, Role>, rules: Rules, winner: oneshot::Sender<Team>) -> Self {
        Self {
            state: State::new(roles),
            rules,
            seq: 0,
            n: 0,
            underway: None,
            winner: Some(winner),
            started: Instant::now(),
        }
    }

    /// Open the next phase: number it, write every living player its
    /// announcement, numbered in turn in name order, and wait on the
    /// players awake in it.
    fn open(&mut self) -> anyhow::Result<Opened> {
        self.seq += 1;
        let awake = self.state.awake();
        let limit = match self.state.phase {
            Phase::Night => self.rules.night_limit,
            Phase::Day => self.rules.day_limit,
        };
        let mut alive: Vec<&PlayerId> = self.state.alive.iter().collect();
        alive.sort();
        let mut announcements = Vec::new();
        for player in alive {
            self.n += 1;
            let message = Message::Announce {
                seq: self.seq,
                n: self.n,
                view: self.state.view(player.clone())?,
                limit,
            };
            announcements.push((player.clone(), message));
        }
        self.underway = Some(Underway {
            awake,
            ..Underway::default()
        });
        Ok(Opened {
            seq: self.seq,
            announcements,
            limit,
        })
    }

    /// The ratify limit, if the current phase is a day with one.
    fn ratifying(&self) -> Option<Duration> {
        match self.state.phase {
            Phase::Day => self.rules.ratify_limit,
            Phase::Night => None,
        }
    }

    /// Record `from`'s selection of `target` in the phase numbered `seq`
    /// if it counts: the phase is the one underway and `from` is awake in
    /// it, and, unless the day's vote is ratified, this is its first
    /// selection. Returns what the environment does about a selection that
    /// counted, and nothing for one that did not.
    fn select(&mut self, seq: u64, from: &PlayerId, target: &PlayerId) -> Option<Counted> {
        if seq != self.seq {
            return None;
        }
        let ratifying = self.ratifying();
        let underway = self.underway.as_mut()?;
        if !underway.awake.contains(from) {
            return None;
        }
        let Some(ratify) = ratifying else {
            if underway.selections.contains_key(from) {
                return None;
            }
            underway.selections.insert(from.clone(), target.clone());
            return Some(Counted {
                last: underway.selections.len() == underway.awake.len(),
                ..Counted::default()
            });
        };
        underway.selections.insert(from.clone(), target.clone());
        underway.count += 1;
        let selected = self.rules.day_vote.elect(&underway.selections);
        let tentative = (selected != underway.tentative).then(|| {
            self.n += 1;
            let message = Message::Tentative {
                seq: self.seq,
                n: self.n,
                target: selected.clone(),
                ratify,
            };
            (message, self.state.alive.clone())
        });
        underway.tentative = selected;
        let ratify = underway.tentative.is_some().then(|| {
            let reminder = Message::Ratify {
                seq: self.seq,
                count: underway.count,
            };
            (ratify, reminder)
        });
        Some(Counted {
            last: false,
            tentative,
            ratify,
        })
    }

    /// End the phase numbered `seq` at its limit, or because everyone awake
    /// has selected, unless it has already ended. At the limit of a day
    /// whose vote is ratified nothing stands, whatever was. Returns how the
    /// phase ended, or nothing if it had already.
    fn end(&mut self, seq: u64) -> Option<Ended> {
        if seq != self.seq {
            return None;
        }
        let Underway {
            awake, selections, ..
        } = self.underway.take()?;
        let (ballots, by) = match self.ratifying() {
            Some(_) => (Choices::new(), Ending::DayLimit),
            None if selections.len() == awake.len() => (selections, Ending::AllSelected),
            None => (selections, Ending::DayLimit),
        };
        Some(self.conclude(&ballots, by))
    }

    /// End the day numbered `seq` because its votes have stood for the
    /// ratify limit since its `count`th counting selection, unless the
    /// reminder is stale: a later selection has counted since, or the day
    /// has ended. Returns how it ended, or nothing if the reminder was
    /// stale.
    fn ratify(&mut self, seq: u64, count: u64) -> Option<Ended> {
        if seq != self.seq || self.underway.as_ref()?.count != count {
            return None;
        }
        let Underway { selections, .. } = self.underway.take()?;
        Some(self.conclude(&selections, Ending::Ratified))
    }

    /// End the current phase: resolve it from `ballots` under its vote, a
    /// day as ended `by`, and move on to the next phase if the game goes
    /// on.
    fn conclude(&mut self, ballots: &Choices, by: Ending) -> Ended {
        let outcome = match self.state.phase {
            Phase::Night => Outcome::Night(
                self.state
                    .resolve_night(ballots, self.rules.night_vote.as_ref()),
            ),
            Phase::Day => Outcome::Day {
                out: self
                    .state
                    .resolve_day(ballots, self.rules.day_vote.as_ref()),
                by,
            },
        };
        let winner = self.state.winner();
        if winner.is_none() {
            self.state.next();
        }
        Ended { outcome, winner }
    }
}

/// How a phase ended: what it came to, and who won the game if anyone.
struct Ended {
    outcome: Outcome,
    winner: Option<Team>,
}

/// What a phase came to: by night the hunt, if the werewolves elected
/// anyone; by day who was voted out, if anyone, and how the day ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    Night(Option<Hunt>),
    Day { out: Option<PlayerId>, by: Ending },
}

impl Outcome {
    /// Who died in the phase, if anyone.
    fn dead(&self) -> Option<&PlayerId> {
        match self {
            Outcome::Night(hunt) => hunt.as_ref().and_then(Hunt::dead),
            Outcome::Day { out, .. } => out.as_ref(),
        }
    }

    /// The entry that records the phase numbered `seq` coming to this: a
    /// night's hunt, if the werewolves elected anyone, or a day's vote.
    fn entry(self, seq: u64) -> Option<Entry> {
        match self {
            Outcome::Night(hunt) => hunt.map(|hunt| Entry::Hunted { seq, hunt }),
            Outcome::Day { out, by } => Some(Entry::Voted { seq, out, by }),
        }
    }
}

/// A phase just opened: its number, the announcement for each living
/// player, and how long it waits for the awake ones.
struct Opened {
    seq: u64,
    announcements: Vec<(PlayerId, Message)>,
    limit: Duration,
}

/// The actor that holds the game. Its perceive loop and its think loop are
/// each an `Environment` with a context of its own over the one shared
/// game: the perceive loop opens the first night, takes selections and
/// announces tentative decisions, and the think loop ends phases. A
/// variant's actor enum holds the perceive loop's, built by [`init`].
pub struct Environment {
    context: Context<Message, Entry>,
    game: Arc<Mutex<Game>>,
    /// The entry the perceive loop logs right after the deal, if any.
    /// Taken when logged. Boxed to keep the environment no larger than a
    /// player in a variant's actor enum.
    configuration: Option<Box<Entry>>,
}

impl Environment {
    /// The game, locked for as long as the guard lives. Never held across
    /// an await.
    fn game(&self) -> MutexGuard<'_, Game> {
        self.game.lock().unwrap()
    }

    /// Open the next phase: show each living player what it may see of it
    /// and how long it lasts, each announcement an action of its own, and
    /// set the timer that ends it at its limit.
    fn open_phase(&self) -> anyhow::Result<()> {
        let Opened {
            seq,
            announcements,
            limit,
        } = self.game().open()?;
        for (to, message) in announcements {
            act(&self.context, message, HashSet::from([to]), None)?;
        }
        self.context.think_after(limit, Message::EndPhase { seq })
    }

    /// Carry out the end of the phase numbered `seq`, if `ended` says it
    /// did: log the hunt or the vote, stop whoever died, and then end the
    /// game or open the next phase.
    fn end_phase(&self, seq: u64, ended: Option<Ended>) -> anyhow::Result<()> {
        let Some(Ended { outcome, winner }) = ended else {
            return Ok(());
        };
        if let Some(dead) = outcome.dead() {
            self.context.stop(dead)?;
        }
        if let Some(entry) = outcome.entry(seq) {
            self.context.log(entry);
        }
        match winner {
            Some(team) => self.end_game(team),
            None => self.open_phase(),
        }
    }

    /// Stop the survivors, log the result, send the winner, and shut down.
    /// The survivors stop first: a player still waiting on its model is
    /// dropped at that wait, and logs nothing after the end.
    fn end_game(&self, winner: Team) -> anyhow::Result<()> {
        let (days, survivors, took, report) = {
            let mut game = self.game();
            (
                game.state.days(),
                game.state.alive.clone(),
                game.started.elapsed(),
                game.winner.take(),
            )
        };
        for player in &survivors {
            self.context.stop(player)?;
        }
        self.context.log(Entry::End {
            winner,
            days,
            survivors,
            took,
        });
        if let Some(report) = report {
            // The owner of the episode may have stopped listening.
            let _ = report.send(winner);
        }
        self.context.shutdown();
        Ok(())
    }
}

#[async_trait]
impl Behavior for Environment {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log a selection as observed in the current phase, stale or not, and
    /// record it if it counts. One that changes the tentative decision has
    /// every living player told; one that leaves a player selected restarts
    /// the ratify timer; the last one a phase is waiting for ends it.
    /// Anything else is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::Select {
            seq,
            from,
            n,
            target,
        } = message
        {
            let (current, counted) = {
                let mut game = self.game();
                (game.seq, game.select(*seq, from, target))
            };
            observe(&self.context, from.clone(), *n, current);
            if let Some(Counted {
                last,
                tentative,
                ratify,
            }) = counted
            {
                if let Some((message, to)) = tentative {
                    act(&self.context, message, to, None)?;
                }
                if let Some((limit, reminder)) = ratify {
                    self.context.think_after(limit, reminder)?;
                }
                if last {
                    self.context.think(Message::EndPhase { seq: *seq })?;
                }
            }
        }
        Ok(())
    }

    /// Log the deal and the limits, then the configuration if there is
    /// one, and open the first night.
    async fn start(&mut self) -> anyhow::Result<()> {
        let start = {
            let game = self.game();
            game.rules.start(game.state.roles.clone())
        };
        self.context.log(start);
        if let Some(configuration) = self.configuration.take() {
            self.context.log(*configuration);
        }
        self.open_phase()
    }
}

#[async_trait]
impl Think for Environment {
    type Message = Message;

    /// End the phase the message says is over, or the day whose votes it
    /// says have stood, unless they have ended already. Nothing else is
    /// thought about.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::EndPhase { seq } = message {
            let ended = self.game().end(*seq);
            self.end_phase(*seq, ended)?;
        }
        if let Message::Ratify { seq, count } = message {
            let ended = self.game().ratify(*seq, *count);
            self.end_phase(*seq, ended)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::candidates;
    use crate::werewolf::tests::{self, one_wolf_against, selection, village};
    use crate::werewolf::uniform_random::Actor;
    use crate::werewolf::uniform_random::tests::{Script, Seat, at_once, mute, play, quick};

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// The selections that count in the phase underway.
    fn selections(game: &Game) -> &Choices {
        &game.underway.as_ref().unwrap().selections
    }

    /// A game of `roles` under `rules`, with its first night open.
    fn opened_under(roles: HashMap<PlayerId, Role>, rules: Rules) -> (Game, Opened) {
        let (winner, _) = oneshot::channel();
        let mut game = Game::new(roles, rules, winner);
        let night = game.open().unwrap();
        (game, night)
    }

    /// The village, with its first night open.
    fn opened() -> (Game, Opened) {
        opened_under(village().roles, Rules::default())
    }

    /// Whom `opened` announces to, in name order, and the numbers of its
    /// announcements, in order.
    fn announced(opened: &Opened) -> (Vec<&PlayerId>, Vec<u64>) {
        let mut to: Vec<&PlayerId> = opened.announcements.iter().map(|(to, _)| to).collect();
        to.sort();
        let mut numbers: Vec<u64> = opened
            .announcements
            .iter()
            .map(|(_, message)| message.number().unwrap())
            .collect();
        numbers.sort_unstable();
        (to, numbers)
    }

    /// The players the phase underway waits on, in name order.
    fn awake(game: &Game) -> Vec<PlayerId> {
        tests::sorted(game.underway.as_ref().unwrap().awake.clone())
    }

    #[test]
    fn opening_a_phase_numbers_it_from_one_announces_it_to_the_living_and_waits_on_the_awake() {
        let (mut game, night) = opened();
        assert_eq!(night.seq, 1);
        let (to, _) = announced(&night);
        assert_eq!(to, ["seer", "villager", "wolf1", "wolf2"]);
        assert_eq!(awake(&game), ["seer", "wolf1", "wolf2"]);
        assert_eq!(night.limit, Rules::default().night_limit);
        game.state.alive.remove("villager");
        game.state.next();
        let day = game.open().unwrap();
        assert_eq!(day.seq, 2);
        assert_eq!(announced(&day).0, ["seer", "wolf1", "wolf2"]);
        assert_eq!(awake(&game), ["seer", "wolf1", "wolf2"]);
        assert_eq!(day.limit, Rules::default().day_limit);
    }

    #[test]
    fn each_announcement_is_numbered_in_one_series_from_one_and_carries_its_phase_and_limit() {
        let (mut game, night) = opened();
        assert_eq!(announced(&night).1, [1, 2, 3, 4]);
        game.state.next();
        let day = game.open().unwrap();
        assert_eq!(announced(&day).1, [5, 6, 7, 8]);
        for (_, message) in &day.announcements {
            assert!(
                matches!(message, Message::Announce { seq: 2, limit, .. }
                    if *limit == Rules::default().day_limit),
                "{message:?}"
            );
        }
    }

    /// A selection that counted and calls for nothing more.
    fn counted() -> Option<Counted> {
        Some(Counted::default())
    }

    /// A selection that counted and was the last the phase waited for.
    fn last() -> Option<Counted> {
        Some(Counted {
            last: true,
            ..Counted::default()
        })
    }

    #[test]
    fn the_last_selection_of_a_phase_ends_it() {
        let (mut game, Opened { seq, .. }) = opened();
        assert_eq!(game.select(seq, &id("wolf1"), &id("villager")), counted());
        assert_eq!(game.select(seq, &id("wolf2"), &id("villager")), counted());
        assert_eq!(game.select(seq, &id("seer"), &id("wolf1")), last());
    }

    #[test]
    fn a_selection_for_another_phase_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        assert_eq!(game.select(seq - 1, &id("wolf1"), &id("villager")), None);
        assert_eq!(game.select(seq + 1, &id("wolf1"), &id("villager")), None);
        assert!(selections(&game).is_empty());
    }

    #[test]
    fn a_selection_by_a_player_who_is_not_awake_does_not_count() {
        // The villager is announced the night and sleeps through it: its
        // selection is not kept, and the night still waits on the awake.
        let (mut game, Opened { seq, .. }) = opened();
        assert_eq!(game.select(seq, &id("villager"), &id("wolf1")), None);
        assert!(selections(&game).is_empty());
        assert_eq!(game.select(seq, &id("wolf1"), &id("villager")), counted());
        assert_eq!(game.select(seq, &id("wolf2"), &id("villager")), counted());
        assert_eq!(game.select(seq, &id("seer"), &id("wolf1")), last());
    }

    #[test]
    fn in_a_ratified_day_a_selection_by_a_player_who_is_not_awake_neither_counts_nor_restarts_the_timer()
     {
        // Ann dies the first night, so by day she is not awake; her vote
        // selects nobody, tells nobody, and sets no timer.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, ratified());
        game.select(seq, &id("wolf"), &id("ann"));
        game.end(seq);
        let day = game.open().unwrap().seq;
        assert_eq!(game.select(day, &id("ann"), &id("wolf")), None);
        assert!(selections(&game).is_empty());
        assert_eq!(game.underway.as_ref().unwrap().count, 0);
        assert_eq!(
            game.select(day, &id("bob"), &id("wolf")),
            Some(Counted {
                last: false,
                tentative: tentative_to(day, 8, Some("wolf"), &["wolf", "bob", "cat"]),
                ratify: timer(day, 1),
            })
        );
    }

    #[test]
    fn only_a_players_first_selection_in_a_phase_counts() {
        let (mut game, Opened { seq, .. }) = opened();
        game.select(seq, &id("wolf1"), &id("villager"));
        game.select(seq, &id("wolf2"), &id("villager"));
        assert_eq!(game.select(seq, &id("wolf1"), &id("seer")), None);
        assert_eq!(selections(&game)[&id("wolf1")], id("villager"));
    }

    #[test]
    fn a_selection_after_a_phase_has_ended_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        game.end(seq).unwrap();
        assert_eq!(game.select(seq, &id("wolf1"), &id("villager")), None);
    }

    #[test]
    fn a_phase_is_resolved_from_its_selections_under_its_vote() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, Rules::default());
        game.select(seq, &id("wolf"), &id("ann"));
        let night = game.end(seq).unwrap();
        let hunt = Hunt {
            victim: id("ann"),
            protectors: HashSet::new(),
        };
        assert_eq!(night.outcome, Outcome::Night(Some(hunt)));
        assert_eq!(night.outcome.dead(), Some(&id("ann")));
        assert_eq!(night.winner, None);
        // The game goes on, so the next phase is the day.
        let day = game.open().unwrap();
        assert_eq!(day.seq, 2);
        assert_eq!(day.limit, Rules::default().day_limit);
        game.select(day.seq, &id("wolf"), &id("bob"));
        game.select(day.seq, &id("bob"), &id("wolf"));
        let tied = game.end(day.seq).unwrap();
        assert_eq!(
            tied.outcome,
            Outcome::Day {
                out: None,
                by: Ending::DayLimit
            }
        );
        assert_eq!(tied.outcome.dead(), None);
        assert_eq!(tied.winner, None);
    }

    #[test]
    fn a_day_without_a_ratify_limit_ends_by_everyone_selecting_or_at_its_limit() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, Rules::default());
        game.end(seq);
        let day = game.open().unwrap().seq;
        for voter in ["wolf", "ann", "bob", "cat"] {
            game.select(day, &id(voter), &id("ann"));
        }
        let ended = game.end(day).unwrap();
        assert_eq!(
            ended.outcome,
            Outcome::Day {
                out: Some(id("ann")),
                by: Ending::AllSelected
            }
        );
        // A night's hunt is recorded only when the werewolves elected
        // someone; a day's vote always.
        assert_eq!(Outcome::Night(None).entry(1), None);
        assert_eq!(
            ended.outcome.entry(day),
            Some(Entry::Voted {
                seq: day,
                out: Some(id("ann")),
                by: Ending::AllSelected
            })
        );
    }

    #[test]
    fn a_phase_that_has_ended_does_not_end_again() {
        let (mut game, Opened { seq, .. }) = opened();
        assert!(game.end(seq).is_some());
        game.open().unwrap();
        assert!(game.end(seq).is_none());
    }

    #[test]
    fn a_phase_that_decides_the_game_is_the_last() {
        let (mut game, Opened { seq, .. }) = opened();
        game.state.alive.remove("wolf2");
        game.select(seq, &id("wolf1"), &id("villager"));
        let night = game.end(seq).unwrap();
        assert_eq!(night.outcome.dead(), Some(&id("villager")));
        assert_eq!(night.winner, Some(Team::Werewolves));
        assert_eq!(game.state.days(), 0);
    }

    /// Rules under which a day's votes stand after twenty seconds, as the
    /// shared [`tests::tentative`] has them.
    fn ratified() -> Rules {
        Rules {
            ratify_limit: Some(RATIFY),
            ..Rules::default()
        }
    }

    const RATIFY: Duration = Duration::from_secs(20);

    /// One werewolf against ann, bob and cat, with the first day open under
    /// [`ratified`], and the number of the day.
    fn ratifying_day() -> (Game, u64) {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, ratified());
        game.end(seq);
        let day = game.open().unwrap().seq;
        (game, day)
    }

    /// The tentative decision for the day numbered `seq`, the `n`th message
    /// the environment sent, naming `target` or nobody, to everyone alive,
    /// which is everyone.
    fn tentative(seq: u64, n: u64, target: Option<&str>) -> Option<(Message, HashSet<PlayerId>)> {
        tentative_to(seq, n, target, &["wolf", "ann", "bob", "cat"])
    }

    /// The tentative decision for the day numbered `seq`, the `n`th message
    /// the environment sent, naming `target` or nobody, to everyone in `to`.
    fn tentative_to(
        seq: u64,
        n: u64,
        target: Option<&str>,
        to: &[&str],
    ) -> Option<(Message, HashSet<PlayerId>)> {
        let to = to.iter().map(|name| id(name)).collect();
        Some((tests::tentative(seq, n, target), to))
    }

    /// The ratify timer set by the `count`th counting selection of the day
    /// numbered `seq`.
    fn timer(seq: u64, count: u64) -> Option<(Duration, Message)> {
        Some((RATIFY, Message::Ratify { seq, count }))
    }

    #[test]
    fn in_a_ratified_day_the_first_selection_selects_a_player_and_starts_the_timer() {
        let (mut game, day) = ratifying_day();
        // The environment has announced the night and the day to four
        // players each.
        assert_eq!(game.n, 8);
        assert_eq!(
            game.select(day, &id("ann"), &id("wolf")),
            Some(Counted {
                last: false,
                tentative: tentative(day, 9, Some("wolf")),
                ratify: timer(day, 1),
            })
        );
    }

    #[test]
    fn a_selection_that_leaves_the_decision_unchanged_restarts_the_timer_and_tells_nobody() {
        let (mut game, day) = ratifying_day();
        game.select(day, &id("ann"), &id("wolf"));
        assert_eq!(
            game.select(day, &id("bob"), &id("wolf")),
            Some(Counted {
                last: false,
                tentative: None,
                ratify: timer(day, 2),
            })
        );
        // The same player voting the same way again restarts it too.
        assert_eq!(
            game.select(day, &id("bob"), &id("wolf")),
            Some(Counted {
                last: false,
                tentative: None,
                ratify: timer(day, 3),
            })
        );
    }

    #[test]
    fn a_tie_selects_nobody_and_sets_no_timer() {
        let (mut game, day) = ratifying_day();
        game.select(day, &id("ann"), &id("wolf"));
        assert_eq!(
            game.select(day, &id("wolf"), &id("ann")),
            Some(Counted {
                last: false,
                tentative: tentative(day, 10, None),
                ratify: None,
            })
        );
        // The timer set before the tie is stale.
        assert!(game.ratify(day, 1).is_none());
        // A vote that breaks the tie selects again.
        assert_eq!(
            game.select(day, &id("bob"), &id("wolf")),
            Some(Counted {
                last: false,
                tentative: tentative(day, 11, Some("wolf")),
                ratify: timer(day, 3),
            })
        );
    }

    #[test]
    fn in_a_ratified_day_a_players_latest_selection_is_its_vote() {
        let (mut game, day) = ratifying_day();
        game.select(day, &id("ann"), &id("wolf"));
        assert_eq!(
            game.select(day, &id("ann"), &id("bob")),
            Some(Counted {
                last: false,
                tentative: tentative(day, 10, Some("bob")),
                ratify: timer(day, 2),
            })
        );
        assert_eq!(selections(&game)[&id("ann")], id("bob"));
        // Everyone selecting does not end the day.
        for voter in ["wolf", "bob", "cat"] {
            assert!(!game.select(day, &id(voter), &id("ann")).unwrap().last);
        }
    }

    #[test]
    fn by_night_the_first_selection_still_counts_whatever_the_ratify_limit() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, ratified());
        assert_eq!(game.select(seq, &id("wolf"), &id("ann")), last());
        assert_eq!(game.select(seq, &id("wolf"), &id("bob")), None);
        assert_eq!(selections(&game)[&id("wolf")], id("ann"));
    }

    #[test]
    fn the_ratify_timer_votes_out_the_selected_player_unless_it_is_stale() {
        let (mut game, day) = ratifying_day();
        game.select(day, &id("ann"), &id("wolf"));
        game.select(day, &id("bob"), &id("wolf"));
        // The first selection's timer is stale: a selection has counted
        // since. So is one for another day.
        assert!(game.ratify(day, 1).is_none());
        assert!(game.ratify(day + 1, 2).is_none());
        let ended = game.ratify(day, 2).unwrap();
        assert_eq!(
            ended.outcome,
            Outcome::Day {
                out: Some(id("wolf")),
                by: Ending::Ratified
            }
        );
        assert_eq!(ended.winner, Some(Team::Villagers));
        // The day has ended, so its timer is stale now too.
        assert!(game.ratify(day, 2).is_none());
    }

    #[test]
    fn the_day_limit_votes_nobody_out_whatever_is_standing() {
        let (mut game, day) = ratifying_day();
        game.select(day, &id("ann"), &id("wolf"));
        game.select(day, &id("bob"), &id("wolf"));
        let ended = game.end(day).unwrap();
        assert_eq!(
            ended.outcome,
            Outcome::Day {
                out: None,
                by: Ending::DayLimit
            }
        );
        assert_eq!(ended.winner, None);
        assert!(game.state.alive.contains("wolf"));
    }

    /// Rules under which a day's votes stand after five seconds, in a game
    /// of short phases.
    fn ratifying_quickly() -> Rules {
        Rules {
            ratify_limit: Some(Duration::from_secs(5)),
            ..quick()
        }
    }

    /// A werewolf who kills its first candidate each night at once and says
    /// nothing by day.
    fn hunting() -> Script {
        Box::new(|me, seq, view| match view.phase {
            Phase::Night => {
                let first = &candidates(me, view)[0];
                at_once(vec![selection(seq, me, seq, first)])
            }
            Phase::Day => vec![],
        })
    }

    /// Run a game of one werewolf against ann, bob and cat under
    /// [`ratifying_quickly`], in which the werewolf kills ann the first
    /// night and `bob` and `cat` are the seats given.
    async fn play_a_day(bob: Seat, cat: Seat) -> (Team, Vec<Entry>, Duration) {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let puppets = HashMap::from([
            ("wolf", Actor::puppet(hunting())),
            ("ann", mute()),
            ("bob", bob),
            ("cat", cat),
        ]);
        play(roles, ratifying_quickly(), puppets).await.unwrap()
    }

    /// A villager, announced the day alone, who votes against each of
    /// `targets` in turn, the first `first` after the day is announced and
    /// each of the rest `every` after the one before.
    fn voter(first: Duration, targets: &'static [&'static str], every: Duration) -> Seat {
        Actor::puppet(Box::new(move |me, seq, _| {
            targets
                .iter()
                .enumerate()
                .map(|(i, target)| {
                    let after = first + every * u32::try_from(i).unwrap();
                    (after, selection(seq, me, i as u64 + 1, target))
                })
                .collect()
        }))
    }

    /// How each day ended, in order.
    fn voted(logged: &[Entry]) -> Vec<(u64, Option<&str>, Ending)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Voted { seq, out, by } => Some((*seq, out.as_deref(), *by)),
                _ => None,
            })
            .collect()
    }

    /// Each tentative decision the environment logged sending, as whom it
    /// named and whom it was sent to, in order.
    fn tentatives(logged: &[Entry]) -> Vec<(Option<&str>, Vec<&str>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from,
                    to,
                    message: Message::Tentative { target, ratify, .. },
                    seen: None,
                    ..
                } => {
                    assert_eq!(from, super::super::ENVIRONMENT);
                    assert_eq!(*ratify, Duration::from_secs(5));
                    let mut to: Vec<&str> = to.iter().map(String::as_str).collect();
                    to.sort_unstable();
                    Some((target.as_deref(), to))
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_whose_votes_settle_ends_one_ratify_limit_after_the_last_and_votes_out_the_selected()
     {
        // Bob votes against the werewolf at once and cat a second later,
        // which restarts the timer without changing the decision, so the
        // day ends six seconds in, well before its ten-second limit.
        let bob = voter(Duration::ZERO, &["wolf"], Duration::ZERO);
        let cat = voter(Duration::from_secs(1), &["wolf"], Duration::ZERO);
        let (winner, logged, took) = play_a_day(bob, cat).await;
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(6));
        assert_eq!(voted(&logged), [(2, Some("wolf"), Ending::Ratified)]);
        assert_eq!(
            tentatives(&logged),
            [(Some("wolf"), vec!["bob", "cat", "wolf"])]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_of_vote_restarts_the_timer_and_the_latest_vote_counts() {
        // Bob votes against cat at once, then changes to the werewolf three
        // seconds in: the day ends eight seconds in, and the werewolf is
        // out, not cat.
        let bob = voter(Duration::ZERO, &["cat", "wolf"], Duration::from_secs(3));
        let (winner, logged, took) = play_a_day(bob, mute()).await;
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(8));
        assert_eq!(voted(&logged), [(2, Some("wolf"), Ending::Ratified)]);
        // Every living player is told each change of the tentative decision,
        // and the changes alone.
        let living = vec!["bob", "cat", "wolf"];
        assert_eq!(
            tentatives(&logged),
            [(Some("cat"), living.clone()), (Some("wolf"), living)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_vote_that_ties_selects_nobody_and_the_day_runs_out() {
        // Bob votes against cat at once, and cat against bob two seconds in.
        // The tie tells everyone nobody is selected and stops the timer: the
        // first vote's timer fires five seconds in and is ignored, and the
        // day runs out at ten with nobody voted out. The werewolf then kills
        // bob and wins.
        let bob = voter(Duration::ZERO, &["cat"], Duration::ZERO);
        let cat = voter(Duration::from_secs(2), &["bob"], Duration::ZERO);
        let (winner, logged, took) = play_a_day(bob, cat).await;
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(10));
        assert_eq!(voted(&logged), [(2, None, Ending::DayLimit)]);
        let living = vec!["bob", "cat", "wolf"];
        assert_eq!(
            tentatives(&logged),
            [(Some("cat"), living.clone()), (None, living)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_whose_votes_keep_changing_ends_at_its_limit_with_nobody_voted_out() {
        // Bob votes against the werewolf every three seconds, restarting
        // the five-second timer each time, so the day runs out at ten with
        // the werewolf selected and nobody voted out. The timers still
        // pending fire during the next night and are ignored.
        let bob = voter(
            Duration::ZERO,
            &["wolf", "wolf", "wolf", "wolf"],
            Duration::from_secs(3),
        );
        let (winner, logged, took) = play_a_day(bob, mute()).await;
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(10));
        assert_eq!(voted(&logged), [(2, None, Ending::DayLimit)]);
        assert_eq!(
            tentatives(&logged),
            [(Some("wolf"), vec!["bob", "cat", "wolf"])]
        );
    }
}
