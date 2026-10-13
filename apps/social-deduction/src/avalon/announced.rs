//! The announcing environment, which runs every game: it opens each
//! phase with a message to every player, awaited in it or not, takes the
//! awaited players' choices as they come, and ends the phase. Every
//! player hears every phase begin, and may talk in it; only the players
//! the phase awaits choose. Players who take seconds to decide hold
//! nothing up.
//!
//! A phase takes each awaited player's first legal choice and ends once
//! every awaited player has chosen or at its limit, whichever comes
//! first. A choice is legal when it is of the kind the phase takes, a team
//! is the size the quest needs of distinct players at the table, a
//! sabotage comes from an evil player, and the Assassin's target is
//! another player it does not see as evil. A choice that is not legal,
//! from a player the phase does not await, for another phase, or after the
//! player's first, is treated like a stale one: observed, and nothing
//! more.
//!
//! The environment logs the deal, then the configuration; an [`Entry::Action`] for each
//! announcement it sends, numbered in one series from 1; an
//! [`Entry::Observation`] for each choice it receives, counted or not,
//! with the phase it was in when it arrived; how each phase came out, as
//! [`Entry::Proposed`], [`Entry::Tallied`], [`Entry::Quested`] or
//! [`Entry::Assassinated`]; and the result. The reminders it sends itself
//! are not logged.

