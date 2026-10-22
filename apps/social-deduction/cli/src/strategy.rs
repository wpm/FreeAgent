//! Social Deduction's strategies: the policies a player may follow, each
//! with its parameters, the strategy [`RANDOM`] that is always defined,
//! and what a file says about one role's seats.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use free_agent_game::strategy::Prompted;
pub use social_deduction_gui_types::settings::RANDOM;

/// A `[strategies.<name>]` table: a policy, named by `policy`, with its
/// parameters, and only those.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "lowercase")]
pub enum Strategy {
    /// Select uniformly at random among the candidates, at once.
    Random(Random),
    /// Ask a language model.
    Prompted(Prompted),
}

/// The random policy's parameters: none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Random {}

/// What the file says about one role.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// How many of the role sit at the table.
    pub count: Option<usize>,
    /// The strategy every seat of the role plays that `strategies` does
    /// not allot.
    pub strategy: Option<String>,
    /// How many seats of the role play each strategy, by name.
    pub strategies: Option<BTreeMap<String, usize>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The strategy a `[strategies.<name>]` table of `table` parses into.
    fn parsed(table: &str) -> Result<Strategy, toml::de::Error> {
        toml::from_str(table)
    }

    #[test]
    fn a_strategy_of_either_policy_parses_and_is_written_with_its_parameters() {
        assert_eq!(
            parsed("policy = \"random\"").unwrap(),
            Strategy::Random(Random {})
        );
        let prompted = parsed("policy = \"prompted\"\nsystem = \"Hi.\"\nmodel = \"qwen\"").unwrap();
        assert_eq!(
            prompted,
            Strategy::Prompted(Prompted {
                system: "Hi.".to_string(),
                model: "qwen".to_string(),
            })
        );
        assert_eq!(
            serde_json::to_value(&prompted).unwrap(),
            json!({"policy": "prompted", "system": "Hi.", "model": "qwen"})
        );
    }

    #[test]
    fn a_strategy_takes_its_policy_s_parameters_and_only_those() {
        for table in [
            "policy = \"random\"\nsystem = \"Hi.\"\n",
            "policy = \"prompted\"\nsystem = \"Hi.\"\ntemperature = 0.5\n",
            "policy = \"prompted\"\nmodel = \"qwen\"\n",
            "policy = \"scripted\"\n",
            "system = \"Hi.\"\n",
        ] {
            assert!(parsed(table).is_err(), "{table}");
        }
    }

    #[test]
    fn what_a_file_says_about_a_role_may_be_left_out() {
        let role: RoleConfig = toml::from_str("count = 2\n[strategies]\nbold = 1\n").unwrap();
        assert_eq!(role.count, Some(2));
        assert_eq!(role.strategy, None);
        assert_eq!(
            role.strategies,
            Some(BTreeMap::from([("bold".to_string(), 1)]))
        );
        assert!(toml::from_str::<RoleConfig>("prompt = \"\"").is_err());
    }
}
