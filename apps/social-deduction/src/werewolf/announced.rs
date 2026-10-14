//! Werewolf's rules for the [announcing environment](crate::announced):
//! each phase is announced to every living player, awake in it or not. A
//! player announced a phase it sleeps through learns the phase has begun,
//! and its selections do not count.
//!
//! A night, and a day without a ratify limit, take each awake player's
//! first selection and end once everyone awake has selected or at the
//! limit, whichever comes first. A day with a ratify limit takes each
//! player's latest selection as its vote, unless the player has retracted
//! it since with a [`Move::Retract`] and is undecided again, and keeps the
//! votes: whenever they change the tentative decision, the player they
//! select or nobody, every living player is sent a [`Move::Tentative`], and
//! whenever they change it to a player a [`Move::Ratify`] timer starts
//! again. The day ends when the latest timer fires, and the selected player
//! is voted out, or at the day limit, whatever is standing, and nobody is.
//!
//! Each night's hunt is logged as [`Outcome::Hunted`], and each day's vote
//! as [`Outcome::Voted`]. Whoever dies is stopped at once.

use super::{
    Choices, Ending, Hunt, Message, Move, Outcome, Phase, PlayerId, Role, State, Werewolf,
};
use crate::announced::{Announced, Awaited, Core, Counted, Opening, Over, Resolved};
use crate::report::in_order;
use std::collections::HashMap;
use std::time::Duration;

/// The phase underway: whom the environment waits on and what they have
/// selected.
pub struct Underway {
    /// The players awake in the phase, and each one's selection: its
    /// latest in a day with a ratify limit, unless retracted since, its
    /// first otherwise.
    awake: Awaited<PlayerId>,
    /// In a day with a ratify limit, the tentative decision standing: the
    /// player the votes select, if they select anyone.
    tentative: Option<PlayerId>,
    /// How many times the tentative decision has changed, which numbers
    /// the ratify timers so that a stale one is known.
    changes: u64,
    /// The tentative decision has stood for the ratify limit: the day ends
    /// with it, and nothing more counts.
    ratified: bool,
}

impl Announced for Werewolf {
    type State = State;
    type Underway = Underway;

    /// The first night, with everyone alive.
    fn state(roles: HashMap<PlayerId, Role>) -> State {
        State::new(roles)
    }

    /// Every living player, awaiting those awake in the phase.
    fn open(core: &Core<Self>) -> anyhow::Result<Opening<Self>> {
        let state = &core.state;
        let limit = match state.phase {
            Phase::Night => core.rules.night_limit,
            Phase::Day => core.rules.day_limit,
        };
        let views = in_order(&state.alive)
            .into_iter()
            .map(|player| Ok((player.clone(), state.view(player.clone())?)))
            .collect::<anyhow::Result<_>>()?;
        Ok(Opening {
            views,
            limit,
            underway: Underway {
                awake: Awaited::new(state.awake()),
                tentative: None,
                changes: 0,
                ratified: false,
            },
        })
    }

    /// An awake player's first selection; or, in a day with a ratify
    /// limit, an awake player's every selection, a retraction of its vote
    /// standing, and the latest ratify timer, until it fires.
    fn counts(
        core: &mut Core<Self>,
        underway: &mut Underway,
        played: &Move,
    ) -> Option<Counted<Self>> {
        match (played, ratifying(core)) {
            (Move::Select { from, target, .. }, None) => underway.awake.first(from, target.clone()),
            (Move::Select { from, target, .. }, Some(ratify))
                if underway.awake.players.contains(from) && !underway.ratified =>
            {
                underway.awake.choices.insert(from.clone(), target.clone());
                Some(decide(core, underway, ratify))
            }
            (Move::Retract { from, .. }, Some(ratify)) if !underway.ratified => {
                underway.awake.choices.remove(from)?;
                Some(decide(core, underway, ratify))
            }
            (Move::Ratify { change, .. }, Some(_)) if *change == underway.changes => {
                underway.ratified = true;
                Some(Counted {
                    last: true,
                    ..Counted::default()
                })
            }
            _ => None,
        }
    }

