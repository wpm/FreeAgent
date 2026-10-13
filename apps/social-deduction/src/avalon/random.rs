//! The random policy: a player who answers each announcement of a phase
//! that awaits it at once with a choice made uniformly at random among the
//! legal ones, observes the rest, and never speaks. A player logs an
//! [`Entry::Observation`] for each message it receives, in the latest
//! phase it was announced, and an [`Entry::Action`] for each choice it
//! sends, numbering its choices from 1; no model made them, so `seen` is
//! `None`.

use super::{
    Actor, Ballot, Choice, Deed, Entry, Message, Phase, PlayerId, Seat, Team, View, act, awaited,
    candidates, observe, receivers,
};
use crate::report::seat_order;
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context};
use rand::seq::IndexedRandom;

/// A player who answers each announcement of a phase that [awaits](awaited)
/// it at once, with a choice drawn uniformly from the legal ones, and does
/// nothing about one that does not. Its choice goes to the [`receivers`]
/// of a choice: the environment alone. It has no think loop.
pub struct Player {
    context: Context<Message, Entry>,
    /// The number of the last choice it sent.
    n: u64,
    /// The number of the latest phase it was announced, and 0 before any.
    seq: u64,
}

impl Player {
    /// Builds a player once the episode has made its context. It has no
    /// think loop.
    pub fn init() -> Seat {
        let behavior: Builder<Actor> = Box::new(|context| {
            Actor::Random(Player {
                context,
                n: 0,
                seq: 0,
            })
        });
        (behavior, None)
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log a message from another actor, which is any numbered one, as
    /// observed in the latest phase announced. Announced a phase that
    /// awaits it, choose at once; otherwise, or with nothing to choose, say
    /// nothing. Anything else, another player's utterance above all, is
    /// observed and nothing more, whatever phase it is tagged with: the
    /// player keeps nothing it could put it in.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        if let Message::Announce { seq, .. } = message {
            self.seq = *seq;
        }
        observe(&self.context, message.sender(), n, self.seq);
        let me = &self.context.id;
        if let Message::Announce { seq, view, .. } = message
            && awaited(me, view)
            && let Some(choice) = choose(me, view)
        {
            self.n += 1;
            let message = choice.message(*seq, me.clone(), self.n);
            let to = receivers(&message, me, view);
            act(&self.context, message, to, None, None)?;
        }
        Ok(())
    }
}

