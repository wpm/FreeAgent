//! What every game shares in telling its story: the lines it is told in,
//! the order the seats are told in, how a player is named, lists in prose,
//! how long something took, the models in play, whether a message comes
//! late, and how a player's story forgets talk to fit a budget.

use crate::model::spec::Models;
use crate::model::window::{ContextWindow, Source};
use crate::model::{ToolChoice, tokens};
use crate::seat::{PlayerId, PlayerName};
use humantime::format_duration;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::time::Duration;

/// A line of a game's story, as its narrator tells it. The console prints
/// each line as it displays; a viewer tells speech, headings, votes and
/// blank lines apart by what they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A phase heading: `Night 1.`, `Day 2.`, `Quest 1, proposal 1 of 5.
    /// Player 1 (Assassin) leads and needs a team of 2.`
    Heading(String),
    /// One player's vote or selection, or its withdrawal: `Player 4
    /// (werewolf) votes to kill Player 3 (seer).`
    Vote(String),
    /// Anything else the narrator tells: `Player 6 dies.`
    Told(String),
    /// An utterance. `speaker` is the subject as the narrator renders it,
    /// `Player 3 (seer)`, so the role comes from the narrator's own table.
    Said {
        /// Who spoke.
        who: PlayerId,
        /// The speaker as the narrator names it.
        speaker: String,
        /// What was said.
        text: String,
    },
    /// A blank line between paragraphs of the story.
    Blank,
}

impl fmt::Display for Line {
    /// The console's words: a heading, vote or told line as is; `{speaker}
    /// says, "{text}"`; and nothing for a blank line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Line::Heading(line) | Line::Vote(line) | Line::Told(line) => f.write_str(line),
            Line::Said { speaker, text, .. } => write!(f, "{speaker} says, \"{text}\""),
            Line::Blank => Ok(()),
        }
    }
}

/// A line compares with the words the console prints for it.
#[cfg(test)]
impl PartialEq<&str> for Line {
    fn eq(&self, other: &&str) -> bool {
        self.to_string().as_str() == *other
    }
}

/// A line compares with the words the console prints for it.
#[cfg(test)]
impl PartialEq<String> for Line {
    fn eq(&self, other: &String) -> bool {
        self == &other.as_str()
    }
}

/// `elapsed` to the nearest whole second, as `humantime` writes it: `40s`,
/// `1m 5s`.
pub(crate) fn whole_seconds(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64().round() as u64;
    format_duration(Duration::from_secs(seconds)).to_string()
}

/// `players` in seat order.
pub(crate) fn in_order<'a>(players: impl IntoIterator<Item = &'a PlayerId>) -> Vec<&'a PlayerId> {
    let mut players: Vec<&PlayerId> = players.into_iter().collect();
    players.sort_by_cached_key(|player| PlayerName::from(player.as_str()));
    players
}

/// Seat order: the order of the [`PlayerName`]s, so that `Player 2`
/// comes before `Player 10`.
pub fn seat_order(a: &PlayerId, b: &PlayerId) -> Ordering {
    PlayerName::from(a.as_str()).cmp(&PlayerName::from(b.as_str()))
}

/// A player as an account names it: `player (role)` when the role is
/// known, the name alone when it is not.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seat<'a, R> {
    /// The player.
    pub(crate) player: &'a PlayerId,
    /// Its role, when the account knows it.
    pub(crate) role: Option<R>,
}

impl<'a, R: Copy> Seat<'a, R> {
    /// `player` as an account knowing `roles` names it.
    pub(crate) fn named(player: &'a PlayerId, roles: &HashMap<PlayerId, R>) -> Self {
        Seat {
            player,
            role: roles.get(player).copied(),
        }
    }
}

impl<R: fmt::Display> fmt::Display for Seat<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.role {
            Some(role) => write!(f, "{} ({role})", self.player),
            None => f.write_str(self.player),
        }
    }
}

/// `named` as a list in prose: "a", "a and b", "a, b and c".
pub(crate) fn in_prose(named: impl IntoIterator<Item = impl fmt::Display>) -> String {
    let named: Vec<String> = named.into_iter().map(|name| name.to_string()).collect();
    match named.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => named.concat(),
    }
}

/// What a game's configuration tells of `models`, the models in play: each
/// one's context window and where it came from, and, for a model that does
/// not accept a required tool call, that its players are asked to call one.
pub(crate) fn configured(models: &Models) -> Vec<Line> {
    let mut lines = Vec::new();
    for (name, model) in models {
        lines.push(Line::Told(context_window(name, model.context_window)));
        if model.tool_choice == ToolChoice::Auto {
            lines.push(Line::Told(format!(
                "{name} does not accept a required tool call, so its players are asked to call one."
            )));
        }
    }
    lines
}