    /// The night's hunt or the day's vote, from the selections standing
    /// under the phase's vote. At the limit of a day whose vote is ratified
    /// nothing stands, whatever was. The game is over once a team has won,
    /// the survivors stopped.
    fn resolve(core: &mut Core<Self>, underway: Underway) -> Resolved<Self> {
        let Underway {
            awake, ratified, ..
        } = underway;
        let everyone = awake.all();
        let (ballots, by) = match ratifying(core) {
            _ if ratified => (awake.choices, Ending::Ratified),
            Some(_) => (Choices::new(), Ending::DayLimit),
            None if everyone => (awake.choices, Ending::AllSelected),
            None => (awake.choices, Ending::DayLimit),
        };
        let seq = core.seq;
        let (outcome, out) = match core.state.phase {
            Phase::Night => {
                let hunt = core
                    .state
                    .resolve_night(&ballots, core.rules.night_vote.as_ref());
                let dead = hunt.as_ref().and_then(Hunt::dead).cloned();
                (hunt.map(|hunt| Outcome::Hunted { seq, hunt }), dead)
            }
            Phase::Day => {
                let out = core
                    .state
                    .resolve_day(&ballots, core.rules.day_vote.as_ref());
                let voted = Outcome::Voted {
                    seq,
                    out: out.clone(),
                    by,
                };
                (Some(voted), out)
            }
        };
        let over = match core.state.winner() {
            Some(winner) => Some(Over {
                winner,
                stopped: core.state.alive.iter().cloned().collect(),
                end: Outcome::End {
                    winner,
                    days: core.state.days(),
                    survivors: core.state.alive.clone(),
                    took: core.took(),
                },
            }),
            None => {
                core.state.next();
                None
            }
        };
        Resolved { outcome, out, over }
    }
}

/// The ratify limit, if the current phase is a day with one above zero. A
/// day whose limit is zero is not ratified, so no timer of zero ever
/// starts.
fn ratifying(core: &Core<Werewolf>) -> Option<Duration> {
    match core.state.phase {
        Phase::Day if core.rules.ratified() => Some(core.rules.ratify_limit),
        _ => None,
    }
}