use super::{
    Choice, Choices, Dealt, Entry, Message, PlayerId, Rules, State, Team, Victory, act, observe,
};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Context, Think};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// Everything the episode needs to run the environment for a game of
/// `deal`, every player's role and strategy, under `rules`, as the actor
/// `wrap` makes of it: it may send to and stop every player, holds the
/// logger, and thinks. It logs the deal, then `configuration`, before it
/// opens the first proposal. The winning team is sent on `winner` when the
/// game ends.
pub fn init<A>(
    deal: HashMap<PlayerId, Dealt>,
    rules: Rules,
    configuration: Entry,
    winner: oneshot::Sender<Team>,
    wrap: impl FnOnce(Environment) -> A + Send + 'static,
) -> ActorInit<A>
where
    A: Behavior<Message = Message, Log = Entry>,
{
    let players: HashSet<PlayerId> = deal.keys().cloned().collect();
    let opening = Box::new([rules.start(&deal), configuration]);
    let roles = deal
        .into_iter()
        .map(|(id, dealt)| (id, dealt.role))
        .collect();
    let game = Arc::new(Mutex::new(Game::new(roles, rules, winner)));
    let shared = Arc::clone(&game);
    ActorInit {
        behavior: Box::new(move |context| {
            wrap(Environment {
                context,
                game,
                opening: Some(opening),
            })
        }),
        think: Some(Box::new(move |context| {
            Box::new(Environment {
                context,
                game: shared,
                opening: None,
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
    /// The number of the current phase. The first proposal is 1.
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
/// chosen.
#[derive(Default)]
struct Underway {
    /// The players the phase awaits.
    awaited: HashSet<PlayerId>,
    /// Each awaited player's first legal choice.
    choices: Choices,
}

/// What the environment does about a choice that counted.
#[derive(Debug, Default, PartialEq)]
struct Counted {
    /// The choice was the last the phase was waiting for: end it.
    last: bool,
}

impl Game {
    /// A game of `roles` under `rules`, before its first proposal. The
    /// winning team is sent on `winner` when the game ends.
    fn new(
        roles: HashMap<PlayerId, super::Role>,
        rules: Rules,
        winner: oneshot::Sender<Team>,
    ) -> Self {
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

    /// Open the next phase: number it, write every player its
    /// announcement, numbered in turn in seat order, and wait on the
    /// players it awaits.
    fn open(&mut self) -> Opened {
        self.seq += 1;
        let awaited = self.state.awaited();
        let limit = self.rules.limit(self.state.phase);
        let mut announcements = Vec::new();
        for player in &self.state.seats {
            self.n += 1;
            let message = Message::Announce {
                seq: self.seq,
                n: self.n,
                view: self.state.view(player),
                limit,
            };
            announcements.push((player.clone(), message));
        }
        self.underway = Some(Underway {
            awaited,
            ..Underway::default()
        });
        Opened {
            seq: self.seq,
            announcements,
            limit,
        }
    }

    /// Record `from`'s `choice` in the phase numbered `seq` if it counts:
    /// the phase is the one underway, it awaits `from`, this is the first
    /// choice of `from`'s to count in it, and the rules allow it. Returns
    /// what the environment does about a choice that counted, and nothing
    /// for one that did not.
    fn choose(&mut self, seq: u64, from: &PlayerId, choice: Choice) -> Option<Counted> {
        if seq != self.seq {
            return None;
        }
        let underway = self.underway.as_mut()?;
        if !underway.awaited.contains(from)
            || underway.choices.contains_key(from)
            || !self.state.legal(from, &choice)
        {
            return None;
        }
        underway.choices.insert(from.clone(), choice);
        Some(Counted {
            last: underway.choices.len() == underway.awaited.len(),
        })
    }

    /// End the phase numbered `seq` at its limit, or because everyone it
    /// awaits has chosen, unless it has already ended. Returns how it
    /// ended, or nothing if it had already.
    fn end(&mut self, seq: u64) -> Option<Ended> {
        if seq != self.seq {
            return None;
        }
        let Underway { choices, .. } = self.underway.take()?;
        let entry = self.state.resolve(&choices, seq);
        let winner = self.state.winner();
        if winner.is_none() {
            self.state.next();
        }
        Some(Ended { entry, winner })
    }
}

/// How a phase ended: the entry that records what it came to, and who
/// won the game and how, if anyone did.
struct Ended {
    entry: Entry,
    winner: Option<(Team, Victory)>,
}

/// A phase just opened: its number, the announcement for each player, and
/// how long it waits for the awaited ones.
struct Opened {
    seq: u64,
    announcements: Vec<(PlayerId, Message)>,
    limit: Duration,
}

/// The actor that holds the game. Its perceive loop and its think loop are
/// each an `Environment` with a context of its own over the one shared
/// game: the perceive loop opens the first proposal and takes choices,
/// and the think loop ends phases.
/// [`Actor::Environment`](super::Actor::Environment) holds the perceive
/// loop's, built by [`init`].
pub struct Environment {
    context: Context<Message, Entry>,
    game: Arc<Mutex<Game>>,
    /// The entries the perceive loop opens the log with: the deal, then
    /// the configuration. Taken when logged. Boxed to keep the environment
    /// no larger than a player in [`Actor`](super::Actor).
    opening: Option<Box<[Entry; 2]>>,
}

impl Environment {
    /// The game, locked for as long as the guard lives. Never held across
    /// an await.
    fn game(&self) -> MutexGuard<'_, Game> {
        self.game.lock().unwrap()
    }

    /// Open the next phase: show each player what it may see of it and
    /// how long it lasts, each announcement an action of its own, and set
    /// the timer that ends it at its limit.
    fn open_phase(&self) -> anyhow::Result<()> {
        let Opened {
            seq,
            announcements,
            limit,
        } = self.game().open();
        for (to, message) in announcements {
            act(&self.context, message, HashSet::from([to]), None, None)?;
        }
        self.context.think_after(limit, Message::EndPhase { seq })
    }

    /// Carry out the end of a phase, if `ended` says it did: log what it
    /// came to, and then end the game or open the next phase.
    fn end_phase(&self, ended: Option<Ended>) -> anyhow::Result<()> {
        let Some(Ended { entry, winner }) = ended else {
            return Ok(());
        };
        self.context.log(entry);
        match winner {
            Some((team, by)) => self.end_game(team, by),
            None => self.open_phase(),
        }
    }

    /// Stop the players, log the result, send the winner, and shut down.
    /// The players stop first: a player still waiting on its model is
    /// dropped at that wait, and logs nothing after the end.
    fn end_game(&self, winner: Team, by: Victory) -> anyhow::Result<()> {
        let (players, took, report) = {
            let mut game = self.game();
            (
                game.state.seats.clone(),
                game.started.elapsed(),
                game.winner.take(),
            )
        };
        for player in &players {
            self.context.stop(player)?;
        }
        self.context.log(Entry::End { winner, by, took });
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

    /// Log a choice as observed in the current phase, counted or not, and
    /// record it if it counts. The last choice a phase is waiting for ends
    /// it. Anything else is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        let Some((seq, n, choice)) = message.choice() else {
            return Ok(());
        };
        let from = message.sender();
        let (current, counted) = {
            let mut game = self.game();
            (game.seq, game.choose(seq, &from, choice))
        };
        observe(&self.context, from, n, current);
        // A choice counts only in the phase underway.
        if counted.is_some_and(|counted| counted.last) {
            self.context.think(Message::EndPhase { seq: current })?;
        }
        Ok(())
    }

    /// Log the deal and the limits, then the configuration, and open the
    /// first proposal.
    async fn start(&mut self) -> anyhow::Result<()> {
        for entry in self.opening.take().into_iter().flat_map(|opening| *opening) {
            self.context.log(entry);
        }
        self.open_phase()
    }
}

#[async_trait]
impl Think for Environment {
    type Message = Message;

    /// End the phase the message says is over, unless it has ended
    /// already. Nothing else is thought about.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::EndPhase { seq } = message {
            let ended = self.game().end(*seq);
            self.end_phase(ended)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::tests::{
        Script, ending, five, mute, observed_by, outcomes, play, quick, seated, sorted,
    };
    use crate::avalon::{Actor, Ballot, Deed, Phase, QuestResult, Role, Tally};
    use crate::seat::ENVIRONMENT;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// The default table, with its first proposal open.
    fn opened() -> (Game, Opened) {
        let (winner, _) = oneshot::channel();
        let mut game = Game::new(five(), Rules::default(), winner);
        let proposal = game.open();
        (game, proposal)
    }

    /// The players the phase underway awaits, in name order.
    fn awaited(game: &Game) -> Vec<PlayerId> {
        sorted(game.underway.as_ref().unwrap().awaited.clone())
    }

    /// A team of the players named.
    fn team(players: &[&str]) -> Choice {
        Choice::Team(players.iter().map(|player| id(player)).collect())
    }

    /// A choice that counted and calls for nothing more.
    fn counted() -> Option<Counted> {
        Some(Counted::default())
    }

    /// A choice that counted and was the last the phase waited for.
    fn last() -> Option<Counted> {
        Some(Counted { last: true })
    }

    #[test]
    fn opening_a_phase_numbers_it_announces_it_to_everyone_and_waits_on_the_awaited() {
        let (mut game, proposal) = opened();
        assert_eq!(proposal.seq, 1);
        let to: Vec<&PlayerId> = proposal.announcements.iter().map(|(to, _)| to).collect();
        assert_eq!(
            to,
            ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"]
        );
        let numbers: Vec<Option<u64>> = proposal
            .announcements
            .iter()
            .map(|(_, message)| message.number())
            .collect();
        assert_eq!(numbers, [1, 2, 3, 4, 5].map(Some));
        assert_eq!(proposal.limit, Rules::default().propose_limit);
        assert_eq!(awaited(&game), ["Player 1"]);
        // The leader proposes, which is all the proposal waits for.
        assert_eq!(
            game.choose(1, &id("Player 1"), team(&["Player 1", "Player 2"])),
            last()
        );
        game.end(1).unwrap();
        let vote = game.open();
        assert_eq!(vote.seq, 2);
        assert_eq!(vote.announcements[0].1.number(), Some(6));
        assert_eq!(vote.limit, Rules::default().vote_limit);
        assert_eq!(awaited(&game).len(), 5);
    }

    #[test]
    fn announcements_are_numbered_in_seat_order_with_player_2_before_player_10() {
        let (winner, _) = oneshot::channel();
        let mut roles = seated(&[Role::Servant; 10]);
        roles.insert(id("Player 1"), Role::Merlin);
        roles.insert(id("Player 2"), Role::Assassin);
        let mut game = Game::new(roles, Rules::default(), winner);
        let opened = game.open();
        let to: Vec<&str> = opened
            .announcements
            .iter()
            .map(|(to, _)| to.as_str())
            .collect();
        let expected: Vec<String> = (1..=10).map(|seat| format!("Player {seat}")).collect();
        assert_eq!(to, expected);
    }

    #[test]
    fn the_last_choice_of_a_phase_ends_it() {
        let (mut game, Opened { seq, .. }) = opened();
        game.choose(seq, &id("Player 1"), team(&["Player 1", "Player 2"]));
        game.end(seq);
        let vote = game.open().seq;
        for voter in ["Player 1", "Player 2", "Player 3", "Player 4"] {
            assert_eq!(
                game.choose(vote, &id(voter), Choice::Ballot(Ballot::Approve)),
                counted()
            );
        }
        assert_eq!(
            game.choose(vote, &id("Player 5"), Choice::Ballot(Ballot::Reject)),
            last()
        );
    }

    #[test]
    fn a_choice_that_is_late_early_unawaited_second_or_illegal_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        let proposal = team(&["Player 1", "Player 2"]);
        assert_eq!(
            game.choose(seq - 1, &id("Player 1"), proposal.clone()),
            None
        );
        assert_eq!(
            game.choose(seq + 1, &id("Player 1"), proposal.clone()),
            None
        );
        // Only the leader proposes.
        assert_eq!(game.choose(seq, &id("Player 2"), proposal.clone()), None);
        // A team of the wrong size, or a vote, is not a proposal.
        assert_eq!(game.choose(seq, &id("Player 1"), team(&["Player 1"])), None);
        assert_eq!(
            game.choose(seq, &id("Player 1"), Choice::Ballot(Ballot::Approve)),
            None
        );
        assert!(game.underway.as_ref().unwrap().choices.is_empty());
        assert_eq!(game.choose(seq, &id("Player 1"), proposal), last());
        // A second proposal does not count.
        assert_eq!(
            game.choose(seq, &id("Player 1"), team(&["Player 3", "Player 4"])),
            None
        );
        // Nor does one after the phase has ended.
        game.end(seq).unwrap();
        assert_eq!(
            game.choose(seq, &id("Player 1"), team(&["Player 3", "Player 4"])),
            None
        );
    }

    #[test]
    fn a_phase_that_has_ended_does_not_end_again() {
        let (mut game, Opened { seq, .. }) = opened();
        assert!(game.end(seq).is_some());
        game.open();
        assert!(game.end(seq).is_none());
    }

    /// A leader who proposes itself and the next seat in seat order, a
    /// voter who approves, a member who does `deed`, and an Assassin who
    /// names `target`: the script of a player who does the same whatever
    /// it is.
    fn always(deed: Deed, target: &'static str) -> Script {
        Box::new(move |me, seq, view| {
            if !crate::avalon::awaited(me, view) {
                return vec![];
            }
            let choice = match view.phase {
                Phase::Propose => {
                    let at = view.players.iter().position(|p| p == me).unwrap();
                    let team = (0..view.size)
                        .map(|i| view.players[(at + i) % view.players.len()].clone())
                        .collect();
                    Choice::Team(team)
                }
                Phase::Vote => Choice::Ballot(Ballot::Approve),
                Phase::Quest => Choice::Deed(deed),
                Phase::Assassinate => Choice::Target(id(target)),
            };
            vec![choice.message(seq, me.clone(), seq)]
        })
    }

    /// The default table, every player playing `always(deed, target)`.
    fn everyone(deed: Deed, target: &'static str) -> HashMap<&'static str, crate::avalon::Seat> {
        ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"]
            .into_iter()
            .map(|player| (player, Actor::puppet(always(deed, target))))
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_of_three_successful_quests_and_merlin_named_is_won_by_evil_without_waiting() {
        let (winner, logged, took) = play(five(), quick(), everyone(Deed::Support, "Player 1"))
            .await
            .unwrap();
        assert_eq!(winner, Team::Evil);
        assert_eq!(ending(&logged), (Team::Evil, Victory::Assassination));
        assert_eq!(took, Duration::ZERO);
        // Three rounds of proposal, vote and quest, then the assassination,
        // each phase ending with exactly one outcome.
        let outcomes = outcomes(&logged);
        assert_eq!(outcomes.len(), 10, "{outcomes:?}");
        for (i, outcome) in outcomes.iter().enumerate() {
            let seq = i as u64 + 1;
            let ok = match i {
                9 => {
                    matches!(outcome, Entry::Assassinated { seq: s, target: Some(t), merlin: true } if *s == seq && t == "Player 1")
                }
                _ => match i % 3 {
                    0 => matches!(outcome, Entry::Proposed { seq: s, team: Some(_) } if *s == seq),
                    1 => {
                        matches!(outcome, Entry::Tallied { seq: s, tally: Tally { approved: true, .. } } if *s == seq)
                    }
                    _ => {
                        matches!(outcome, Entry::Quested { seq: s, result: QuestResult { succeeded: true, .. } } if *s == seq)
                    }
                },
            };
            assert!(ok, "{outcome:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn naming_someone_else_wins_for_good() {
        let (winner, logged, _) = play(five(), quick(), everyone(Deed::Support, "Player 3"))
            .await
            .unwrap();
        assert_eq!(winner, Team::Good);
        assert_eq!(ending(&logged), (Team::Good, Victory::Quests));
    }

    #[tokio::test(start_paused = true)]
    async fn evil_sabotaging_every_quest_wins_once_three_fail() {
        // Every leader takes itself and the next seat, so evil, in the
        // second and fifth seats, is on the first three teams.
        let (winner, logged, _) = play(five(), quick(), everyone(Deed::Sabotage, "Player 1"))
            .await
            .unwrap();
        assert_eq!(winner, Team::Evil);
        assert_eq!(ending(&logged), (Team::Evil, Victory::Sabotage));
        // Merlin and the Servants supported: their sabotages did not count.
        let quests: Vec<&QuestResult> = outcomes(&logged)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Quested { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert_eq!(quests.len(), 4);
        let sabotages: Vec<usize> = quests.iter().map(|quest| quest.sabotages).collect();
        assert_eq!(sabotages, [1, 1, 0, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_leader_who_never_proposes_lets_five_proposals_lapse_and_evil_win() {
        let puppets = ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"]
            .into_iter()
            .map(|player| (player, mute()))
            .collect();
        let (winner, logged, took) = play(five(), quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Evil);
        assert_eq!(ending(&logged), (Team::Evil, Victory::Rejections));
        assert_eq!(took, quick().propose_limit * 5);
        let outcomes = outcomes(&logged);
        assert_eq!(outcomes.len(), 5);
        assert!(
            outcomes
                .iter()
                .all(|entry| matches!(entry, Entry::Proposed { team: None, .. })),
            "{outcomes:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_vote_waits_out_its_limit_for_a_player_who_never_votes() {
        // Four approve and one never votes: the team goes once the vote
        // runs out, and the quest goes on.
        let mut puppets = everyone(Deed::Support, "Player 3");
        puppets.insert("Player 5", mute());
        let (_, logged, took) = play(five(), quick(), puppets).await.unwrap();
        let tally = logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Tallied { tally, .. } => Some(tally),
                _ => None,
            })
            .unwrap();
        assert!(tally.approved);
        assert_eq!(tally.ballots.len(), 4);
        // Three votes, and an assassination, without Player 5's ballot.
        assert!(took >= quick().vote_limit * 3, "{took:?}");
    }

    /// [`always`], supporting and naming Player 3, after each of the
    /// `illegal` choices, and then `again`, if anything, as a second
    /// choice.
    fn cheating(illegal: Vec<Choice>, again: Option<Choice>) -> Script {
        Box::new(move |me, seq, view| {
            let mut said: Vec<Message> = illegal
                .iter()
                .zip(1..)
                .map(|(choice, i)| choice.clone().message(seq, me.clone(), 100 * i + seq))
                .collect();
            said.extend(always(Deed::Support, "Player 3")(me, seq, view));
            said.extend(
                again
                    .clone()
                    .map(|choice| choice.message(seq, me.clone(), 900 + seq)),
            );
            said
        })
    }

    #[tokio::test(start_paused = true)]
    async fn an_illegal_choice_is_observed_and_not_counted() {
        // Merlin sabotages; the Assassin names itself, an evil player it
        // sees, and a player not at the table; and a Servant proposes out
        // of turn, and votes again, against the team.
        let mut puppets = everyone(Deed::Support, "Player 3");
        let merlin = cheating(vec![Choice::Deed(Deed::Sabotage)], None);
        let assassin = cheating(
            ["Player 2", "Player 5", "nobody"]
                .map(|target| Choice::Target(id(target)))
                .into(),
            None,
        );
        let servant = cheating(
            vec![team(&["Player 3", "Player 4"])],
            Some(Choice::Ballot(Ballot::Reject)),
        );
        puppets.insert("Player 1", Actor::puppet(merlin));
        puppets.insert("Player 2", Actor::puppet(assassin));
        puppets.insert("Player 3", Actor::puppet(servant));
        let (winner, logged, _) = play(five(), quick(), puppets).await.unwrap();
        // The quests all succeed, and the Assassin names Player 3.
        assert_eq!(winner, Team::Good);
        let observed = observed_by(&logged, ENVIRONMENT);
        for choice in [
            ("Player 3", 101, 1),
            ("Player 3", 902, 2),
            ("Player 1", 103, 3),
            ("Player 2", 110, 10),
            ("Player 2", 210, 10),
            ("Player 2", 310, 10),
        ] {
            assert!(observed.contains(&choice), "{choice:?}: {observed:?}");
        }
        let outcomes = outcomes(&logged);
        assert!(
            matches!(outcomes[0], Entry::Proposed { team: Some(team), .. } if team == &["Player 1", "Player 2"]),
            "{outcomes:?}"
        );
        assert!(
            matches!(outcomes[1], Entry::Tallied { tally, .. } if tally.ballots[&id("Player 3")] == Ballot::Approve),
            "{outcomes:?}"
        );
        assert!(
            matches!(outcomes[2], Entry::Quested { result, .. } if result.sabotages == 0),
            "{outcomes:?}"
        );
        assert!(
            matches!(outcomes[9], Entry::Assassinated { target: Some(target), merlin: false, .. } if target == "Player 3"),
            "{outcomes:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_message_that_is_not_a_choice_is_neither_counted_nor_observed() {
        let meddler: Script = Box::new(|me, seq, _| {
            vec![
                Message::EndPhase { seq },
                crate::avalon::tests::utterance(seq, me, 1, "Hello."),
            ]
        });
        let mut puppets: HashMap<&str, crate::avalon::Seat> =
            HashMap::from([("Player 1", Actor::puppet(meddler))]);
        for player in ["Player 2", "Player 3", "Player 4", "Player 5"] {
            puppets.insert(player, mute());
        }
        let (_, logged, took) = play(five(), quick(), puppets).await.unwrap();
        assert!(observed_by(&logged, ENVIRONMENT).is_empty());
        assert_eq!(took, quick().propose_limit * 5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_without_merlin_ends_at_the_third_successful_quest() {
        let roles = seated(&[
            Role::Servant,
            Role::Servant,
            Role::Servant,
            Role::Minion,
            Role::Minion,
        ]);
        let (winner, logged, _) = play(roles, quick(), everyone(Deed::Support, "Player 1"))
            .await
            .unwrap();
        assert_eq!(winner, Team::Good);
        assert_eq!(ending(&logged), (Team::Good, Victory::Quests));
        assert!(
            !logged
                .iter()
                .any(|entry| matches!(entry, Entry::Assassinated { .. }))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_assassin_who_names_nobody_lets_good_win_at_the_limit() {
        let mut puppets = everyone(Deed::Support, "Player 1");
        puppets.insert(
            "Player 2",
            Actor::puppet(Box::new(|me, seq, view| match view.phase {
                Phase::Assassinate => vec![],
                _ => always(Deed::Support, "Player 1")(me, seq, view),
            })),
        );
        let (winner, logged, took) = play(five(), quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Good);
        assert_eq!(took, quick().assassinate_limit);
        assert!(logged.iter().any(|entry| matches!(
            entry,
            Entry::Assassinated {
                target: None,
                merlin: false,
                ..
            }
        )));
    }
}
