pub use crate::config::{Config, PolicyConfig};
pub use crate::moderator::{Moderator, Settings};
pub use crate::variation::llm::Llm;
pub use crate::variation::random::UniformRandom;

use crate::{Message, Outcome, PlayerId, moderator};
use anyhow::{Result, ensure};
use free_agent::{ActorId, Ending, Log, Policy, episode};
use rand::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;

/// Everyone tied for the most votes, in a fixed order. No votes means
/// nobody.
pub fn leaders(votes: &[PlayerId]) -> Vec<PlayerId> {
    let mut tally: HashMap<&PlayerId, usize> = HashMap::new();
    for vote in votes {
        *tally.entry(vote).or_default() += 1;
    }
    let Some(most) = tally.values().max().copied() else {
        return vec![];
    };
    let mut leaders: Vec<_> = tally
        .into_iter()
        .filter(|(_, count)| *count == most)
        .map(|(id, _)| id.clone())
        .collect();
    leaders.sort();
    leaders
}

/// Whoever got the most votes, with a tie broken uniformly at random
/// among those tied. No votes choose nobody.
pub fn plurality(votes: &[PlayerId], rng: &mut impl Rng) -> Option<PlayerId> {
    leaders(votes).choose(rng).cloned()
}

/// How long a game may take altogether. Random players answer at once, so
/// this only guards against a game that has gone wrong; a game of models
/// is bounded by its own timing instead.
const TIME_LIMIT: Duration = Duration::from_secs(60 * 60);

/// Play one game as configured. The same seed plays the same game, as
/// far as the players' choices are deterministic. With a log, everything
/// said in the game is recorded on it.
///
/// # Errors
///
/// Fails if the configuration cannot be played, if a model's key is
/// missing, if any actor fails, or if the game does not end within its
/// time limit.
pub async fn play(config: &Config, seed: u64, log: Option<Log<Message>>) -> Result<Outcome> {
    config.check()?;
    let names = config.player_names();
    let (report, outcome) = oneshot::channel();
    let settings = Settings {
        day: config.timing.day(),
        patience: config.timing.patience(),
    };
    let moderator = Moderator::new(names.clone(), config.werewolves, seed, settings, report);

    // The key is read once, up front, so a missing one fails before any
    // actor runs rather than inside one.
    let model = match &config.policy {
        PolicyConfig::Random => None,
        PolicyConfig::Llm(llm) => Some((llm.clone(), llm.api_key()?, reqwest::Client::new())),
    };
    let mut actors: Vec<(ActorId, Box<dyn Policy<Message = Message> + Send>)> =
        vec![(moderator::NAME.to_string(), Box::new(moderator))];
    for (n, name) in names.into_iter().enumerate() {
        let player: Box<dyn Policy<Message = Message> + Send> = match &model {
            None => Box::new(UniformRandom::new(seed.wrapping_add(n as u64 + 1))),
            Some((llm, api_key, client)) => Box::new(Llm::new(
                llm.clone(),
                api_key.clone(),
                config.prompts.clone(),
                client.clone(),
            )),
        };
        actors.push((name, player));
    }

    let ending = episode(actors, None, TIME_LIMIT, log).await?;
    ensure!(ending == Ending::Finished, "the game ran out of time");
    Ok(outcome.await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Team;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn votes(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_plurality_is_whoever_got_the_most_votes() {
        let mut rng = StdRng::seed_from_u64(0);

        let chosen = plurality(&votes(&["ann", "bob", "ann"]), &mut rng);

        assert_eq!(chosen, Some("ann".to_string()));
    }

    #[test]
    fn a_tie_is_broken_among_those_tied() {
        let tied = votes(&["ann", "bob", "cat", "bob", "ann"]);

        for seed in 0..20 {
            let chosen = plurality(&tied, &mut StdRng::seed_from_u64(seed)).unwrap();
            assert!(chosen == "ann" || chosen == "bob", "{chosen}");
        }
    }

    #[test]
    fn leaders_are_everyone_tied_for_the_most_votes() {
        assert_eq!(
            leaders(&votes(&["ann", "bob", "cat", "bob", "ann"])),
            vec!["ann", "bob"]
        );
        assert_eq!(leaders(&votes(&["ann"])), vec!["ann"]);
        assert!(leaders(&[]).is_empty());
    }

    #[test]
    fn no_votes_choose_nobody() {
        assert_eq!(plurality(&[], &mut StdRng::seed_from_u64(0)), None);
    }

    #[tokio::test]
    async fn a_game_with_no_werewolves_is_won_before_it_starts() {
        let outcome = play(&Config::random(5, 0), 1, None).await.unwrap();

        assert_eq!(outcome.winner, Team::Villagers);
        assert_eq!(outcome.rounds.get(), 1);
    }

    #[tokio::test]
    async fn a_game_that_starts_at_parity_is_lost_before_it_starts() {
        let outcome = play(&Config::random(4, 2), 1, None).await.unwrap();

        assert_eq!(outcome.winner, Team::Werewolves);
        assert_eq!(outcome.rounds.get(), 1);
    }

    #[tokio::test]
    async fn every_game_ends_within_as_many_rounds_as_there_are_players() {
        for seed in 0..10 {
            let outcome = play(&Config::random(7, 2), seed, None).await.unwrap();
            assert!(outcome.rounds.get() <= 7, "seed {seed}: {outcome:?}");
        }
    }

    #[tokio::test]
    async fn the_same_seed_plays_the_same_game() {
        let first = play(&Config::random(9, 3), 42, None).await.unwrap();
        let again = play(&Config::random(9, 3), 42, None).await.unwrap();

        assert_eq!(first, again);
    }

    #[tokio::test]
    async fn there_cannot_be_more_werewolves_than_players() {
        assert!(play(&Config::random(3, 4), 0, None).await.is_err());
    }
}