/// Work out the day's tentative decision again from the votes standing,
/// whose ratify limit is `ratify`. Returns what the environment does about
/// it: nothing if the decision is unchanged; otherwise tell every living
/// player, and restart the ratify timer if it selects a player.
fn decide(
    core: &mut Core<Werewolf>,
    underway: &mut Underway,
    ratify: Duration,
) -> Counted<Werewolf> {
    let selected = core.rules.day_vote.elect(&underway.awake.choices);
    if selected == underway.tentative {
        return Counted::default();
    }
    underway.changes += 1;
    let message = Message::Move(Move::Tentative {
        seq: core.seq,
        n: core.number(),
        target: selected.clone(),
        ratify,
    });
    let tell = Some((message, core.state.alive.clone()));
    let remind = selected.is_some().then(|| {
        let reminder = Message::Move(Move::Ratify {
            seq: core.seq,
            change: underway.changes,
        });
        (ratify, reminder)
    });
    underway.tentative = selected;
    Counted {
        last: false,
        tell,
        remind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announced::Opened;
    use crate::announced::tests::{counted, last};
    use crate::seat::id;
    use crate::werewolf::candidates;
    use crate::werewolf::tests::{
        self, Script, at_once, mute, observed_by_the_environment, one_wolf_against, play, quick,
        retraction, selection, village,
    };
    use crate::werewolf::{Actor, Entry, Rules, Seat, Team};
    use std::collections::HashSet;
    use tokio::sync::oneshot;

    type Game = Core<Werewolf>;

    /// The selections that count in the phase underway.
    fn selections(game: &Game) -> &Choices {
        &game.underway.as_ref().unwrap().awake.choices
    }

    /// A game of `roles` under `rules`, with its first night open.
    fn opened_under(roles: HashMap<PlayerId, Role>, rules: Rules) -> (Game, Opened<Werewolf>) {
        let (winner, _) = oneshot::channel();
        let mut game = Game::new(roles, rules, winner);
        let night = game.open().unwrap();
        (game, night)
    }

    /// The village, with its first night open.
    fn opened() -> (Game, Opened<Werewolf>) {
        opened_under(village().roles, Rules::default())
    }

    /// Whom `opened` announces to, in name order, and the numbers of its
    /// announcements, in order.
    fn announced(opened: &Opened<Werewolf>) -> (Vec<&PlayerId>, Vec<u64>) {
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
        tests::sorted(game.underway.as_ref().unwrap().awake.players.clone())
    }

    /// Count `from`'s selection of `target` in the phase numbered `seq`.
    fn select(
        game: &mut Game,
        seq: u64,
        from: &PlayerId,
        target: &PlayerId,
    ) -> Option<Counted<Werewolf>> {
        game.count(&Move::Select {
            seq,
            from: from.clone(),
            n: 1,
            target: target.clone(),
        })
    }

    /// Count `from`'s retraction in the phase numbered `seq`.
    fn retract(game: &mut Game, seq: u64, from: &PlayerId) -> Option<Counted<Werewolf>> {
        game.count(&Move::Retract {
            seq,
            from: from.clone(),
            n: 1,
        })
    }

    /// Fire the ratify timer set at the `change`th change of the tentative
    /// decision of the day numbered `seq`: the day as it ended, or nothing
    /// if the timer was stale.
    fn ratify(game: &mut Game, seq: u64, change: u64) -> Option<Resolved<Werewolf>> {
        game.count(&Move::Ratify { seq, change })?;
        game.end(seq)
    }

    /// Who won, if the phase ended the game.
    fn winner(ended: &Resolved<Werewolf>) -> Option<Team> {
        ended.over.as_ref().map(|over| over.winner)
    }

    /// The day numbered `seq`'s vote: `out` voted out, if anyone, the day
    /// ended `by`.
    fn vote(seq: u64, out: Option<&str>, by: Ending) -> Option<Outcome> {
        Some(Outcome::Voted {
            seq,
            out: out.map(id),
            by,
        })
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

    #[test]
    fn the_last_selection_of_a_phase_ends_it() {
        let (mut game, Opened { seq, .. }) = opened();
        assert_eq!(
            select(&mut game, seq, &id("wolf1"), &id("villager")),
            counted()
        );
        assert_eq!(
            select(&mut game, seq, &id("wolf2"), &id("villager")),
            counted()
        );
        assert_eq!(select(&mut game, seq, &id("seer"), &id("wolf1")), last());
    }

    #[test]
    fn a_selection_for_another_phase_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        assert_eq!(
            select(&mut game, seq - 1, &id("wolf1"), &id("villager")),
            None
        );
        assert_eq!(
            select(&mut game, seq + 1, &id("wolf1"), &id("villager")),
            None
        );
        assert!(selections(&game).is_empty());
    }

    #[test]
    fn a_selection_by_a_player_who_is_not_awake_does_not_count() {
        // The villager is announced the night and sleeps through it: its
        // selection is not kept, and the night still waits on the awake.
        let (mut game, Opened { seq, .. }) = opened();
        assert_eq!(select(&mut game, seq, &id("villager"), &id("wolf1")), None);
        assert!(selections(&game).is_empty());
        assert_eq!(
            select(&mut game, seq, &id("wolf1"), &id("villager")),
            counted()
        );
        assert_eq!(
            select(&mut game, seq, &id("wolf2"), &id("villager")),
            counted()
        );
        assert_eq!(select(&mut game, seq, &id("seer"), &id("wolf1")), last());
    }

    #[test]
    fn in_a_ratified_day_a_selection_by_a_player_who_is_not_awake_neither_counts_nor_restarts_the_timer()
     {
        // Ann dies the first night, so by day she is not awake; her vote
        // selects nobody, tells nobody, and sets no timer.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, ratified());
        select(&mut game, seq, &id("wolf"), &id("ann"));
        game.end(seq);
        let day = game.open().unwrap().seq;
        assert_eq!(select(&mut game, day, &id("ann"), &id("wolf")), None);
        assert!(selections(&game).is_empty());
        assert_eq!(game.underway.as_ref().unwrap().changes, 0);
        assert_eq!(
            select(&mut game, day, &id("bob"), &id("wolf")),
            Some(Counted {
                last: false,
                tell: tentative_to(day, 8, Some("wolf"), &["wolf", "bob", "cat"]),
                remind: timer(day, 1),
            })
        );
    }

    #[test]
    fn only_a_players_first_selection_in_a_phase_counts() {
        let (mut game, Opened { seq, .. }) = opened();
        select(&mut game, seq, &id("wolf1"), &id("villager"));
        select(&mut game, seq, &id("wolf2"), &id("villager"));
        assert_eq!(select(&mut game, seq, &id("wolf1"), &id("seer")), None);
        assert_eq!(selections(&game)[&id("wolf1")], id("villager"));
    }

    #[test]
    fn a_selection_after_a_phase_has_ended_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        game.end(seq).unwrap();
        assert_eq!(select(&mut game, seq, &id("wolf1"), &id("villager")), None);
    }

    #[test]
    fn a_phase_is_resolved_from_its_selections_under_its_vote() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, Rules::default());
        select(&mut game, seq, &id("wolf"), &id("ann"));
        let night = game.end(seq).unwrap();
        let hunt = Hunt {
            victim: id("ann"),
            protectors: HashSet::new(),
        };
        assert_eq!(night.outcome, Some(Outcome::Hunted { seq, hunt }));
        assert_eq!(night.out.as_ref(), Some(&id("ann")));
        assert_eq!(winner(&night), None);
        // The game goes on, so the next phase is the day.
        let day = game.open().unwrap();
        assert_eq!(day.seq, 2);
        assert_eq!(day.limit, Rules::default().day_limit);
        select(&mut game, day.seq, &id("wolf"), &id("bob"));
        select(&mut game, day.seq, &id("bob"), &id("wolf"));
        let tied = game.end(day.seq).unwrap();
        assert_eq!(tied.outcome, vote(day.seq, None, Ending::DayLimit));
        assert_eq!(tied.out.as_ref(), None);
        assert_eq!(winner(&tied), None);
    }

    #[test]
    fn a_day_with_a_ratify_limit_of_zero_ends_by_everyone_selecting_or_at_its_limit() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, Rules::default());
        assert_eq!(game.rules.ratify_limit, Duration::ZERO);
        // A night's hunt is recorded only when the werewolves elected
        // someone; a day's vote always.
        assert_eq!(game.end(seq).unwrap().outcome, None);
        let day = game.open().unwrap().seq;
        // Each first vote counts, and tells nobody of a tentative decision
        // and sets no timer; a second does not count.
        for voter in ["wolf", "ann", "bob"] {
            assert_eq!(select(&mut game, day, &id(voter), &id("ann")), counted());
        }
        assert_eq!(select(&mut game, day, &id("wolf"), &id("bob")), None);
        assert_eq!(select(&mut game, day, &id("cat"), &id("ann")), last());
        let ended = game.end(day).unwrap();
        assert_eq!(ended.outcome, vote(day, Some("ann"), Ending::AllSelected));
    }

    #[test]
    fn a_phase_that_decides_the_game_is_the_last() {
        let (mut game, Opened { seq, .. }) = opened();
        game.state.alive.remove("wolf2");
        select(&mut game, seq, &id("wolf1"), &id("villager"));
        let night = game.end(seq).unwrap();
        assert_eq!(night.out.as_ref(), Some(&id("villager")));
        assert_eq!(winner(&night), Some(Team::Werewolves));
        assert_eq!(game.state.days(), 0);
    }

    /// Rules under which a day's votes stand after twenty seconds, as the
    /// shared [`tests::tentative`] has them.
    fn ratified() -> Rules {
        Rules {
            ratify_limit: RATIFY,
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

    /// The ratify timer set at the `change`th change of the tentative
    /// decision of the day numbered `seq`.
    fn timer(seq: u64, change: u64) -> Option<(Duration, Message)> {
        Some((RATIFY, Message::Move(Move::Ratify { seq, change })))
    }

    #[test]
    fn in_a_ratified_day_the_first_selection_selects_a_player_and_starts_the_timer() {
        let (mut game, day) = ratifying_day();
        // The environment has announced the night and the day to four
        // players each.
        assert_eq!(game.n, 8);
        assert_eq!(
            select(&mut game, day, &id("ann"), &id("wolf")),
            Some(Counted {
                last: false,
                tell: tentative(day, 9, Some("wolf")),
                remind: timer(day, 1),
            })
        );
    }

    #[test]
    fn a_selection_that_leaves_the_decision_unchanged_sets_no_timer_and_tells_nobody() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        // A vote for the selected player, the same vote cast again, and a
        // vote moved between other players all leave the werewolf selected.
        assert_eq!(select(&mut game, day, &id("bob"), &id("wolf")), counted());
        assert_eq!(select(&mut game, day, &id("bob"), &id("wolf")), counted());
        assert_eq!(select(&mut game, day, &id("cat"), &id("ann")), counted());
        assert_eq!(select(&mut game, day, &id("cat"), &id("bob")), counted());
        assert_eq!(game.underway.as_ref().unwrap().changes, 1);
    }

    #[test]
    fn a_selection_that_leaves_the_decision_unchanged_leaves_the_running_timer_to_fire() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        select(&mut game, day, &id("bob"), &id("wolf"));
        select(&mut game, day, &id("bob"), &id("wolf"));
        let ended = ratify(&mut game, day, 1).unwrap();
        assert_eq!(ended.outcome, vote(day, Some("wolf"), Ending::Ratified));
    }

    #[test]
    fn a_change_from_nobody_or_another_player_sets_a_fresh_timer() {
        let (mut game, day) = ratifying_day();
        assert_eq!(
            select(&mut game, day, &id("ann"), &id("cat")),
            Some(Counted {
                last: false,
                tell: tentative(day, 9, Some("cat")),
                remind: timer(day, 1),
            })
        );
        assert_eq!(
            select(&mut game, day, &id("ann"), &id("wolf")),
            Some(Counted {
                last: false,
                tell: tentative(day, 10, Some("wolf")),
                remind: timer(day, 2),
            })
        );
        // The timer set when cat was selected is stale.
        assert!(ratify(&mut game, day, 1).is_none());
        assert!(ratify(&mut game, day, 2).is_some());
    }

    #[test]
    fn a_tie_selects_nobody_and_sets_no_timer() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        assert_eq!(
            select(&mut game, day, &id("wolf"), &id("ann")),
            Some(Counted {
                last: false,
                tell: tentative(day, 10, None),
                remind: None,
            })
        );
        // The timer set before the tie is stale.
        assert!(ratify(&mut game, day, 1).is_none());
        // A vote that breaks the tie selects again.
        assert_eq!(
            select(&mut game, day, &id("bob"), &id("wolf")),
            Some(Counted {
                last: false,
                tell: tentative(day, 11, Some("wolf")),
                remind: timer(day, 3),
            })
        );
        assert!(ratify(&mut game, day, 3).is_some());
    }

    #[test]
    fn in_a_ratified_day_a_players_latest_selection_is_its_vote() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        assert_eq!(
            select(&mut game, day, &id("ann"), &id("bob")),
            Some(Counted {
                last: false,
                tell: tentative(day, 10, Some("bob")),
                remind: timer(day, 2),
            })
        );
        assert_eq!(selections(&game)[&id("ann")], id("bob"));
        // Everyone selecting does not end the day.
        for voter in ["wolf", "bob", "cat"] {
            assert!(!select(&mut game, day, &id(voter), &id("ann")).unwrap().last);
        }
    }

    #[test]
    fn by_night_the_first_selection_still_counts_whatever_the_ratify_limit() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, ratified());
        assert_eq!(select(&mut game, seq, &id("wolf"), &id("ann")), last());
        assert_eq!(select(&mut game, seq, &id("wolf"), &id("bob")), None);
        assert_eq!(selections(&game)[&id("wolf")], id("ann"));
    }

    #[test]
    fn the_ratify_timer_votes_out_the_selected_player_unless_it_is_stale() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("bob"));
        select(&mut game, day, &id("ann"), &id("wolf"));
        // The first selection's timer is stale: the decision has changed
        // since. So is one for another day.
        assert!(ratify(&mut game, day, 1).is_none());
        assert!(ratify(&mut game, day + 1, 2).is_none());
        let ended = ratify(&mut game, day, 2).unwrap();
        assert_eq!(ended.outcome, vote(day, Some("wolf"), Ending::Ratified));
        assert_eq!(winner(&ended), Some(Team::Villagers));
        // The day has ended, so its timer is stale now too.
        assert!(ratify(&mut game, day, 2).is_none());
    }

    #[test]
    fn the_day_limit_votes_nobody_out_whatever_is_standing() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        select(&mut game, day, &id("bob"), &id("wolf"));
        let ended = game.end(day).unwrap();
        assert_eq!(ended.outcome, vote(day, None, Ending::DayLimit));
        assert_eq!(winner(&ended), None);
        assert!(game.state.alive.contains("wolf"));
    }

    #[test]
    fn a_retraction_by_the_only_voter_for_the_selected_player_selects_nobody() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        assert_eq!(
            retract(&mut game, day, &id("ann")),
            Some(Counted {
                last: false,
                tell: tentative(day, 10, None),
                remind: None,
            })
        );
        assert!(selections(&game).is_empty());
        // The timer set when the werewolf was selected is stale.
        assert!(ratify(&mut game, day, 1).is_none());
    }

    #[test]
    fn a_retraction_that_breaks_a_tie_selects_the_remaining_leader_and_sets_a_fresh_timer() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        select(&mut game, day, &id("bob"), &id("cat"));
        assert_eq!(
            retract(&mut game, day, &id("bob")),
            Some(Counted {
                last: false,
                tell: tentative(day, 11, Some("wolf")),
                remind: timer(day, 3),
            })
        );
        assert!(ratify(&mut game, day, 1).is_none());
        assert!(ratify(&mut game, day, 3).is_some());
    }

    #[test]
    fn a_retraction_that_leaves_the_decision_unchanged_tells_nobody_and_leaves_the_timer_running() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        select(&mut game, day, &id("bob"), &id("wolf"));
        assert_eq!(retract(&mut game, day, &id("bob")), counted());
        assert_eq!(game.underway.as_ref().unwrap().changes, 1);
        let ended = ratify(&mut game, day, 1).unwrap();
        assert_eq!(ended.outcome, vote(day, Some("wolf"), Ending::Ratified));
    }

    #[test]
    fn a_retraction_with_no_vote_standing_by_a_player_not_awake_or_for_another_phase_does_not_count()
     {
        // Ann dies the first night, so by day she is not awake.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles, ratified());
        select(&mut game, seq, &id("wolf"), &id("ann"));
        game.end(seq);
        let day = game.open().unwrap().seq;
        assert_eq!(retract(&mut game, day, &id("bob")), None);
        select(&mut game, day, &id("bob"), &id("wolf"));
        assert_eq!(retract(&mut game, day, &id("ann")), None);
        assert_eq!(retract(&mut game, day - 1, &id("bob")), None);
        assert_eq!(retract(&mut game, day + 1, &id("bob")), None);
        assert_eq!(selections(&game)[&id("bob")], id("wolf"));
        assert_eq!(game.underway.as_ref().unwrap().changes, 1);
        // Nor does one after the day has ended.
        game.end(day);
        assert_eq!(retract(&mut game, day, &id("bob")), None);
    }

    #[test]
    fn a_player_that_retracts_and_votes_again_is_counted_with_its_new_vote() {
        let (mut game, day) = ratifying_day();
        select(&mut game, day, &id("ann"), &id("wolf"));
        retract(&mut game, day, &id("ann"));
        assert_eq!(
            select(&mut game, day, &id("ann"), &id("bob")),
            Some(Counted {
                last: false,
                tell: tentative(day, 11, Some("bob")),
                remind: timer(day, 3),
            })
        );
        assert_eq!(selections(&game)[&id("ann")], id("bob"));
    }

    #[test]
    fn by_night_and_in_a_day_without_a_ratify_limit_a_retraction_does_not_count() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (mut game, Opened { seq, .. }) = opened_under(roles.clone(), ratified());
        select(&mut game, seq, &id("wolf"), &id("ann"));
        assert_eq!(retract(&mut game, seq, &id("wolf")), None);
        assert_eq!(selections(&game)[&id("wolf")], id("ann"));
        let (mut game, Opened { seq, .. }) = opened_under(roles, Rules::default());
        game.end(seq);
        let day = game.open().unwrap().seq;
        select(&mut game, day, &id("bob"), &id("wolf"));
        assert_eq!(retract(&mut game, day, &id("bob")), None);
        assert_eq!(selections(&game)[&id("bob")], id("wolf"));
    }

    /// Rules under which a day's votes stand after five seconds, in a game
    /// of short phases.
    fn ratifying_quickly() -> Rules {
        Rules {
            ratify_limit: Duration::from_secs(5),
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

    /// A villager, announced the day alone, who casts each of `ballots` in
    /// turn, the first `first` after the day is announced and each of the
    /// rest `every` after the one before: a vote against the player it
    /// names, or a retraction where it names nobody.
    fn voter(first: Duration, ballots: &'static [Option<&'static str>], every: Duration) -> Seat {
        Actor::puppet(Box::new(move |me, seq, _| {
            ballots
                .iter()
                .enumerate()
                .map(|(i, ballot)| {
                    let after = first + every * u32::try_from(i).unwrap();
                    let n = i as u64 + 1;
                    let message = match ballot {
                        Some(target) => selection(seq, me, n, target),
                        None => retraction(seq, me, n),
                    };
                    (after, message)
                })
                .collect()
        }))
    }

    /// How each day ended, in order.
    fn voted(logged: &[Entry]) -> Vec<(u64, Option<&str>, Ending)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Event(Outcome::Voted { seq, out, by }) => Some((*seq, out.as_deref(), *by)),
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
                    message: Message::Move(Move::Tentative { target, ratify, .. }),
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
    async fn a_day_ends_one_ratify_limit_after_the_selection_and_votes_out_the_selected() {
        // Bob votes against the werewolf at once and cat piles on three
        // seconds later, which leaves the decision as it was, so the day
        // ends five seconds in, one ratify limit after bob's vote.
        let bob = voter(Duration::ZERO, &[Some("wolf")], Duration::ZERO);
        let cat = voter(Duration::from_secs(3), &[Some("wolf")], Duration::ZERO);
        let (winner, logged, took) = play_a_day(bob, cat).await;
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(5));
        assert_eq!(voted(&logged), [(2, Some("wolf"), Ending::Ratified)]);
        assert_eq!(
            tentatives(&logged),
            [(Some("wolf"), vec!["bob", "cat", "wolf"])]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_of_decision_restarts_the_timer_and_the_latest_vote_counts() {
        // Bob votes against cat at once, then changes to the werewolf three
        // seconds in: the day ends eight seconds in, and the werewolf is
        // out, not cat.
        let bob = voter(
            Duration::ZERO,
            &[Some("cat"), Some("wolf")],
            Duration::from_secs(3),
        );
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
        let bob = voter(Duration::ZERO, &[Some("cat")], Duration::ZERO);
        let cat = voter(Duration::from_secs(2), &[Some("bob")], Duration::ZERO);
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
    async fn voting_the_same_way_again_does_not_hold_the_day_open() {
        // Bob votes against the werewolf every three seconds, which leaves
        // the decision as it was, so the day ends five seconds in with the
        // werewolf voted out.
        let bob = voter(
            Duration::ZERO,
            &[Some("wolf"), Some("wolf"), Some("wolf"), Some("wolf")],
            Duration::from_secs(3),
        );
        let (winner, logged, took) = play_a_day(bob, mute()).await;
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(5));
        assert_eq!(voted(&logged), [(2, Some("wolf"), Ending::Ratified)]);
        assert_eq!(
            tentatives(&logged),
            [(Some("wolf"), vec!["bob", "cat", "wolf"])]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_whose_decision_keeps_changing_ends_at_its_limit_with_nobody_voted_out() {
        // Bob moves his vote between cat and the werewolf every three
        // seconds, changing the decision and restarting the five-second
        // timer each time, so the day runs out at ten with nobody voted
        // out. The timers still pending fire during the next night and are
        // ignored.
        let bob = voter(
            Duration::ZERO,
            &[Some("cat"), Some("wolf"), Some("cat"), Some("wolf")],
            Duration::from_secs(3),
        );
        let (winner, logged, took) = play_a_day(bob, mute()).await;
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(10));
        assert_eq!(voted(&logged), [(2, None, Ending::DayLimit)]);
        let living = vec!["bob", "cat", "wolf"];
        assert_eq!(
            tentatives(&logged),
            [
                (Some("cat"), living.clone()),
                (Some("wolf"), living.clone()),
                (Some("cat"), living.clone()),
                (Some("wolf"), living)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_whose_only_voter_retracts_runs_out_with_nobody_voted_out() {
        // Bob votes against the werewolf at once and retracts two seconds
        // in, which stops the timer, so the day runs out at ten with nobody
        // voted out. The werewolf then kills bob or cat and wins.
        let bob = voter(
            Duration::ZERO,
            &[Some("wolf"), None],
            Duration::from_secs(2),
        );
        let (winner, logged, took) = play_a_day(bob, mute()).await;
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(10));
        assert_eq!(voted(&logged), [(2, None, Ending::DayLimit)]);
        let living = vec!["bob", "cat", "wolf"];
        assert_eq!(
            tentatives(&logged),
            [(Some("wolf"), living.clone()), (None, living)]
        );
        // The environment observes the retraction as it does the vote.
        let bobs: Vec<_> = observed_by_the_environment(&logged)
            .into_iter()
            .filter(|(from, _, _)| *from == "bob")
            .collect();
        assert_eq!(bobs, [("bob", 1, 2), ("bob", 2, 2)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_who_retracts_and_votes_again_restarts_the_timer() {
        // Bob votes against the werewolf at once, retracts two seconds in,
        // and votes against it again at four, so the day ends at nine with
        // the werewolf voted out.
        let bob = voter(
            Duration::ZERO,
            &[Some("wolf"), None, Some("wolf")],
            Duration::from_secs(2),
        );
        let (winner, logged, took) = play_a_day(bob, mute()).await;
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(9));
        assert_eq!(voted(&logged), [(2, Some("wolf"), Ending::Ratified)]);
        let living = vec!["bob", "cat", "wolf"];
        assert_eq!(
            tentatives(&logged),
            [
                (Some("wolf"), living.clone()),
                (None, living.clone()),
                (Some("wolf"), living)
            ]
        );
    }
}