/// A choice `me` may make in the phase `view` shows, drawn uniformly from
/// the legal ones: a team of the size the quest needs from every team of
/// that size, `me` included, told in seat order; approval or rejection
/// with even odds; support from a good player, and support or sabotage
/// with even odds from an evil one; and one of the [`candidates`] to name
/// as Merlin. None when there is nobody to name.
fn choose(me: &PlayerId, view: &View) -> Option<Choice> {
    let even = || rand::random_bool(0.5);
    match view.phase {
        Phase::Propose => {
            let mut team: Vec<PlayerId> = view
                .players
                .sample(&mut rand::rng(), view.size)
                .cloned()
                .collect();
            team.sort_by(seat_order);
            Some(Choice::Team(team))
        }
        Phase::Vote => Some(Choice::Ballot(if even() {
            Ballot::Approve
        } else {
            Ballot::Reject
        })),
        Phase::Quest => Some(Choice::Deed(match view.role.team() {
            Team::Evil if even() => Deed::Sabotage,
            _ => Deed::Support,
        })),
        Phase::Assassinate => candidates(me, view)
            .choose(&mut rand::rng())
            .cloned()
            .map(Choice::Target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::config::{Config, Roles};
    use crate::avalon::report::Narrator;
    use crate::avalon::tests::{
        Script, configured, dealt, five, logged, mute, observed_by, outcomes, play, quick, run,
        utterance, view_of,
    };
    use crate::avalon::{PROPOSALS, Role, Rules, Table, game, sabotages_needed, team_size};
    use crate::model::spec::Models;
    use crate::seat::id;
    use crate::seat::{ENVIRONMENT, to_the_environment};
    use crate::strategy::{RANDOM, RoleConfig};
    use free_agent::ActorId;
    use std::collections::{HashMap, HashSet};
    use std::num::NonZero;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    #[tokio::test(start_paused = true)]
    async fn a_game_is_logged_from_the_deal_to_the_end_and_told() {
        let roles = five();
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
        let (winner, entries, took) = run(episode, won, log).await.unwrap();
        // Random players answer at once, so no phase waits out its limit.
        assert!(took < Rules::default().propose_limit, "{took:?}");

        let first = entries.first();
        assert!(
            matches!(first, Some(Entry::Start { roles: dealt, .. }) if *dealt == roles),
            "{first:?}"
        );
        // The configuration comes right after the deal.
        let second = entries.get(1);
        assert!(
            matches!(second, Some(Entry::Configuration(configuration))
                if **configuration == configured(&roles, &Rules::default())),
            "{second:?}"
        );
        let last = entries.last();
        assert!(
            matches!(last, Some(Entry::End { winner: won, .. }) if *won == winner),
            "{last:?}"
        );
        let between = &entries[2..entries.len() - 1];
        // Whom each message, named by its sender and number, was entries as
        // sent to as an action, and who entries observing it.
        let mut actions: HashMap<(&str, u64), &HashSet<ActorId>> = HashMap::new();
        let mut observations: HashMap<(&str, u64), Vec<&str>> = HashMap::new();
        let mut phases = 0;
        for entry in between {
            match entry {
                Entry::Action {
                    from,
                    n,
                    to,
                    message,
                    seen,
                    forgot,
                } => {
                    assert_eq!(*seen, None);
                    assert_eq!(*forgot, None);
                    match message {
                        Message::Announce { .. } => {
                            assert_eq!(from, ENVIRONMENT);
                            assert_eq!(to.len(), 1, "{to:?}");
                        }
                        message => {
                            assert!(message.choice().is_some(), "{message:?}");
                            assert_eq!(message.sender(), *from);
                            assert_eq!(*to, to_the_environment());
                        }
                    }
                    assert_eq!(actions.insert((from, *n), to), None);
                }
                Entry::Observation { by, from, n, .. } => {
                    observations.entry((from, *n)).or_default().push(by);
                }
                entry => {
                    assert!(
                        outcomes(std::slice::from_ref(entry)).len() == 1,
                        "{entry:?}"
                    );
                    phases += 1;
                }
            }
        }
        // Every message is entries once as an action, and once as an
        // observation by each of its receivers.
        let mut sent: Vec<&(&str, u64)> = actions.keys().collect();
        let mut heard: Vec<&(&str, u64)> = observations.keys().collect();
        sent.sort_unstable();
        heard.sort_unstable();
        assert_eq!(sent, heard, "{entries:?}");
        for (message, by) in &observations {
            let by: HashSet<&str> = by.iter().copied().collect();
            let to: HashSet<&str> = actions[message].iter().map(String::as_str).collect();
            assert_eq!(by, to, "{message:?}");
        }
        // Each phase is announced to every player, and ends once.
        let announced = actions
            .keys()
            .filter(|(from, _)| *from == ENVIRONMENT)
            .count();
        assert_eq!(announced, phases * roles.len());
        // Every line of the log tells, and the account ends with the winner.
        let mut narrator = Narrator::default();
        let told: Vec<String> = entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .collect();
        assert_eq!(told[0], "5 players:");
        let won = format!("{winner} wins");
        assert!(
            told[told.len() - 2].to_lowercase().starts_with(&won),
            "{told:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_observes_what_it_is_sent_in_the_phase_it_is_in_and_does_nothing_about_it() {
        // Player 2 talks in every phase, once for the phase before it too.
        // Player 3, who plays at random, observes both in the phase it is
        // in.
        let talker: Script = Box::new(|me, seq, _| {
            vec![
                utterance(seq, me, 100 + seq, "Now."),
                utterance(seq.saturating_sub(1), me, 200 + seq, "Late."),
            ]
        });
        let mut puppets = HashMap::from([("Player 2", Actor::puppet(talker))]);
        for player in ["Player 1", "Player 4", "Player 5"] {
            puppets.insert(player, mute());
        }
        let (_, logged, _) = play(five(), quick(), puppets).await.unwrap();
        let heard: Vec<(&str, u64, u64)> = observed_by(&logged, "Player 3")
            .into_iter()
            .filter(|(from, _, seq)| *from == "Player 2" && *seq == 1)
            .collect();
        assert_eq!(heard, [("Player 2", 101, 1), ("Player 2", 201, 1)]);
        // Player 3 never speaks: all it sends is choices.
        let sent: Vec<&Message> = logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action { from, message, .. } if from == "Player 3" => Some(message),
                _ => None,
            })
            .collect();
        assert!(!sent.is_empty());
        assert!(
            sent.iter().all(|message| message.choice().is_some()),
            "{sent:?}"
        );
    }

    #[test]
    fn a_leader_proposes_a_team_of_the_size_needed_drawn_from_every_team_itself_included() {
        let me = id("Player 1");
        let view = view_of("Player 1", Phase::Propose, &[]);
        let mut teams = HashSet::new();
        for _ in 0..500 {
            let team = choose(&me, &view).unwrap().team().unwrap().clone();
            assert_eq!(team.len(), 2);
            assert_ne!(team[0], team[1]);
            let mut sorted = team.clone();
            sorted.sort_by(seat_order);
            assert_eq!(team, sorted);
            teams.insert(team);
        }
        // Five choose two.
        assert_eq!(teams.len(), 10);
    }

    #[test]
    fn a_player_approves_or_rejects_with_even_odds() {
        let me = id("Player 3");
        let view = view_of("Player 3", Phase::Vote, &["Player 1", "Player 2"]);
        let ballots: Vec<Option<Choice>> = (0..200).map(|_| choose(&me, &view)).collect();
        for ballot in [Ballot::Approve, Ballot::Reject] {
            let cast = ballots
                .iter()
                .filter(|chosen| **chosen == Some(Choice::Ballot(ballot)))
                .count();
            assert!((50..=150).contains(&cast), "{ballot:?}: {cast}");
        }
    }

    #[test]
    fn a_good_member_supports_and_an_evil_one_supports_or_sabotages() {
        let team = ["Player 1", "Player 2"];
        let merlin = view_of("Player 1", Phase::Quest, &team);
        assert_eq!(merlin.role, Role::Merlin);
        for _ in 0..50 {
            assert_eq!(
                choose(&id("Player 1"), &merlin),
                Some(Choice::Deed(Deed::Support))
            );
        }
        let assassin = view_of("Player 2", Phase::Quest, &team);
        let deeds: HashSet<Option<Choice>> = (0..100)
            .map(|_| choose(&id("Player 2"), &assassin))
            .collect();
        assert_eq!(
            deeds,
            HashSet::from([
                Some(Choice::Deed(Deed::Support)),
                Some(Choice::Deed(Deed::Sabotage))
            ])
        );
    }

    #[test]
    fn the_assassin_names_one_of_the_players_it_does_not_see_as_evil() {
        let me = id("Player 2");
        let view = view_of("Player 2", Phase::Assassinate, &[]);
        let named: HashSet<Option<Choice>> = (0..200).map(|_| choose(&me, &view)).collect();
        let expected = ["Player 1", "Player 3", "Player 4"]
            .map(|player| Some(Choice::Target(id(player))))
            .into();
        assert_eq!(named, expected);
        // Seeing everyone else as evil, it names nobody.
        let mut view = view;
        view.players = vec![me.clone(), id("Player 5")];
        assert_eq!(choose(&me, &view), None);
    }

    /// `n` choose `k`, for `k` at most `n`, exactly as a float: every
    /// count here is small.
    fn choose_count(n: usize, k: usize) -> f64 {
        (0..k).fold(1.0, |count, i| count * (n - i) as f64 / (i + 1) as f64)
    }

    /// The chance that a fair coin tossed `n` times comes up heads in a way
    /// `heads` allows.
    fn binomial(n: usize, heads: impl Fn(usize) -> bool) -> f64 {
        (0..=n)
            .filter(|&h| heads(h))
            .map(|h| choose_count(n, h))
            .sum::<f64>()
            / 2f64.powi(n as i32)
    }

    /// The exact chance that good wins an all-random game of `n` players,
    /// `evil` of them evil, with Merlin and the Assassin. A proposal is
    /// approved by a strict majority of even-odds ballots, and a quest goes
    /// unless five in a row are not. A team is drawn uniformly, so the
    /// number of evil players on it is hypergeometric, and each sabotages
    /// with even odds. After three successes the Assassin names one of the
    /// good players uniformly, so good keeps the game unless it names
    /// Merlin.
    fn good_wins_exactly(n: usize, evil: usize) -> f64 {
        let good = n - evil;
        let approved = binomial(n, |approvals| 2 * approvals > n);
        let goes = 1.0 - (1.0 - approved).powi(i32::from(PROPOSALS));
        let succeeds = |quest: usize| {
            let quest = NonZero::new(u8::try_from(quest + 1).unwrap()).unwrap();
            let size = team_size(n, quest);
            let needed = sabotages_needed(n, quest);
            (0..=size.min(evil))
                .map(|on| {
                    let held = choose_count(evil, on) * choose_count(good, size - on)
                        / choose_count(n, size);
                    held * binomial(on, |sabotages| sabotages < needed)
                })
                .sum::<f64>()
        };
        let assassinated = 1.0 - 1.0 / good as f64;
        // value[w][l]: good's chance from w successes and l failures.
        let mut value = [[0.0; 4]; 4];
        for w in (0..=3).rev() {
            for l in (0..=3).rev() {
                value[w][l] = match (w, l) {
                    (3, _) => assassinated,
                    (_, 3) => 0.0,
                    _ => {
                        let p = succeeds(w + l);
                        goes * (p * value[w + 1][l] + (1.0 - p) * value[w][l + 1])
                    }
                };
            }
        }
        value[0][0]
    }

    #[test]
    fn good_wins_an_all_random_game_with_the_exact_chance_its_markov_chain_gives() {
        // The usual splits: 2, 2, 3, 3, 3 and 4 evil for 5 to 10 players.
        let expected = [
            (5, 2, 0.3326),
            (6, 2, 0.2498),
            (7, 3, 0.3771),
            (8, 3, 0.2343),
            (9, 3, 0.4067),
            (10, 4, 0.2337),
        ];
        for (n, evil, chance) in expected {
            let exact = good_wins_exactly(n, evil);
            assert!((exact - chance).abs() < 5e-5, "{n} players: {exact}");
        }
        // The sawtooth is the strict majority: a random team is approved
        // half the time with five players but 22 times in 64 with six.
        assert_eq!(binomial(5, |approvals| 2 * approvals > 5), 0.5);
        assert_eq!(binomial(6, |approvals| 2 * approvals > 6), 22.0 / 64.0);
    }

    /// How many of `games` all-random games at `table` good wins, played
    /// one after another without logging.
    async fn good_wins(table: &Table, games: usize) -> usize {
        let count = |count| RoleConfig {
            count: Some(count),
            ..RoleConfig::default()
        };
        let roles = Roles {
            merlin: count(table.merlin),
            percival: count(table.percival),
            servant: count(table.servants),
            assassin: count(table.assassin),
            morgana: count(table.morgana),
            mordred: count(table.mordred),
            oberon: count(table.oberon),
            minion: count(table.minions),
        };
        let mut config = Config::parse("").unwrap();
        config.roles = roles;
        let settings = config.settle().unwrap();
        let configuration = settings
            .configuration(&settings.deal(), &[], Models::new())
            .unwrap();
        let mut won = 0;
        for _ in 0..games {
            let (winner, result) = oneshot::channel();
            let (logger, _) = unbounded_channel();
            let episode = game(
                settings.deal(),
                settings.rules.clone(),
                configuration.clone(),
                winner,
                |_| Player::init(),
                logger,
            );
            episode.run(Duration::from_secs(60 * 60)).await.unwrap();
            if result.await.unwrap() == Team::Good {
                won += 1;
            }
        }
        won
    }

    /// Plays `games` all-random games at `table` and checks that good wins
    /// within four standard errors of its exact chance.
    async fn plays_to_its_exact_chance(table: Table, games: usize) {
        let evil = table
            .counts()
            .into_iter()
            .filter(|(role, _)| role.team() == Team::Evil)
            .map(|(_, count)| count)
            .sum();
        let exact = good_wins_exactly(table.players(), evil);
        let rate = good_wins(&table, games).await as f64 / games as f64;
        let error = (exact * (1.0 - exact) / games as f64).sqrt();
        assert!(
            (rate - exact).abs() < 4.0 * error,
            "good won {rate} of {games} games, against an exact {exact}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn good_wins_all_random_games_of_the_default_table_at_its_exact_chance() {
        plays_to_its_exact_chance(Table::default(), 2000).await;
    }

    #[tokio::test(start_paused = true)]
    async fn good_wins_all_random_games_of_seven_at_its_exact_chance() {
        // The fourth quest needs two sabotages to fail.
        let table = Table {
            servants: 3,
            minions: 2,
            ..Table::default()
        };
        plays_to_its_exact_chance(table, 2000).await;
    }
}
