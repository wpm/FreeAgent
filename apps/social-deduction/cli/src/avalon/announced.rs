//! Avalon's rules for the [announcing environment](crate::announced):
//! every phase is announced to every player, awaited in it or not. Every
//! player hears every phase begin, and may talk in it; only the players
//! the phase awaits choose.
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
//! How each phase came out is logged as [`Outcome::Proposed`],
//! [`Outcome::Tallied`], [`Outcome::Quested`] or [`Outcome::Assassinated`].

use super::{Avalon, Choice, Outcome, PlayerId, Role, State};
use crate::announced::{Announced, Awaited, Core, Counted, Opening, Over, Resolved};
use crate::game::Tagged;
use std::collections::HashMap;

impl Announced for Avalon {
    /// A phase ends at once: talk belongs to the game, not the phase.
    const CLOSES: bool = false;
    type State = State;
    type Underway = Awaited<Choice>;

    /// The first proposal, led by the first seat.
    fn state(roles: HashMap<PlayerId, Role>) -> State {
        State::new(roles)
    }

    /// Every seat, awaiting those the phase waits on.
    fn open(core: &Core<Self>) -> anyhow::Result<Opening<Self>> {
        let state = &core.state;
        Ok(Opening {
            views: state
                .seats
                .iter()
                .map(|player| (player.clone(), state.view(player)))
                .collect(),
            limit: core.rules.limit(state.phase),
            underway: Awaited::new(state.awaited()),
        })
    }

    /// An awaited player's first legal choice.
    fn counts(
        core: &mut Core<Self>,
        underway: &mut Awaited<Choice>,
        played: &super::Move,
    ) -> Option<Counted<Self>> {
        let (_, _, choice) = played.choice();
        let from = played.sender();
        if !underway.players.contains(&from) || !core.state.legal(&from, &choice) {
            return None;
        }
        underway.first(&from, choice)
    }

