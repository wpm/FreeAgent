//! A configuration file, of any game: its `game` key names the game, and
//! the rest of it is that game's configuration. [`load`] is the one place
//! a configuration file is read.
//!
//! The file's text is parsed twice: once for its `game` alone, and then
//! straight into that game's [`Config`](crate::werewolf::config::Config),
//! so that `toml` places every error inside the game's settings at its line
//! and column.

use crate::model::spec::Spec;
use crate::strategy::{RANDOM, Strategy};
use crate::{avalon, werewolf};
use anyhow::{Context as _, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// One game's settled settings, of either kind, boxed, since the two
/// differ in size.
#[derive(Debug)]
pub enum Game {
    /// A game of Werewolf.
    Werewolf(Box<werewolf::config::Settings>),
    /// A game of Avalon.
    Avalon(Box<avalon::config::Settings>),
}

/// A game, as a configuration file's `game` key names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Name {
    /// `"werewolf"`.
    Werewolf,
    /// `"avalon"`.
    Avalon,
}

/// The one key [`load`] reads first, every other key ignored.
#[derive(Deserialize)]
struct Which {
    game: Name,
}

/// Read the configuration file at `path` once, parse it as the game its
/// `game` key names, and settle it. A file that cannot be read, that has
/// no `game` or names no game by it, that holds a setting the game it
/// names does not have, or that does not parse or settle for any other
/// reason, is an invalid configuration file, and the error names it.
/// Reading it says nothing about the API keys, so a file can be checked
/// without them being set.
pub fn load(path: &Path) -> anyhow::Result<Game> {
    let settled = || -> anyhow::Result<Game> {
        let file = std::fs::read_to_string(path)?;
        let Which { game } = toml::from_str(&file)?;
        Ok(match game {
            Name::Werewolf => {
                Game::Werewolf(Box::new(werewolf::config::Config::parse(&file)?.settle()?))
            }
            Name::Avalon => Game::Avalon(Box::new(avalon::config::Config::parse(&file)?.settle()?)),
        })
    };
    settled().with_context(|| format!("invalid configuration file {}", path.display()))
}

