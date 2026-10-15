//! The random policy in Werewolf: a player who answers each announcement
//! of a phase it is [awake](super::awake) in at once with a selection of
//! one of its [candidates](super::candidates) made at random, and sleeps
//! through the rest. Its selection goes to the
//! [receivers](super::receivers) of a selection: by night the environment,
//! by day the environment and every other living player, since the votes
//! are public.

use super::Werewolf;

/// A player who selects at random.
pub type Player = crate::random::Player<Werewolf>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;
    use crate::game::Narrate;
    use crate::game::game;
    use crate::strategy::RANDOM;
    use crate::werewolf::report::Narrator;
    use crate::werewolf::tests::{
        Script, announced_to, at_once, between_the_deal_and_the_end, closed_before_it_ended,
        closing_to, configured, dealt, logged, mute, observed_by, observed_by_the_environment,
        one_wolf_against, play, quick, run, seen, selection, selections, tentative, utterance,
        village,
    };
    use crate::werewolf::{
        Actor, ENVIRONMENT, Entry, Message, Move, Outcome, Phase, PlayerId, Role, Rules, Team,
        View, candidates, to_everyone_but, to_the_environment,
    };
    use free_agent::ActorId;
    use std::collections::{HashMap, HashSet};
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    /// How long the log says the game took.
    fn game_took(logged: &[Entry]) -> Duration {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Event(Outcome::End { took, .. }) => Some(*took),
                _ => None,
            })
            .unwrap()
    }

    /// Who the log says survived.
    fn survivors(logged: &[Entry]) -> HashSet<PlayerId> {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Event(Outcome::End { survivors, .. }) => Some(survivors.clone()),
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_is_logged_from_the_deal_to_the_end() {
        let roles = village().roles;
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let episode = game(
            dealt(&roles, RANDOM),
            Rules::default(),
            configured(&roles, &Rules::default()),
            winner,
            |_| Player::init(),
            logger,
        );
        let (winner, logged, _) = run(episode, won, log).await.unwrap();

        // Everything between is an announcement, a selection, a phase's
        // closing or a player's close of it, logged as the action of its
        // sender and the observation of each of its receivers, or a night's
        // hunt.
        let doctors: HashSet<&PlayerId> = roles
            .iter()
            .filter(|(_, role)| **role == Role::Doctor)
            .map(|(player, _)| player)
            .collect();
        let between = between_the_deal_and_the_end(&logged, &Rules::default(), &roles, winner);
        // Whom each message, named by its sender and number, was logged as
        // sent to as an action, and who logged observing it.
        let mut actions: HashMap<(&str, u64), &HashSet<ActorId>> = HashMap::new();
        let mut observations: HashMap<(&str, u64), Vec<&str>> = HashMap::new();
        let mut announced = 0;
        let mut closings = 0;
        let mut closes = 0;
        // The players each phase was announced to, in the order announced.
        let mut phases: Vec<Vec<&str>> = Vec::new();
        let mut selected = 0;
        let mut observed = 0;
        let mut hunts = 0;
        for entry in between {
            if let Entry::Action {
                from,
                n,
                to,
                message: Message::Announce { seq, .. },
                seen,
                forgot,
            } = entry
            {
                assert_eq!(from, ENVIRONMENT);
                assert_eq!(to.len(), 1, "{to:?}");
                assert!(to.iter().all(|player| roles.contains_key(player)), "{to:?}");
                assert!(*seq >= 1, "{seq}");
                assert_eq!(*seen, None);
                assert_eq!(*forgot, None);
                assert_eq!(actions.insert((from, *n), to), None);
                announced += 1;
                let seq = usize::try_from(*seq).unwrap();
                phases.resize(phases.len().max(seq), Vec::new());
                phases[seq - 1].extend(to.iter().map(String::as_str));
            }
            if let Entry::Action {
                from,
                n,
                to,
                message: Message::Closing { .. },
                ..
            } = entry
            {
                assert_eq!(from, ENVIRONMENT);
                assert_eq!(actions.insert((from, *n), to), None);
                closings += 1;
            }
            if let Entry::Action {
                from,
                n,
                to,
                message: Message::Closed { from: closer, .. },
                ..
            } = entry
            {
                assert_eq!(from, closer);
                assert_eq!(*to, to_the_environment());
                assert_eq!(actions.insert((from, *n), to), None);
                closes += 1;
            }
            if let Entry::Action {
                from,
                n,
                to,
                message:
                    Message::Move(Move::Select {
                        seq,
                        from: said,
                        target,
                        ..
                    }),
                seen,
                forgot,
            } = entry
            {
                assert_eq!(from, said);
                assert!(roles.contains_key(from), "{from}");
                assert!(roles.contains_key(target), "{target}");
                // To the environment, and by day to the other players too.
                assert!(to.contains(ENVIRONMENT), "{to:?}");
                assert!(!to.contains(from), "{to:?}");
                assert!(
                    to.iter()
                        .all(|actor| actor == ENVIRONMENT || roles.contains_key(actor)),
                    "{to:?}"
                );
                assert!(*seq >= 1, "{seq}");
                assert_eq!(*seen, None);
                assert_eq!(*forgot, None);
                assert_eq!(actions.insert((from, *n), to), None);
                selected += 1;
            }
            if let Entry::Observation { by, from, n, seq } = entry {
                // The environment observes the players, and they it and
                // one another.
                let in_play = |actor: &str| actor == ENVIRONMENT || roles.contains_key(actor);
                assert!(in_play(by) && in_play(from) && by != from, "{entry:?}");
                assert!(*seq >= 1, "{seq}");
                observations.entry((from, *n)).or_default().push(by);
                observed += 1;
            }
            if let Entry::Event(Outcome::Hunted { seq, hunt }) = entry {
                assert!(*seq >= 1, "{seq}");
                assert!(roles.contains_key(&hunt.victim), "{hunt:?}");
                let protectors: HashSet<&PlayerId> = hunt.protectors.iter().collect();
                assert!(protectors.is_subset(&doctors), "{hunt:?}");
                hunts += 1;
            }
        }
        // Nothing else is logged between: no reminder to end a phase.
        assert_eq!(
            announced + closings + closes + selected + observed + hunts,
            between.len(),
            "{logged:?}"
        );
        // Each phase closed before it ended, with a closing and a close
        // for every player it was announced to.
        assert_eq!(closings, announced, "{logged:?}");
        assert_eq!(closes, announced, "{logged:?}");
        for (seq, players) in phases.iter().enumerate() {
            closed_before_it_ended(&logged, seq as u64 + 1, players);
        }
        // Every message is logged once as an action, and as an observation
        // with the same number at most once by each of its receivers, and
        // always by the environment, or by the player it announced to. A
        // player voted out, or stopped when the game ends, may be stopped
        // before it observes a vote.
        let mut sent: Vec<&(&str, u64)> = actions.keys().collect();
        let mut heard: Vec<&(&str, u64)> = observations.keys().collect();
        sent.sort_unstable();
        heard.sort_unstable();
        assert_eq!(sent, heard, "{logged:?}");
        for (message, by) in &observations {
            let to = actions[message];
            let distinct: HashSet<&str> = by.iter().copied().collect();
            assert_eq!(distinct.len(), by.len(), "{message:?}: {by:?}");
            assert!(by.iter().all(|by| to.contains(*by)), "{message:?}: {by:?}");
            let first = if message.0 == ENVIRONMENT {
                to.iter().next().unwrap().as_str()
            } else {
                ENVIRONMENT
            };
            assert!(by.contains(&first), "{message:?}: {by:?}");
        }
        // Each sender numbers its messages from 1, one up each.
        let mut numbers: HashMap<&str, Vec<u64>> = HashMap::new();
        for (from, n) in actions.keys() {
            numbers.entry(from).or_default().push(*n);
        }
        for (from, mut sent) in numbers {
            sent.sort_unstable();
            let counted: Vec<u64> = (1..=sent.len() as u64).collect();
            assert_eq!(sent, counted, "{from}");
        }
        // By the first night every player is announced the game, awake or
        // not, and the werewolves, who always select, hunt.
        assert!(announced >= roles.len(), "{logged:?}");
        assert!(hunts >= 1, "{logged:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_announced_a_phase_it_sleeps_through_observes_it_and_selects_nothing() {
        // Ann, a villager, is announced the first night, which the werewolf
        // sleeps through too, so that the night waits out its limit with no
        // selection from anyone.
        let roles = one_wolf_against(&["ann"]);
        let puppets = HashMap::from([("wolf", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().night_limit);
        assert!(selections(&logged).is_empty(), "{logged:?}");
        // Ann observed the announcement, the environment's first message,
        // and the night's closing, its third, and nothing else.
        assert_eq!(
            observed_by(&logged, "ann"),
            [(ENVIRONMENT, 1, 1), (ENVIRONMENT, 3, 1)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_observes_what_it_is_sent_in_the_phase_it_is_in_and_does_nothing_about_it() {
        // The werewolf kills ann the first night. By day 1 it talks, votes
        // against bob, and sends a tentative decision as if it were the
        // environment, and sends one of each tagged with the night just
        // ended too. Bob and cat, who choose at random, observe all six in
        // the day, and select once each as they would anyway.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let wolf: Script = Box::new(|me, seq, view| match view.phase {
            Phase::Night => at_once(vec![selection(seq, me, seq, &candidates(me, view)[0])]),
            Phase::Day => at_once(vec![
                utterance(seq, me, 10, "Boo."),
                utterance(seq - 1, me, 11, "Late."),
                selection(seq, me, 12, "bob"),
                selection(seq - 1, me, 13, "cat"),
                tentative(seq, 14, Some("bob")),
                tentative(seq - 1, 15, None),
            ]),
        });
        let puppets = HashMap::from([("wolf", Actor::puppet(wolf))]);
        let (_, logged, _) = play(roles, quick(), puppets).await.unwrap();
        for player in ["bob", "cat"] {
            // What the werewolf numbered 10 and up, which the other
            // players' votes are numbered below, and not the environment's
            // announcement or closing of the day. A tentative decision names
            // no sender, so the werewolf's are observed as the environment's.
            let own = [
                (ENVIRONMENT, announced_to(&logged, player, 2), 2),
                (ENVIRONMENT, closing_to(&logged, player, 2), 2),
            ];
            let heard: Vec<(&str, u64, u64)> = observed_by(&logged, player)
                .into_iter()
                .filter(|heard| heard.1 >= 10 && heard.2 == 2 && !own.contains(heard))
                .collect();
            assert_eq!(
                heard,
                [
                    ("wolf", 10, 2),
                    ("wolf", 11, 2),
                    ("wolf", 12, 2),
                    ("wolf", 13, 2),
                    (ENVIRONMENT, 14, 2),
                    (ENVIRONMENT, 15, 2),
                ],
                "{player}"
            );
            let selected = selections(&logged)
                .into_iter()
                .filter(|message| {
                    matches!(message, Message::Move(Move::Select { seq: 2, from, .. }) if from == player)
                })
                .count();
            assert_eq!(selected, 1, "{player}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_goes_to_the_environment_and_by_day_to_every_other_living_player_too() {
        // The werewolf kills one of three villagers the first night, so the
        // three left alive vote by day.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let (_, logged, _) = play(roles, quick(), HashMap::new()).await.unwrap();
        // What each player was last announced, as the log has it so far.
        let mut views: HashMap<&PlayerId, &View> = HashMap::new();
        let mut by_day = 0;
        for entry in &logged {
            match entry {
                Entry::Action {
                    to,
                    message: Message::Announce { view, .. },
                    ..
                } => {
                    for player in to {
                        views.insert(player, view);
                    }
                }
                Entry::Action {
                    from,
                    to,
                    message: Message::Move(Move::Select { .. }),
                    ..
                } => {
                    let view = views[from];
                    let expected = match view.phase {
                        Phase::Night => to_the_environment(),
                        Phase::Day => {
                            by_day += 1;
                            to_everyone_but(from, &view.alive)
                        }
                    };
                    assert_eq!(*to, expected, "{from}");
                }
                _ => {}
            }
        }
        assert!(by_day >= 3, "{logged:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn the_werewolves_win_on_reaching_parity() {
        // One werewolf against two: whoever it kills the first night, the
        // werewolf then equals the village.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, _, _) = play(roles, Rules::default(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
    }

    #[tokio::test(start_paused = true)]
    async fn a_phase_in_which_everyone_selects_ends_before_its_limit() {
        // The werewolf alone is awake the first night, and whoever it kills
        // brings the werewolves to parity.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, took) = play(roles, quick(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(selections(&logged).len(), 1);
        assert!(took < quick().night_limit, "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_phase_with_a_player_who_never_selects_ends_at_its_limit() {
        // The werewolf never selects, so nobody dies, and the werewolf is
        // at parity anyway.
        let roles = one_wolf_against(&["ann"]);
        let puppets = HashMap::from([("wolf", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert!(selections(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
        // The log says so too.
        assert_eq!(game_took(&logged), quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_with_an_old_number_is_logged_but_not_counted() {
        let roles = one_wolf_against(&["ann"]);
        let stale: Script = Box::new(|me, seq, _| at_once(vec![selection(seq - 1, me, 1, "ann")]));
        let puppets = HashMap::from([("wolf", Actor::puppet(stale))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(selections(&logged), [&selection(0, "wolf", 1, "ann")]);
        // The environment observes it in the phase it is in.
        assert_eq!(observed_by_the_environment(&logged), [("wolf", 1, 1)]);
        // Not counted: the night waits out its limit and kills nobody.
        assert_eq!(took, quick().night_limit);
        assert_eq!(survivors(&logged).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_that_arrives_in_a_later_phase_is_observed_in_it_and_goes_untold() {
        // The werewolf kills bob the first night, and by day sends a
        // selection for the night instead of a vote, which the environment
        // observes by day. Nobody votes, so the day waits out its limit,
        // and the second night the werewolf kills ann and wins.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let wolf: Script = Box::new(|me, seq, _| {
            at_once(match seq {
                1 => vec![selection(1, me, 1, "bob")],
                2 => vec![selection(1, me, 2, "ann")],
                seq => vec![selection(seq, me, seq, "ann")],
            })
        });
        let puppets = HashMap::from([
            ("wolf", Actor::puppet(wolf)),
            ("ann", mute()),
            ("cat", mute()),
        ]);
        let (winner, entries, _) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(
            observed_by_the_environment(&entries),
            [("wolf", 1, 1), ("wolf", 2, 2), ("wolf", 3, 3)]
        );
        let mut narrator = Narrator::default();
        let told: Vec<String> = entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .map(|line| line.to_string())
            .collect();
        assert!(
            !told.contains(&"wolf (werewolf) votes against ann (villager).".to_string()),
            "{told:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_selection_from_the_same_player_is_logged_but_not_counted() {
        // The werewolf selects two villagers in turn, and the doctor never
        // selects, so the night waits out its limit with both selections
        // in. Only the first kills.
        let mut roles = one_wolf_against(&["ann"]);
        roles.insert("doctor".to_string(), Role::Doctor);
        let twice: Script = Box::new(|me, seq, _| {
            at_once(vec![
                selection(seq, me, 1, "ann"),
                selection(seq, me, 2, "doctor"),
            ])
        });
        let puppets = HashMap::from([("wolf", Actor::puppet(twice)), ("doctor", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        let wolf = "wolf".to_string();
        assert_eq!(
            selections(&logged),
            [
                &selection(1, &wolf, 1, "ann"),
                &selection(1, &wolf, 2, "doctor")
            ]
        );
        assert_eq!(
            observed_by_the_environment(&logged),
            [("wolf", 1, 1), ("wolf", 2, 1)]
        );
        assert_eq!(took, quick().night_limit);
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(
            survivors(&logged),
            HashSet::from([wolf, "doctor".to_string()])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_end_of_a_phase_that_already_ended_is_ignored() {
        // Night 1 ends at once when the werewolf kills bob, and its timer
        // fires during day 1, which waits out its longer limit for ann,
        // who never votes. Were the night's timer to end the day, the day
        // would be over at the night's limit instead.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let rules = Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(100),
            ..Rules::default()
        };
        let wolf: Script = Box::new(|me, seq, view| {
            let target = match view.phase {
                Phase::Night => "bob",
                Phase::Day => "ann",
            };
            at_once(vec![selection(seq, me, seq, target)])
        });
        let cat: Script = Box::new(|me, seq, _| at_once(vec![selection(seq, me, 1, "ann")]));
        let puppets = HashMap::from([
            ("wolf", Actor::puppet(wolf)),
            ("ann", mute()),
            ("cat", Actor::puppet(cat)),
        ]);
        let (winner, logged, took) = play(roles, rules, puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(100));
        let phases: HashSet<u64> = logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    message: Message::Announce { seq, .. },
                    ..
                } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(phases, HashSet::from([1, 2]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_message_that_is_not_a_selection_is_neither_counted_nor_observed() {
        let roles = one_wolf_against(&["ann"]);
        let meddler: Script = Box::new(|_, seq, _| at_once(vec![Message::EndPhase { seq }]));
        let puppets = HashMap::from([("wolf", Actor::puppet(meddler))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert!(selections(&logged).is_empty());
        assert!(observed_by_the_environment(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_with_nobody_to_select_says_nothing() {
        // Two werewolves know each other, so neither has anyone to select.
        let roles = HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
        ]);
        let (winner, logged, took) = play(roles, quick(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert!(selections(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[test]
    fn a_player_chooses_one_of_its_candidates() {
        let me = "wolf1".to_string();
        let view = seen(
            &["wolf1", "wolf2", "ann", "bob"],
            &[("wolf1", Role::Werewolf), ("wolf2", Role::Werewolf)],
        );
        let allowed = candidates(&me, &view);
        assert_eq!(allowed.len(), 2);

        for _ in 0..50 {
            let chosen = Werewolf::pick(&me, &view, 1, 1);
            assert!(
                matches!(&chosen, Some(Move::Select { target, .. }) if allowed.contains(target)),
                "{chosen:?}"
            );
        }
    }

    #[test]
    fn a_player_without_candidates_chooses_no_one() {
        let view = seen(&["ann"], &[("ann", Role::Villager)]);
        assert_eq!(Werewolf::pick(&"ann".to_string(), &view, 1, 1), None);
    }
}