    /// The phase resolved from the choices made, and the game over once a
    /// team has won, every seat stopped.
    fn resolve(core: &mut Core<Self>, underway: Awaited<Choice>) -> Resolved<Self> {
        let outcome = core.state.resolve(&underway.choices, core.seq);
        let over = match core.state.winner() {
            Some((winner, by)) => Some(Over {
                winner,
                stopped: core.state.seats.clone(),
                end: Outcome::End {
                    winner,
                    by,
                    took: core.took(),
                },
            }),
            None => {
                core.state.next();
                None
            }
        };
        Resolved {
            outcome: Some(outcome),
            out: None,
            over,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announced::Opened;
    use crate::announced::tests::{counted, last};
    use crate::avalon::tests::{
        Script, ending, five, mute, observed_by, outcomes, play, quick, seated, sorted,
    };
    use crate::avalon::{
        Actor, Ballot, Deed, Entry, Message, Phase, QuestResult, Rules, Tally, Team, Victory,
    };
    use crate::seat::ENVIRONMENT;
    use crate::seat::id;
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// The default table, with its first proposal open.
    fn opened() -> (Core<Avalon>, Opened<Avalon>) {
        let (winner, _) = oneshot::channel();
        let mut game = Core::new(five(), Rules::default(), winner);
        let proposal = game.open().unwrap();
        (game, proposal)
    }

    /// The players the phase underway awaits, in name order.
    fn awaited(game: &Core<Avalon>) -> Vec<PlayerId> {
        sorted(game.underway.as_ref().unwrap().players.clone())
    }

    /// A team of the players named.
    fn team(players: &[&str]) -> Choice {
        Choice::Team(players.iter().map(|player| id(player)).collect())
    }

    /// Count `from`'s `choice` in the phase numbered `seq`.
    fn choose(
        game: &mut Core<Avalon>,
        seq: u64,
        from: &PlayerId,
        choice: Choice,
    ) -> Option<Counted<Avalon>> {
        game.count(&choice.made(seq, from.clone(), 1))
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
            choose(
                &mut game,
                1,
                &id("Player 1"),
                team(&["Player 1", "Player 2"])
            ),
            last()
        );
        game.end(1).unwrap();
        let vote = game.open().unwrap();
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
        let mut game = Core::<Avalon>::new(roles, Rules::default(), winner);
        let opened = game.open().unwrap();
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
        choose(
            &mut game,
            seq,
            &id("Player 1"),
            team(&["Player 1", "Player 2"]),
        );
        game.end(seq);
        let vote = game.open().unwrap().seq;
        for voter in ["Player 1", "Player 2", "Player 3", "Player 4"] {
            assert_eq!(
                choose(&mut game, vote, &id(voter), Choice::Ballot(Ballot::Approve)),
                counted()
            );
        }
        assert_eq!(
            choose(
                &mut game,
                vote,
                &id("Player 5"),
                Choice::Ballot(Ballot::Reject)
            ),
            last()
        );
    }

    #[test]
    fn a_choice_that_is_late_early_unawaited_second_or_illegal_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        let proposal = team(&["Player 1", "Player 2"]);
        assert_eq!(
            choose(&mut game, seq - 1, &id("Player 1"), proposal.clone()),
            None
        );
        assert_eq!(
            choose(&mut game, seq + 1, &id("Player 1"), proposal.clone()),
            None
        );
        // Only the leader proposes.
        assert_eq!(
            choose(&mut game, seq, &id("Player 2"), proposal.clone()),
            None
        );
        // A team of the wrong size, or a vote, is not a proposal.
        assert_eq!(
            choose(&mut game, seq, &id("Player 1"), team(&["Player 1"])),
            None
        );
        assert_eq!(
            choose(
                &mut game,
                seq,
                &id("Player 1"),
                Choice::Ballot(Ballot::Approve)
            ),
            None
        );
        assert!(game.underway.as_ref().unwrap().choices.is_empty());
        assert_eq!(choose(&mut game, seq, &id("Player 1"), proposal), last());
        // A second proposal does not count.
        assert_eq!(
            choose(
                &mut game,
                seq,
                &id("Player 1"),
                team(&["Player 3", "Player 4"])
            ),
            None
        );
        // Nor does one after the phase has ended.
        game.end(seq).unwrap();
        assert_eq!(
            choose(
                &mut game,
                seq,
                &id("Player 1"),
                team(&["Player 3", "Player 4"])
            ),
            None
        );
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
                    matches!(outcome, Entry::Event(Outcome::Assassinated { seq: s, target: Some(t), merlin: true }) if *s == seq && t == "Player 1")
                }
                _ => match i % 3 {
                    0 => {
                        matches!(outcome, Entry::Event(Outcome::Proposed { seq: s, team: Some(_) }) if *s == seq)
                    }
                    1 => {
                        matches!(outcome, Entry::Event(Outcome::Tallied { seq: s, tally: Tally { approved: true, .. } }) if *s == seq)
                    }
                    _ => {
                        matches!(outcome, Entry::Event(Outcome::Quested { seq: s, result: QuestResult { succeeded: true, .. } }) if *s == seq)
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
                Entry::Event(Outcome::Quested { result, .. }) => Some(result),
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
                .all(|entry| matches!(entry, Entry::Event(Outcome::Proposed { team: None, .. }))),
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
                Entry::Event(Outcome::Tallied { tally, .. }) => Some(tally),
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
            matches!(outcomes[0], Entry::Event(Outcome::Proposed { team: Some(team), .. }) if team == &["Player 1", "Player 2"]),
            "{outcomes:?}"
        );
        assert!(
            matches!(outcomes[1], Entry::Event(Outcome::Tallied { tally, .. }) if tally.ballots[&id("Player 3")] == Ballot::Approve),
            "{outcomes:?}"
        );
        assert!(
            matches!(outcomes[2], Entry::Event(Outcome::Quested { result, .. }) if result.sabotages == 0),
            "{outcomes:?}"
        );
        assert!(
            matches!(outcomes[9], Entry::Event(Outcome::Assassinated { target: Some(target), merlin: false, .. }) if target == "Player 3"),
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
                .any(|entry| matches!(entry, Entry::Event(Outcome::Assassinated { .. })))
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
            Entry::Event(Outcome::Assassinated {
                target: None,
                merlin: false,
                ..
            })
        )));
    }
}