/// The context window of the model `name` and where it came from, in a
/// line.
fn context_window(name: &str, ContextWindow { tokens, source }: ContextWindow) -> String {
    let source = match source {
        Source::File => "as the configuration file says",
        Source::Provider => "as the provider reports",
        Source::ProviderDefault => "the provider's default",
        Source::Default => "the default",
    };
    format!("{name}'s context window is {tokens} tokens, {source}.")
}

/// Whether a later phase than the one numbered `seq` is among those
/// `announced`, by number: a message for that phase comes late.
pub(crate) fn late<V>(announced: &BTreeMap<u64, V>, seq: u64) -> bool {
    announced.range(seq + 1..).next().is_some()
}

/// The line that stands in place of the last line of talk a story forgets.
pub const FORGOTTEN: &str = "You don't remember what was said before this.";

/// The lines of `account`, each with what it is, fitting within `budget`
/// tokens by estimate once joined by newlines, and how many lines of talk,
/// the lines that are `talk`, were left out to fit. Talk is left out oldest
/// first until the rest fits or none is left, and [`FORGOTTEN`] stands in
/// place of the last line left out, counted with the rest.
pub(crate) fn told_within<L>(
    account: &[(String, L)],
    talk: impl Fn(&L) -> bool,
    budget: u64,
) -> (Vec<&str>, usize) {
    let chars = |line: &str| line.chars().count();
    let talk: Vec<usize> = (0..account.len())
        .filter(|&i| talk(&account[i].1))
        .collect();
    // The length of the lines, each with a newline after it, as each line
    // of talk is left out. Joined, they have one newline fewer.
    let mut length = account
        .iter()
        .map(|(line, _)| chars(line) + 1)
        .sum::<usize>();
    let mut forgot = 0;
    while forgot < talk.len() && tokens(length.saturating_sub(1)) > budget {
        length -= chars(&account[talk[forgot]].0) + 1;
        if forgot == 0 {
            length += chars(FORGOTTEN) + 1;
        }
        forgot += 1;
    }
    let forgotten = &talk[..forgot];
    let lines = account
        .iter()
        .enumerate()
        .filter_map(|(i, (line, _))| match forgotten.binary_search(&i) {
            Ok(at) if at + 1 == forgot => Some(FORGOTTEN),
            Ok(_) => None,
            Err(_) => Some(line.as_str()),
        })
        .collect();
    (lines, forgot)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::estimate;
    use crate::model::spec::ModelInPlay;

    #[test]
    fn a_line_displays_as_the_console_prints_it() {
        let shown = |line: Line| line.to_string();
        assert_eq!(shown(Line::Heading("Night 1.".to_string())), "Night 1.");
        assert_eq!(
            shown(Line::Vote("ann votes against bob.".to_string())),
            "ann votes against bob."
        );
        assert_eq!(shown(Line::Told("bob dies.".to_string())), "bob dies.");
        assert_eq!(
            shown(Line::Said {
                who: "ann".to_string(),
                speaker: "ann (seer)".to_string(),
                text: "Hello.".to_string(),
            }),
            "ann (seer) says, \"Hello.\""
        );
        assert_eq!(shown(Line::Blank), "");
    }

    #[test]
    fn players_come_in_seat_order_with_player_2_before_player_10() {
        let players = ["Player 10", "ann", "Player 2", "Player 1"].map(String::from);
        assert_eq!(
            in_order(&players),
            ["ann", "Player 1", "Player 2", "Player 10"]
        );
    }

    #[test]
    fn a_player_is_named_with_its_role_when_the_account_knows_it() {
        let roles = HashMap::from([("ann".to_string(), "seer")]);
        let named = |player: &str| Seat::named(&player.to_string(), &roles).to_string();
        assert_eq!(named("ann"), "ann (seer)");
        assert_eq!(named("bob"), "bob");
    }

    #[test]
    fn a_duration_is_told_to_the_nearest_whole_second() {
        assert_eq!(whole_seconds(Duration::from_millis(40_400)), "40s");
        assert_eq!(whole_seconds(Duration::from_millis(64_600)), "1m 5s");
    }

    #[test]
    fn three_or_more_are_listed_with_commas_and_a_last_and() {
        let listed = |names: &[&str]| in_prose(names);
        assert_eq!(listed(&[]), "");
        assert_eq!(listed(&["a"]), "a");
        assert_eq!(listed(&["a", "b"]), "a and b");
        assert_eq!(listed(&["a", "b", "c"]), "a, b and c");
        assert_eq!(listed(&["a", "b", "c", "d"]), "a, b, c and d");
    }

    /// The models `models`, each in play under its tool choice within the
    /// default context window.
    pub(crate) fn in_play(models: &[(&str, ToolChoice)]) -> Models {
        models
            .iter()
            .map(|(name, tool_choice)| {
                let model = ModelInPlay {
                    base_url: "http://localhost:1234/v1".to_string(),
                    id: format!("{name}-7b-instruct"),
                    request_timeout: Duration::from_secs(10),
                    api_key_env: None,
                    tool_choice: *tool_choice,
                    context_window: ContextWindow {
                        tokens: 4096,
                        source: Source::Default,
                    },
                };
                (name.to_string(), model)
            })
            .collect()
    }

    #[test]
    fn a_configuration_is_told_model_by_model_and_with_no_models_not_at_all() {
        assert_eq!(
            configured(&in_play(&[
                ("claude", ToolChoice::Auto),
                ("qwen", ToolChoice::Required)
            ])),
            [
                "claude's context window is 4096 tokens, the default.",
                "claude does not accept a required tool call, \
                 so its players are asked to call one.",
                "qwen's context window is 4096 tokens, the default.",
            ]
        );
        assert!(configured(&Models::new()).is_empty());
    }

    #[test]
    fn the_context_window_is_told_with_where_it_came_from() {
        let told = |tokens, source| context_window("qwen", ContextWindow { tokens, source });
        assert_eq!(
            told(4096, Source::Default),
            "qwen's context window is 4096 tokens, the default."
        );
        assert_eq!(
            told(32768, Source::Provider),
            "qwen's context window is 32768 tokens, as the provider reports."
        );
        assert_eq!(
            told(128000, Source::ProviderDefault),
            "qwen's context window is 128000 tokens, the provider's default."
        );
        assert_eq!(
            told(8192, Source::File),
            "qwen's context window is 8192 tokens, as the configuration file says."
        );
    }

    #[test]
    fn a_message_comes_late_once_a_later_phase_is_announced() {
        let announced = BTreeMap::from([(1, ()), (2, ())]);
        assert!(late(&announced, 1));
        assert!(!late(&announced, 2));
        assert!(!late(&announced, 3));
    }

    /// An account of a phase, a line of talk at length, another phase,
    /// and two more lines of talk at length, each line marked whether it
    /// is talk.
    fn account() -> Vec<(String, bool)> {
        let said = |word: &str| [word; 20].join(" ");
        vec![
            ("Day 1.".to_string(), false),
            (said("First."), true),
            ("Day 2.".to_string(), false),
            (said("Second."), true),
            (said("Third."), true),
        ]
    }

    /// The lines of `account`, as a story within a budget gives them.
    fn lines(account: &[(String, bool)]) -> Vec<&str> {
        account.iter().map(|(line, _)| line.as_str()).collect()
    }

    /// The tokens `lines` take, joined by newlines, by estimate.
    pub(crate) fn estimated(lines: &[&str]) -> u64 {
        estimate(&lines.join("\n"))
    }

    /// `account` told within `budget`, its lines of talk marked as such.
    fn within(account: &[(String, bool)], budget: u64) -> (Vec<&str>, usize) {
        told_within(account, |talk| *talk, budget)
    }

    #[test]
    fn an_account_within_its_budget_is_told_whole() {
        let account = account();
        let whole = lines(&account);
        assert_eq!(within(&account, estimated(&whole)), (whole.clone(), 0));
        assert_eq!(within(&account, u64::MAX), (whole, 0));
    }

    #[test]
    fn an_account_over_its_budget_forgets_talk_oldest_first_in_its_place() {
        let account = account();
        let whole = lines(&account);
        let forgetting = |forgot: usize| -> Vec<&str> {
            let talk = [1, 3, 4];
            whole
                .iter()
                .enumerate()
                .filter(|(i, _)| !talk[..forgot].contains(i) || *i == talk[forgot - 1])
                .map(|(i, line)| {
                    if talk[..forgot].contains(&i) {
                        FORGOTTEN
                    } else {
                        line
                    }
                })
                .collect()
        };
        // Only as much talk is forgotten as has to be.
        for forgot in 1..=2 {
            let budget = estimated(&forgetting(forgot));
            assert!(budget < estimated(&whole));
            assert_eq!(within(&account, budget), (forgetting(forgot), forgot));
        }
        // Short of everything, every line of talk is forgotten, and the
        // last one forgotten is where the account says so.
        assert_eq!(
            within(&account, 0),
            (vec!["Day 1.", "Day 2.", FORGOTTEN], 3)
        );
    }

    #[test]
    fn an_account_with_no_talk_is_told_whole_however_far_over_it_is() {
        let quiet: Vec<(String, bool)> = account().into_iter().filter(|(_, talk)| !talk).collect();
        assert_eq!(within(&quiet, 0), (lines(&quiet), 0));
    }
}