/// Check what every game's configuration file holds alike: its `text`
/// blocks, its `models`, the file's `strategy`, and its `strategies`,
/// whose templates take `variables` besides the text blocks. A text block
/// named like one of the `variables` is an error, since the templates could
/// not tell them apart, and so is a model's context window of 0, a
/// strategy named [`RANDOM`], a file's `strategy` that is not defined, and
/// a prompted strategy naming a model the file does not define, or naming
/// none when the file does not define exactly one. A prompted strategy
/// that names no model, of a file that defines one, is given that one.
pub(crate) fn check_common(
    text: &BTreeMap<String, String>,
    models: &BTreeMap<String, Spec>,
    strategy: Option<&str>,
    strategies: &mut BTreeMap<String, Strategy>,
    variables: &[&str],
) -> anyhow::Result<()> {
    if let Some(taken) = variables.iter().find(|v| text.contains_key(**v)) {
        bail!("[text] has a block named {taken}, which is a template variable");
    }
    if let Some((name, _)) = models
        .iter()
        .find(|(_, model)| model.context_window == Some(0))
    {
        bail!("[models.{name}] context_window is 0; a context window holds at least 1 token");
    }
    if strategies.contains_key(RANDOM) {
        bail!("[strategies.{RANDOM}] is built in, and may not be defined");
    }
    if let Some(name) = strategy
        && name != RANDOM
        && !strategies.contains_key(name)
    {
        bail!("the file's strategy is {name}, which is not a strategy");
    }
    for (name, strategy) in strategies {
        let Strategy::Prompted(prompted) = strategy else {
            continue;
        };
        let model = &prompted.model;
        if model.is_empty() {
            let [only] = models.keys().collect::<Vec<_>>()[..] else {
                bail!(
                    "strategy {name} names no model, which it may leave out only when the file defines exactly one; it defines {}",
                    models.len()
                );
            };
            prompted.model = only.clone();
        } else if !models.contains_key(model) {
            bail!("strategy {name} names the model {model}, which is not defined");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The example file `name` in `configs/`.
    fn example(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("configs")
            .join(name)
    }

    /// A file holding `text`, written for the test `test` at a path of its
    /// own so the tests do not collide.
    fn written(test: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "social-deduction-{}-{test}.toml",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The error loading `text` from a file of its own gives, causes
    /// included, and the file's path.
    fn refused(test: &str, text: &str) -> (String, PathBuf) {
        let path = written(test, text);
        let error = load(&path).unwrap_err();
        (format!("{error:#}"), path)
    }

    /// Assert that `shown` begins by calling the file at `path` invalid,
    /// and goes on to say `said`.
    fn assert_invalid(shown: &str, path: &Path, said: &str) {
        let invalid = format!("invalid configuration file {}: ", path.display());
        assert!(shown.starts_with(&invalid), "{shown}");
        assert!(shown.contains(said), "{shown}");
    }

    /// Assert that `shown` places the error at a line and a column, as
    /// `toml` does.
    fn assert_placed(shown: &str) {
        assert!(
            shown.contains("line ") && shown.contains("column "),
            "{shown}"
        );
    }

    #[test]
    fn each_example_loads_as_the_game_it_names() {
        for (name, named) in [
            ("werewolf/random.toml", Name::Werewolf),
            ("werewolf/prompted.toml", Name::Werewolf),
            ("avalon/random.toml", Name::Avalon),
            ("avalon/prompted.toml", Name::Avalon),
        ] {
            let game = load(&example(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let loaded = match game {
                Game::Werewolf(_) => Name::Werewolf,
                Game::Avalon(_) => Name::Avalon,
            };
            assert_eq!(loaded, named, "{name}");
        }
    }

    #[test]
    fn a_file_with_no_game_or_one_that_names_no_game_is_invalid_and_placed() {
        for (test, text, said) in [
            ("no-game", "strategy = \"random\"\n", "missing field `game`"),
            (
                "chess",
                "game = \"chess\"\n",
                "unknown variant `chess`, expected `werewolf` or `avalon`",
            ),
            ("number", "game = 7\n", "wanted string or table"),
        ] {
            let (shown, path) = refused(test, text);
            assert_invalid(&shown, &path, said);
            assert_placed(&shown);
        }
    }

    #[test]
    fn a_file_that_is_not_toml_is_invalid_and_placed() {
        let (shown, path) = refused("not-toml", "game = \"werewolf\"\n[roles\n");
        assert_invalid(&shown, &path, "unclosed table, expected `]`");
        assert_placed(&shown);
    }

    #[test]
    fn another_game_s_setting_is_an_unknown_key_placed_at_its_line_and_column() {
        for (test, text, said) in [
            (
                "werewolf-merlin",
                "game = \"werewolf\"\n[roles.merlin]\ncount = 1\n",
                "unknown field `merlin`",
            ),
            (
                "werewolf-vote-limit",
                "game = \"werewolf\"\n[phases]\nvote_limit = \"60s\"\n",
                "unknown field `vote_limit`",
            ),
            (
                "avalon-werewolf",
                "game = \"avalon\"\n[roles.werewolf]\ncount = 1\n",
                "unknown field `werewolf`",
            ),
            (
                "avalon-night-limit",
                "game = \"avalon\"\n[phases]\nnight_limit = \"60s\"\n",
                "unknown field `night_limit`",
            ),
        ] {
            let (shown, path) = refused(test, text);
            assert_invalid(&shown, &path, said);
            assert_placed(&shown);
        }
    }

    #[test]
    fn a_file_that_does_not_settle_is_invalid() {
        let (shown, path) = refused(
            "werewolf-ratify",
            "game = \"werewolf\"\n[phases]\nday_limit = \"10s\"\nratify_limit = \"20s\"\n",
        );
        assert_invalid(
            &shown,
            &path,
            "the ratify limit (20s) is longer than the day limit (10s)",
        );
        let (shown, path) = refused(
            "avalon-no-merlin",
            "game = \"avalon\"\n[roles.merlin]\ncount = 0\n[roles.servant]\ncount = 3\n",
        );
        assert_invalid(&shown, &path, "Merlin exactly when it has an Assassin");
    }

    #[test]
    fn a_file_that_is_not_there_is_invalid_and_named() {
        let path = Path::new("no-such-file.toml");
        let shown = format!("{:#}", load(path).unwrap_err());
        assert_invalid(&shown, path, "No such file");
    }

    /// What every game's configuration file holds alike, as written.
    #[derive(Deserialize)]
    struct Common {
        strategy: Option<String>,
        #[serde(default)]
        models: BTreeMap<String, Spec>,
        #[serde(default)]
        text: BTreeMap<String, String>,
        #[serde(default)]
        strategies: BTreeMap<String, Strategy>,
    }

    /// One model, `qwen`, and one prompted strategy, `plain`, naming none.
    const PLAIN: &str = r#"
[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
[strategies.plain]
policy = "prompted"
system = "{{ name }}"
"#;

    /// The strategies `file` defines once checked, with the template
    /// variable `name`, or the error the check gives, causes included.
    fn checked(file: &str) -> Result<BTreeMap<String, Strategy>, String> {
        let mut common: Common = toml::from_str(file).unwrap();
        check_common(
            &common.text,
            &common.models,
            common.strategy.as_deref(),
            &mut common.strategies,
            &["name"],
        )
        .map(|()| common.strategies)
        .map_err(|error| format!("{error:#}"))
    }

    /// Whether `plain`, of `strategies`, is prompted and asks `model`.
    fn plain_asks(strategies: &BTreeMap<String, Strategy>, model: &str) -> bool {
        matches!(&strategies["plain"], Strategy::Prompted(prompted) if prompted.model == model)
    }

    #[test]
    fn a_prompted_strategy_may_leave_its_model_out_only_when_the_file_defines_one() {
        assert!(plain_asks(&checked(PLAIN).unwrap(), "qwen"));
        let two = format!(
            "{PLAIN}[models.claude]\nbase_url = \"https://api.anthropic.com/v1\"\nid = \"claude\"\n"
        );
        assert_eq!(
            checked(&two).unwrap_err(),
            "strategy plain names no model, which it may leave out only when the file defines exactly one; it defines 2"
        );
        let none = "[strategies.plain]\npolicy = \"prompted\"\nsystem = \"Hi.\"\n";
        let shown = checked(none).unwrap_err();
        assert!(shown.ends_with("it defines 0"), "{shown}");
        // Named, either may be asked.
        let named = two.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"claude\"",
        );
        assert!(plain_asks(&checked(&named).unwrap(), "claude"));
    }

    #[test]
    fn a_model_that_is_not_defined_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"claude\"",
        );
        assert_eq!(
            checked(&file).unwrap_err(),
            "strategy plain names the model claude, which is not defined"
        );
    }

    #[test]
    fn the_random_strategy_is_built_in_and_may_not_be_defined() {
        let file = "[strategies.random]\npolicy = \"random\"\n";
        assert_eq!(
            checked(file).unwrap_err(),
            "[strategies.random] is built in, and may not be defined"
        );
    }

    #[test]
    fn a_context_window_of_0_is_an_error_naming_the_model() {
        let file = PLAIN.replace("id = ", "context_window = 0\nid = ");
        assert_eq!(
            checked(&file).unwrap_err(),
            "[models.qwen] context_window is 0; a context window holds at least 1 token"
        );
    }

    #[test]
    fn a_text_block_named_like_a_variable_is_an_error_naming_it() {
        let file = format!("{PLAIN}[text]\nname = \"Taken.\"\n");
        assert_eq!(
            checked(&file).unwrap_err(),
            "[text] has a block named name, which is a template variable"
        );
    }

    #[test]
    fn the_file_s_strategy_is_random_or_one_it_defines() {
        for strategy in [RANDOM, "plain"] {
            checked(&format!("strategy = \"{strategy}\"\n{PLAIN}")).unwrap();
        }
        assert_eq!(
            checked(&format!("strategy = \"cautious\"\n{PLAIN}")).unwrap_err(),
            "the file's strategy is cautious, which is not a strategy"
        );
    }
}
